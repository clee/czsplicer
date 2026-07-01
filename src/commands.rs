use crate::builtin;
use crate::filter::{Filter, FilterArgs};
use crate::format::{self, RecordStream};
use crate::mailbox;
use crate::mermaid;
use crate::theme;
use crate::thread::{conversation_root, ThreadBuilder};
use anyhow::{anyhow, Result};
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// small helpers
// ---------------------------------------------------------------------------

/// Parse a JSON stream (either a JSON array or one-record-per-line NDJSON)
/// from `reader`, converting each object to CBOR via the JSON→CBOR bridge and
/// invoking `emit` for it. Returns the number of records emitted. `is_array`
/// selects the input shape (auto-detected by the caller).
fn parse_json_records(
    reader: BufReader<File>,
    is_array: bool,
    mut emit: impl FnMut(&ciborium::Value) -> Result<()>,
) -> Result<u64> {
    let mut count = 0u64;
    if is_array {
        let arr: serde_json::Value = serde_json::from_reader(reader)?;
        if let serde_json::Value::Array(items) = arr {
            for item in items {
                let cbor = format::json_to_cbor(&item);
                emit(&cbor)?;
                count += 1;
            }
        }
    } else {
        for line in reader.lines() {
            let line = line?;
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            let jv: serde_json::Value = serde_json::from_str(t)?;
            let cbor = format::json_to_cbor(&jv);
            emit(&cbor)?;
            count += 1;
        }
    }
    Ok(count)
}

fn output_writer(path: Option<&Path>) -> Result<Box<dyn Write>> {
    match path {
        Some(p) if p != Path::new("-") => Ok(Box::new(BufWriter::new(File::create(p)?))),
        _ => Ok(Box::new(BufWriter::new(std::io::stdout()))),
    }
}

/// Stream every record from `files` (in order), invoking `body` for each
/// record that passes `filter`. Returns the number of records the filter
/// skipped (the "dropped" count); decode/IO errors propagate from `body`.
///
/// This is the common read path for `ls`/`extract`/`grep`/`stats`/`edit`/
/// `merge`/`split`/`thread`: stream the concatenated CBOR-over-zstd records,
/// gate on the filter, then run a per-record transform. `verify` and `info`
/// don't fit this shape (verify collects decode errors instead of
/// propagating them; `info` summarizes whole files) and keep their own loops.
fn for_each_matching_record(
    files: &[PathBuf],
    filter: &Filter,
    mut body: impl FnMut(ciborium::Value) -> Result<()>,
) -> Result<u64> {
    let mut dropped = 0u64;
    for f in files {
        let stream = RecordStream::open(f)?;
        for rec in stream {
            let rec = rec?;
            if filter.matches(&rec) {
                body(rec)?;
            } else {
                dropped += 1;
            }
        }
    }
    Ok(dropped)
}

fn usage_int(rec: &ciborium::Value, key: &str) -> i64 {
    format::field(rec, "usage")
        .and_then(|u| format::field(u, key))
        .and_then(format::as_int)
        .unwrap_or(0)
}

fn rec_cost(rec: &ciborium::Value) -> f64 {
    format::field(rec, "estimated_cost")
        .and_then(|c| format::field(c, "dollars"))
        .and_then(format::as_f64)
        .unwrap_or(0.0)
}

fn rec_id(rec: &ciborium::Value) -> i64 {
    format::rec_int(rec, "id").unwrap_or(-1)
}

fn human_bytes(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if n >= GB {
        format!("{:.2} GB", n as f64 / GB as f64)
    } else if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{:.1} KB", n as f64 / KB as f64)
    } else {
        format!("{n} B")
    }
}

// ---------------------------------------------------------------------------
// info
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct InfoArgs {
    #[arg(required = true)]
    pub files: Vec<PathBuf>,
    #[arg(long)]
    pub json: bool,
}

pub fn cmd_info(args: &InfoArgs) -> Result<()> {
    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut totals = (0u64, 0u64, 0u64); // records, compressed, decompressed

    for f in &args.files {
        let comp = std::fs::metadata(f)?.len();
        let mut count = 0u64;
        let mut min_id: Option<i64> = None;
        let mut max_id: Option<i64> = None;
        let mut first_ts: Option<String> = None;
        let mut last_ts: Option<String> = None;
        let mut min_keys = usize::MAX;
        let mut max_keys = 0usize;
        let mut distinct_keys: std::collections::BTreeSet<String> = Default::default();
        let mut models: std::collections::BTreeSet<String> = Default::default();

        let mut stream = RecordStream::open_counting(f)?;
        for rec in &mut stream {
            let rec = rec?;
            count += 1;
            let id = rec_id(&rec);
            min_id = Some(min_id.map_or(id, |m| m.min(id)));
            max_id = Some(max_id.map_or(id, |m| m.max(id)));
            if let Some(ts) = format::rec_str(&rec, "timestamp") {
                if first_ts.is_none() {
                    first_ts = Some(ts.clone());
                }
                last_ts = Some(ts);
            }
            if let ciborium::Value::Map(m) = &rec {
                min_keys = min_keys.min(m.len());
                max_keys = max_keys.max(m.len());
                for (k, _) in m.iter() {
                    if let Some(s) = format::as_str(k) {
                        distinct_keys.insert(s);
                    }
                }
            }
            if let Some(m) = format::rec_str(&rec, "model") {
                models.insert(m);
            }
        }
        let decomp = stream.decompressed_bytes();
        totals.0 += count;
        totals.1 += comp;
        totals.2 += decomp;

        rows.push(serde_json::json!({
            "file": f.display().to_string(),
            "records": count,
            "compressed_bytes": comp,
            "compressed": human_bytes(comp),
            "decompressed_bytes": decomp,
            "decompressed": human_bytes(decomp),
            "id_min": min_id,
            "id_max": max_id,
            "first_timestamp": first_ts,
            "last_timestamp": last_ts,
            "top_level_keys": distinct_keys.len(),
            "min_top_level_keys": if min_keys == usize::MAX { 0 } else { min_keys },
            "max_top_level_keys": max_keys,
            "models": models.into_iter().collect::<Vec<_>>(),
        }));
    }

    if args.json {
        let mut out = serde_json::Map::new();
        out.insert("files".into(), serde_json::Value::Array(rows));
        out.insert(
            "totals".into(),
            serde_json::json!({
                "records": totals.0,
                "compressed_bytes": totals.1,
                "decompressed_bytes": totals.2,
            }),
        );
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::Value::Object(out))?
        );
    } else {
        println!(
            "{:<48} {:>7} {:>10} {:>10} {:>10} {:>22}",
            "file", "records", "compressed", "decomp", "id range", "first timestamp"
        );
        println!("{}", "-".repeat(120));
        for r in &rows {
            let rng = match (r["id_min"].as_i64(), r["id_max"].as_i64()) {
                (Some(a), Some(b)) if a == b => format!("{a}"),
                (Some(a), Some(b)) => format!("{a}-{b}"),
                _ => "-".into(),
            };
            println!(
                "{:<48} {:>7} {:>10} {:>10} {:>10} {:>22}",
                r["file"].as_str().unwrap_or("-"),
                r["records"].as_i64().unwrap_or(0),
                r["compressed"].as_str().unwrap_or("-"),
                r["decompressed"].as_str().unwrap_or("-"),
                rng,
                r["first_timestamp"].as_str().unwrap_or("-"),
            );
        }
        println!("{}", "-".repeat(120));
        println!(
            "TOTAL: {} records, {} compressed, {} decompressed",
            totals.0,
            human_bytes(totals.1),
            human_bytes(totals.2)
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// ls
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct LsArgs {
    #[arg(required = true)]
    pub files: Vec<PathBuf>,
    #[command(flatten)]
    pub filter: FilterArgs,
    #[arg(long)]
    pub json: bool,
}

pub fn cmd_ls(args: &LsArgs) -> Result<()> {
    let filter = args.filter.build()?;
    let mut out = output_writer(None)?;
    let mut shown = 0u64;

    if !args.json {
        writeln!(
            out,
            "{:>6}  {:<26} {:<28} {:<26} {:>6} {:>9} {:>9} {:>9}",
            "id", "timestamp", "model", "path", "status", "in_tok", "out_tok", "cost$"
        )?;
        writeln!(out, "{}", "-".repeat(130))?;
    }

    for_each_matching_record(&args.files, &filter, |rec| {
        let id = rec_id(&rec);
        let ts = format::rec_str(&rec, "timestamp").unwrap_or_default();
        let model = format::rec_str(&rec, "model").unwrap_or_default();
        let path = format::rec_str(&rec, "path").unwrap_or_default();
        let status = format::rec_int(&rec, "status_code").unwrap_or(0);
        let in_tok = usage_int(&rec, "input_tokens");
        let out_tok = usage_int(&rec, "output_tokens");
        let cost = rec_cost(&rec);

        if args.json {
            let row = serde_json::json!({
                "id": id,
                "timestamp": ts,
                "model": model,
                "path": path,
                "status_code": status,
                "input_tokens": in_tok,
                "output_tokens": out_tok,
                "cost_usd": cost,
            });
            writeln!(out, "{}", serde_json::to_string(&row)?)?;
        } else {
            let ts = format::clip_chars(&ts, 26).to_string();
            let model = format::clip_chars(&model, 28).to_string();
            let path = format::clip_chars(&path, 26).to_string();
            writeln!(
                out,
                "{:>6}  {:<26} {:<28} {:<26} {:>6} {:>9} {:>9} {:>9.4}",
                id, ts, model, path, status, in_tok, out_tok, cost
            )?;
        }
        shown += 1;
        Ok(())
    })?;

    if !args.json {
        eprintln!("{shown} record(s)");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// extract
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct ExtractArgs {
    #[arg(required = true)]
    pub files: Vec<PathBuf>,
    /// Output file (default: stdout).
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Emit a single JSON array instead of NDJSON (buffers in memory).
    #[arg(long)]
    pub array: bool,
    /// Pretty-print (implies --array formatting).
    #[arg(long)]
    pub pretty: bool,
    /// Also dump capture.requestBody / responseBody to <id>.request / <id>.response in DIR.
    #[arg(long, value_name = "DIR")]
    pub bodies: Option<PathBuf>,
    /// Project only these dotted paths (e.g. id,model,usage.input_tokens).
    #[arg(long, value_name = "PATHS", value_delimiter = ',')]
    pub fields: Vec<String>,
    #[command(flatten)]
    pub filter: FilterArgs,
}

pub fn cmd_extract(args: &ExtractArgs) -> Result<()> {
    let filter = args.filter.build()?;
    if let Some(d) = &args.bodies {
        std::fs::create_dir_all(d)?;
    }
    let mut out = output_writer(args.output.as_deref())?;
    let mut count = 0u64;

    if args.array {
        // Collect into a JSON array (buffers all matched records in memory).
        let mut arr: Vec<serde_json::Value> = Vec::new();
        for_each_matching_record(&args.files, &filter, |rec| {
            arr.push(extract_record(&rec, args)?);
            count += 1;
            Ok(())
        })?;
        if args.pretty {
            serde_json::to_writer_pretty(&mut out, &serde_json::Value::Array(arr))?;
        } else {
            serde_json::to_writer(&mut out, &serde_json::Value::Array(arr))?;
        }
        writeln!(out)?;
    } else {
        // NDJSON: one record per line, streaming.
        for_each_matching_record(&args.files, &filter, |rec| {
            let jv = extract_record(&rec, args)?;
            serde_json::to_writer(&mut out, &jv)?;
            writeln!(out)?;
            count += 1;
            Ok(())
        })?;
    }
    eprintln!("{count} record(s) extracted");
    Ok(())
}

fn extract_record(rec: &ciborium::Value, args: &ExtractArgs) -> Result<serde_json::Value> {
    if let Some(bodies_dir) = &args.bodies {
        let id = rec_id(rec);
        if let Some(cap) = format::field(rec, "capture") {
            for (key, suffix) in [("requestBody", "request"), ("responseBody", "response")] {
                if let Some(v) = format::field(cap, key) {
                    let path = bodies_dir.join(format!("{id}.{suffix}"));
                    match v {
                        ciborium::Value::Text(s) => std::fs::write(&path, s)?,
                        ciborium::Value::Bytes(b) => std::fs::write(&path, b)?,
                        _ => {}
                    }
                }
            }
        }
    }

    let full = format::cbor_to_json(rec);
    if args.fields.is_empty() {
        Ok(full)
    } else {
        let mut obj = serde_json::Map::new();
        for f in &args.fields {
            if let Some(cval) = format::path_get(rec, f) {
                obj.insert(f.clone(), format::cbor_to_json(cval));
            }
        }
        Ok(serde_json::Value::Object(obj))
    }
}

// ---------------------------------------------------------------------------
// repack
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct RepackArgs {
    /// Input JSON file (NDJSON or a JSON array). Use - for stdin.
    #[arg(required = true)]
    pub input: PathBuf,
    #[arg(short, long, required = true)]
    pub output: PathBuf,
    /// zstd compression level 1..22 (default 9).
    #[arg(long, default_value_t = 9)]
    pub level: i32,
    /// Emit raw uncompressed .cbor instead of .cbor.zstd.
    #[arg(long)]
    pub raw: bool,
}

pub fn cmd_repack(args: &RepackArgs) -> Result<()> {
    let f = File::open(&args.input)?;
    let mut reader = BufReader::new(f);

    // Auto-detect JSON array vs NDJSON by peeking the first non-whitespace byte.
    let buf = reader.fill_buf()?;
    let is_array = buf
        .iter()
        .find(|b| !b.is_ascii_whitespace())
        .map(|b| *b == b'[')
        .unwrap_or(false);

    let count = if args.raw {
        let mut file = BufWriter::new(File::create(&args.output)?);
        let n = parse_json_records(reader, is_array, |cbor| {
            format::write_cbor_record(cbor, &mut file)
        })?;
        file.flush()?;
        n
    } else {
        let mut packer = format::ZstdPacker::create(&args.output, args.level)?;
        let n = parse_json_records(reader, is_array, |cbor| packer.write_record(cbor))?;
        packer.finish()?;
        n
    };
    eprintln!("{count} record(s) repacked -> {}", args.output.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// edit (redact / strip / drop)
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct EditArgs {
    #[arg(required = true)]
    pub files: Vec<PathBuf>,
    #[arg(short, long, required = true)]
    pub output: PathBuf,
    /// zstd compression level (default 9). Ignored with --json.
    #[arg(long, default_value_t = 9)]
    pub level: i32,
    /// Null out request/response headers.
    #[arg(long)]
    pub strip_headers: bool,
    /// Null out a dotted field path (e.g. capture.requestBody). Repeatable.
    #[arg(long, value_name = "PATH")]
    pub strip: Vec<String>,
    /// Regex to redact from string bodies; matches replaced with the replacement.
    /// Repeatable. Applies to capture.* by default, or whole record with --all-strings.
    #[arg(long, value_name = "REGEX")]
    pub redact: Vec<String>,
    /// Redact using a named preset: email, jwt, apikey, bearer, aws, ipv4,
    /// uuid, creditcard, ssn, or all. Repeatable. Combines with --redact.
    #[arg(long = "redact-preset", value_name = "PRESET")]
    pub redact_presets: Vec<String>,
    /// Apply redact regexes to every string in the record, not just capture bodies.
    #[arg(long)]
    pub all_strings: bool,
    /// Replacement text for redacted matches (default "[REDACTED]").
    #[arg(long, default_value = "[REDACTED]")]
    pub redact_replacement: String,
    /// Emit NDJSON instead of a .cbor.zstd file (output "-" for stdout).
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub filter: FilterArgs,
}

pub fn cmd_edit(args: &EditArgs) -> Result<()> {
    let filter = args.filter.build()?;

    // Merge explicit redact patterns with expanded presets.
    let regexes = compile_redact_regexes(&args.redact, &args.redact_presets)?;

    if args.json && args.output.to_string_lossy() != "-" {
        // writing JSON NDJSON; ensure parent dir exists
        if let Some(p) = args.output.parent() {
            std::fs::create_dir_all(p).ok();
        }
    }

    let mut kept = 0u64;
    let dropped;

    if args.json {
        let mut out = output_writer(Some(args.output.as_path()))?;
        dropped = for_each_matching_record(&args.files, &filter, |mut rec| {
            transform(&mut rec, args, &regexes);
            serde_json::to_writer(&mut out, &format::cbor_to_json(&rec))?;
            writeln!(out)?;
            kept += 1;
            Ok(())
        })?;
        out.flush()?;
    } else {
        let mut packer = format::ZstdPacker::create(&args.output, args.level)?;
        dropped = for_each_matching_record(&args.files, &filter, |mut rec| {
            transform(&mut rec, args, &regexes);
            packer.write_record(&rec)?;
            kept += 1;
            Ok(())
        })?;
        packer.finish()?;
    }
    eprintln!(
        "{kept} record(s) written, {dropped} dropped -> {}",
        args.output.display()
    );
    Ok(())
}

fn transform(rec: &mut ciborium::Value, args: &EditArgs, regexes: &[regex::Regex]) {
    if args.strip_headers {
        format::path_null(rec, "capture.requestHeaders");
        format::path_null(rec, "capture.responseHeaders");
    }
    for p in &args.strip {
        format::path_null(rec, p);
    }
    if !regexes.is_empty() {
        let repl = args.redact_replacement.clone();
        let sub = |s: &str| -> String {
            let mut cur = s.to_string();
            for r in regexes {
                cur = r.replace_all(&cur, repl.as_str()).to_string();
            }
            cur
        };
        if args.all_strings {
            format::redact_strings(rec, &sub);
        } else if let Some(cap) = format::field_mut(rec, "capture") {
            format::redact_strings(cap, &sub);
        }
    }
}

// ---------------------------------------------------------------------------
// stats
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct StatsArgs {
    #[arg(required = true)]
    pub files: Vec<PathBuf>,
    #[arg(long)]
    pub json: bool,
    /// Output format: "text" (default), "json", "mermaid".
    #[arg(long, value_name = "FMT")]
    pub format: Option<String>,
    /// Breakdown dimension: "model" (default), "provider", "path", or "status".
    #[arg(long)]
    pub by: Option<String>,
    #[command(flatten)]
    pub filter: FilterArgs,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StatsDim {
    Model,
    Provider,
    Path,
    Status,
}

fn parse_stats_dim(s: Option<&str>) -> Result<StatsDim> {
    match s {
        None | Some("model") => Ok(StatsDim::Model),
        Some("provider") => Ok(StatsDim::Provider),
        Some("path") => Ok(StatsDim::Path),
        Some("status") => Ok(StatsDim::Status),
        Some(other) => Err(anyhow!(
            "unknown --by `{other}` (expected model|provider|path|status)"
        )),
    }
}

#[derive(Default, Clone)]
struct Bucket {
    count: u64,
    input_tokens: i64,
    output_tokens: i64,
    cached_tokens: i64,
    reasoning_tokens: i64,
    cost: f64,
    duration_ms: i64,
}

pub fn cmd_stats(args: &StatsArgs) -> Result<()> {
    let filter = args.filter.build()?;
    let dim = parse_stats_dim(args.by.as_deref())?;
    let mut total = Bucket::default();
    let mut by_model: BTreeMap<String, Bucket> = Default::default();
    let mut by_provider: BTreeMap<String, Bucket> = Default::default();
    let mut by_path: BTreeMap<String, Bucket> = Default::default();
    let mut by_status: BTreeMap<String, Bucket> = Default::default();
    let mut by_day: BTreeMap<String, f64> = Default::default();
    let mut min_ts: Option<String> = None;
    let mut max_ts: Option<String> = None;

    for_each_matching_record(&args.files, &filter, |rec| {
        let model = format::rec_str(&rec, "model").unwrap_or_else(|| "-".into());
        let provider = model
            .split_once('/')
            .map(|(p, _)| p.to_string())
            .unwrap_or_else(|| "-".into());
        let path = format::rec_str(&rec, "path").unwrap_or_else(|| "-".into());
        let status = format::rec_int(&rec, "status_code")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "-".into());
        let b = Bucket {
            count: 1,
            input_tokens: usage_int(&rec, "input_tokens"),
            output_tokens: usage_int(&rec, "output_tokens"),
            cached_tokens: usage_int(&rec, "cached_tokens"),
            reasoning_tokens: usage_int(&rec, "reasoning_tokens"),
            cost: rec_cost(&rec),
            duration_ms: format::rec_int(&rec, "duration_ms").unwrap_or(0),
        };
        accumulate(&mut total, &b);
        accumulate(by_model.entry(model).or_default(), &b);
        accumulate(by_provider.entry(provider).or_default(), &b);
        accumulate(by_path.entry(path).or_default(), &b);
        accumulate(by_status.entry(status).or_default(), &b);
        if let Some(ts) = format::rec_str(&rec, "timestamp") {
            if let Some(day) = ts.get(..10) {
                *by_day.entry(day.to_string()).or_insert(0.0) += b.cost;
            }
            if min_ts.as_ref().map_or(true, |m| m > &ts) {
                min_ts = Some(ts.clone());
            }
            if max_ts.as_ref().map_or(true, |m| m < &ts) {
                max_ts = Some(ts);
            }
        }
        Ok(())
    })?;

    let want_json = args.json || args.format.as_deref() == Some("json");
    if want_json {
        let j = serde_json::json!({
            "records": total.count,
            "input_tokens": total.input_tokens,
            "output_tokens": total.output_tokens,
            "cached_tokens": total.cached_tokens,
            "reasoning_tokens": total.reasoning_tokens,
            "total_tokens": total.input_tokens + total.output_tokens + total.reasoning_tokens,
            "cost_usd": total.cost,
            "duration_ms_total": total.duration_ms,
            "duration_avg_ms": if total.count > 0 { total.duration_ms as f64 / total.count as f64 } else { 0.0 },
            "first_timestamp": min_ts,
            "last_timestamp": max_ts,
            "by_model": buckets_json(&by_model),
            "by_provider": buckets_json(&by_provider),
            "by_path": buckets_json(&by_path),
            "by_status": buckets_json(&by_status),
        });
        println!("{}", serde_json::to_string_pretty(&j)?);
        return Ok(());
    }

    if args.format.as_deref() == Some("mermaid") {
        return stats_mermaid(dim, &by_model, &by_provider, &by_path, &by_status, &by_day);
    }

    println!("=== czsplicer stats ===");
    println!("records:              {}", total.count);
    println!(
        "time span:            {} .. {}",
        min_ts.as_deref().unwrap_or("-"),
        max_ts.as_deref().unwrap_or("-")
    );
    println!("tokens (input):       {}", total.input_tokens);
    println!("tokens (output):      {}", total.output_tokens);
    println!("tokens (cached):      {}", total.cached_tokens);
    println!("tokens (reasoning):   {}", total.reasoning_tokens);
    println!(
        "tokens (total):       {}",
        total.input_tokens + total.output_tokens + total.reasoning_tokens
    );
    println!("estimated cost:       ${:.4}", total.cost);
    println!("duration total:       {}", human_dur(total.duration_ms));
    if total.count > 0 {
        println!(
            "duration avg:         {}",
            human_dur(total.duration_ms / total.count as i64)
        );
    }

    let (buckets, label): (&BTreeMap<String, Bucket>, &str) = match dim {
        StatsDim::Model => (&by_model, "model"),
        StatsDim::Provider => (&by_provider, "provider"),
        StatsDim::Path => (&by_path, "path"),
        StatsDim::Status => (&by_status, "status"),
    };
    println!("\n=== by {label} ===");
    println!(
        "{:<34} {:>7} {:>9} {:>9} {:>9} {:>9}",
        label, "recs", "in_tok", "out_tok", "reason", "cost$"
    );
    println!("{}", "-".repeat(86));
    let mut rows: Vec<(&String, &Bucket)> = buckets.iter().collect();
    rows.sort_by(|a, b| {
        b.1.cost
            .partial_cmp(&a.1.cost)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for (k, b) in rows {
        let k = format::clip_chars(k, 34);
        println!(
            "{:<34} {:>7} {:>9} {:>9} {:>9} {:>9.4}",
            k, b.count, b.input_tokens, b.output_tokens, b.reasoning_tokens, b.cost
        );
    }
    Ok(())
}

fn accumulate(dst: &mut Bucket, src: &Bucket) {
    dst.count += src.count;
    dst.input_tokens += src.input_tokens;
    dst.output_tokens += src.output_tokens;
    dst.cached_tokens += src.cached_tokens;
    dst.reasoning_tokens += src.reasoning_tokens;
    dst.cost += src.cost;
    dst.duration_ms += src.duration_ms;
}

fn buckets_json(m: &BTreeMap<String, Bucket>) -> serde_json::Value {
    let mut arr = Vec::new();
    for (k, b) in m {
        arr.push(serde_json::json!({
            "key": k,
            "count": b.count,
            "input_tokens": b.input_tokens,
            "output_tokens": b.output_tokens,
            "cached_tokens": b.cached_tokens,
            "reasoning_tokens": b.reasoning_tokens,
            "cost_usd": b.cost,
            "duration_ms": b.duration_ms,
        }));
    }
    serde_json::Value::Array(arr)
}

/// Top-N threshold for high-cardinality dimensions (model, provider). Top-8
/// captures ~95% of cost on a real corpus; the rest collapses to "other".
const MERMAID_TOP_N: usize = 8;

/// Emit stats as Mermaid diagrams: a pie for the selected dimension (top-N
/// collapsed for model/provider) and an xychart of daily cost.
fn stats_mermaid(
    dim: StatsDim,
    by_model: &BTreeMap<String, Bucket>,
    by_provider: &BTreeMap<String, Bucket>,
    by_path: &BTreeMap<String, Bucket>,
    by_status: &BTreeMap<String, Bucket>,
    by_day: &BTreeMap<String, f64>,
) -> Result<()> {
    let (buckets, label, collapse): (&BTreeMap<String, Bucket>, &str, bool) = match dim {
        StatsDim::Model => (by_model, "model", true),
        StatsDim::Provider => (by_provider, "provider", true),
        StatsDim::Path => (by_path, "path", false),
        StatsDim::Status => (by_status, "status", false),
    };

    let mut rows: Vec<(String, f64)> = buckets.iter().map(|(k, b)| (k.clone(), b.cost)).collect();
    if collapse {
        rows = mermaid::top_n(rows, MERMAID_TOP_N, "other");
    } else {
        rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    }
    let slices: Vec<mermaid::Slice> = rows
        .iter()
        .map(|(l, v)| mermaid::Slice {
            label: l.clone(),
            value: *v,
        })
        .collect();
    print!("{}", mermaid::pie(&format!("Cost by {label}"), &slices));

    if !by_day.is_empty() {
        let pts: Vec<mermaid::Point> = by_day
            .iter()
            .map(|(day, c)| mermaid::Point {
                x: day.clone(),
                y: *c,
            })
            .collect();
        print!(
            "{}",
            mermaid::xychart("Cost by day (USD)", &pts, "day", "$")
        );
    }
    Ok(())
}

fn human_dur(ms: i64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}h {:02}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{}.{:03}s", s, ms % 1000)
    }
}

// ---------------------------------------------------------------------------
// failures
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct FailuresArgs {
    #[arg(required = true)]
    pub files: Vec<PathBuf>,
    #[arg(long)]
    pub json: bool,
    /// Output format: "text" (default), "json", "mermaid".
    #[arg(long, value_name = "FMT")]
    pub format: Option<String>,
    /// Include 2xx successes in the histogram (baseline contrast).
    #[arg(long)]
    pub all: bool,
    #[command(flatten)]
    pub filter: FilterArgs,
}

#[derive(Default)]
struct FailBucket {
    by_hour: [u64; 24],
    by_model: BTreeMap<String, u64>,
    total: u64,
}

/// Inline sparkline characters for 8 levels (low → high).
const SPARK: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Render a 24-element hour histogram as a single-line sparkline string.
fn sparkline(hours: &[u64; 24]) -> String {
    let max = *hours.iter().max().unwrap_or(&0);
    if max == 0 {
        return " ".repeat(24);
    }
    hours
        .iter()
        .map(|&c| {
            if c == 0 {
                ' '
            } else if max == 1 {
                SPARK[3]
            } else {
                SPARK[(((c - 1) * 7) as usize / (max - 1) as usize).min(7)]
            }
        })
        .collect()
}

/// Format the top-N non-zero hours as "08▲10  20▲12" annotations.
fn peak_hours(hours: &[u64; 24], top: usize) -> String {
    let mut peaks: Vec<(usize, u64)> = (0..24)
        .filter(|&h| hours[h] > 0)
        .map(|h| (h, hours[h]))
        .collect();
    peaks.sort_by_key(|x| Reverse(x.1));
    peaks
        .iter()
        .take(top)
        .map(|(h, c)| format!("{h:02}▲{c}"))
        .collect::<Vec<_>>()
        .join("  ")
}

pub fn cmd_failures(args: &FailuresArgs) -> Result<()> {
    let filter = args.filter.build()?;
    let mut buckets: BTreeMap<i64, FailBucket> = BTreeMap::new();
    let mut total_scanned = 0u64;

    for_each_matching_record(&args.files, &filter, |rec| {
        total_scanned += 1;
        let status = format::rec_int(&rec, "status_code").unwrap_or(0);
        // Default: only collect non-2xx. --all disables this.
        if !args.all && (200..300).contains(&status) {
            return Ok(());
        }
        let ts = format::rec_str(&rec, "timestamp").unwrap_or_default();
        let hour = ts
            .get(11..13)
            .and_then(|h| h.parse::<usize>().ok())
            .unwrap_or(0)
            .min(23);
        let model = format::rec_str(&rec, "model").unwrap_or_default();
        let b = buckets.entry(status).or_default();
        b.by_hour[hour] += 1;
        b.total += 1;
        *b.by_model.entry(model).or_insert(0) += 1;
        Ok(())
    })?;

    let total_shown: u64 = buckets.values().map(|b| b.total).sum();

    let want_json = args.json || args.format.as_deref() == Some("json");
    if want_json {
        return failures_json(&buckets, total_scanned, total_shown);
    }

    if buckets.is_empty() {
        if args.format.as_deref() == Some("mermaid") {
            return Ok(());
        }
        eprintln!("no failures found ({total_scanned} records scanned)");
        return Ok(());
    }

    if args.format.as_deref() == Some("mermaid") {
        return failures_mermaid(&buckets, total_scanned, total_shown);
    }

    let pct = if total_scanned > 0 {
        total_shown as f64 * 100.0 / total_scanned as f64
    } else {
        0.0
    };
    eprintln!(
        "FAILURES: {} of {} records ({:.1}%) — {} distinct status code(s)",
        total_shown,
        total_scanned,
        pct,
        buckets.len()
    );
    println!();

    // Sort by total descending.
    let mut sorted: Vec<(&i64, &FailBucket)> = buckets.iter().collect();
    sorted.sort_by_key(|x| Reverse(x.1.total));

    // Sparkline table.
    for &(status, b) in &sorted {
        let spark = sparkline(&b.by_hour);
        let peaks = peak_hours(&b.by_hour, 4);
        println!("{status:>5}  {spark}  {:>5}  {peaks}", b.total);
    }
    // Hour ruler aligned under the 24-char sparkline (7-char "NNNNN  " indent).
    // Ticks at hours 0, 8, 16, 23 — the sparkline itself is exactly 24 chars.
    println!("       └───────┬───────┬──────┘");
    println!("       0       8       16    23");
    println!();

    // Model breakdown (only non-2xx unless --all).
    let mut model_map: BTreeMap<String, BTreeMap<i64, u64>> = BTreeMap::new();
    for &(status, b) in &sorted {
        for (model, &cnt) in &b.by_model {
            *model_map
                .entry(model.clone())
                .or_default()
                .entry(*status)
                .or_insert(0) += cnt;
        }
    }
    let mut model_rows: Vec<(&String, u64, &BTreeMap<i64, u64>)> = model_map
        .iter()
        .map(|(m, codes)| {
            let tot: u64 = codes.values().sum();
            (m, tot, codes)
        })
        .collect();
    model_rows.sort_by_key(|x| Reverse(x.1));

    let label = if args.all {
        "BY MODEL (all records)"
    } else {
        "BY MODEL (errors only)"
    };
    println!("{label}");
    for (model, total, codes) in &model_rows {
        let code_strs: Vec<String> = codes.iter().map(|(s, c)| format!("{s}×{c}")).collect();
        println!("  {:<44} {:>5}  {}", model, total, code_strs.join("  "));
    }

    Ok(())
}

fn failures_json(
    buckets: &BTreeMap<i64, FailBucket>,
    total_scanned: u64,
    total_shown: u64,
) -> Result<()> {
    let mut by_status = Vec::new();
    for (status, b) in buckets {
        let hour_arr: Vec<u64> = b.by_hour.to_vec();
        let model_obj: serde_json::Value = b
            .by_model
            .iter()
            .map(|(m, c)| (m.clone(), serde_json::json!(c)))
            .collect();
        by_status.push(serde_json::json!({
            "status": status,
            "count": b.total,
            "by_hour": hour_arr,
            "by_model": model_obj,
        }));
    }
    // Sort by count descending.
    by_status.sort_by(|a, b| {
        b["count"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&a["count"].as_u64().unwrap_or(0))
    });
    let pct = if total_scanned > 0 {
        total_shown as f64 * 100.0 / total_scanned as f64
    } else {
        0.0
    };
    let out = serde_json::json!({
        "records_scanned": total_scanned,
        "records_shown": total_shown,
        "error_rate": (pct / 100.0),
        "by_status": by_status,
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

/// Emit failures as Mermaid: a pie of status-code share, then a timeline of
/// peak error hours grouped by status code.
fn failures_mermaid(
    buckets: &BTreeMap<i64, FailBucket>,
    _total_scanned: u64,
    _total_shown: u64,
) -> Result<()> {
    // Pie: status code share (cardinality is small; render in full).
    let mut sorted: Vec<(&i64, &FailBucket)> = buckets.iter().collect();
    sorted.sort_by_key(|x| Reverse(x.1.total));
    let slices: Vec<mermaid::Slice> = sorted
        .iter()
        .map(|(s, b)| mermaid::Slice {
            label: s.to_string(),
            value: b.total as f64,
        })
        .collect();
    print!("{}", mermaid::pie("Failures by status code", &slices));

    // Timeline: peak hours per status code. One period per status, listing up
    // to 4 peak hours as "HH▲count" events.
    let mut groups: Vec<mermaid::TimelineGroup> = Vec::new();
    for (s, b) in &sorted {
        let mut hours: Vec<(usize, u64)> = (0..24)
            .filter(|&h| b.by_hour[h] > 0)
            .map(|h| (h, b.by_hour[h]))
            .collect();
        hours.sort_by_key(|x| Reverse(x.1));
        let events: Vec<String> = hours
            .iter()
            .take(4)
            .map(|(h, c)| format!("{h:02}: {c}"))
            .collect();
        if !events.is_empty() {
            groups.push(mermaid::TimelineGroup {
                period: format!("status {s}"),
                events,
            });
        }
    }
    print!("{}", mermaid::timeline("Failure peak hours", &groups));
    Ok(())
}

// ---------------------------------------------------------------------------
// grep
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct GrepArgs {
    /// Regex pattern to search for.
    pub pattern: String,
    #[arg(required = true)]
    pub files: Vec<PathBuf>,
    /// Case-insensitive matching.
    #[arg(short = 'i', long)]
    pub ignore_case: bool,
    /// Search only this dotted field path (default: all string/bytes values).
    #[arg(long, value_name = "PATH")]
    pub field: Option<String>,
    /// Print "id: <snippet>" per match (grep-like) instead of the table.
    #[arg(long)]
    pub show_matches: bool,
    /// Print only the count of matching records.
    #[arg(long)]
    pub count: bool,
    /// Output matching records as NDJSON.
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub filter: FilterArgs,
}

/// Build a snippet around the first match of `re` in `s`.
fn snippet_around(re: &regex::Regex, s: &str) -> Option<String> {
    let m = re.find(s)?;
    let pad = 30;
    let start = m.start().saturating_sub(pad);
    let end = (m.end() + pad).min(s.len());
    let mut snip = String::new();
    if start > 0 {
        snip.push_str("...");
    }
    // Trim to char boundaries so we don't panic on mid-codepoint slices.
    // (Manual `is_char_boundary` walk instead of `floor`/`ceil_char_boundary`,
    // which are stable only since 1.91 — this project targets 1.80.)
    let s_start = {
        let mut i = start.min(s.len());
        while !s.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let s_end = {
        let mut i = end.min(s.len());
        while i < s.len() && !s.is_char_boundary(i) {
            i += 1;
        }
        i
    };
    snip.push_str(&s[s_start..s_end]);
    if end < s.len() {
        snip.push_str("...");
    }
    // collapse newlines for single-line display
    Some(snip.replace(['\n', '\r', '\t'], " "))
}

/// Search records for a regex pattern. By default scans all string/bytes values;
/// `--field` narrows to one dotted path.
pub fn cmd_grep(args: &GrepArgs) -> Result<()> {
    let filter = args.filter.build()?;
    let re = regex::RegexBuilder::new(&args.pattern)
        .case_insensitive(args.ignore_case)
        .build()
        .map_err(|e| anyhow!("invalid pattern `{}`: {e}", args.pattern))?;

    let mut out = output_writer(None)?;
    let mut total = 0u64;

    for_each_matching_record(&args.files, &filter, |rec| {
        // Find first matching snippet (or, in --count mode, first match).
        let mut found: Option<String> = None;
        let mut visitor = |s: &str| {
            if found.is_none() {
                if args.count {
                    if re.is_match(s) {
                        found = Some(String::new());
                    }
                } else {
                    found = snippet_around(&re, s);
                }
            }
        };
        if let Some(field_path) = &args.field {
            if let Some(v) = format::path_get(&rec, field_path) {
                format::search_value_strings(v, &mut visitor);
            }
        } else {
            format::search_value_strings(&rec, &mut visitor);
        }

        if found.is_none() {
            return Ok(());
        }
        total += 1;

        if args.count {
            return Ok(());
        }
        if args.json {
            serde_json::to_writer(&mut out, &format::cbor_to_json(&rec))?;
            writeln!(out)?;
        } else {
            let id = rec_id(&rec);
            let model = format::rec_str(&rec, "model").unwrap_or_default();
            let model = format::clip_chars(&model, 24).to_string();
            let snip = found.unwrap();
            if args.show_matches {
                writeln!(out, "{id}: {snip}")?;
            } else {
                writeln!(out, "{:>5}  {:<24}  {}", id, model, snip)?;
            }
        }
        Ok(())
    })?;

    if args.count {
        writeln!(out, "{total}")?;
    } else if !args.json {
        // Table / show-matches: print a visible footer on stdout.
        writeln!(out, "{total} matching record(s)")?;
    } else {
        // JSON mode: footer to stderr so stdout stays valid NDJSON.
        eprintln!("{total} matching record(s)");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// verify
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct VerifyArgs {
    #[arg(required = true)]
    pub files: Vec<PathBuf>,
    #[arg(long)]
    pub json: bool,
}

/// Integrity-check each file: fully decompress and decode every record,
/// reporting any decode error with context. Exits non-zero if any file is bad.
pub fn cmd_verify(args: &VerifyArgs) -> Result<()> {
    let mut all_ok = true;
    let mut results: Vec<serde_json::Value> = Vec::new();

    for f in &args.files {
        let comp = std::fs::metadata(f)?.len();
        let mut stream = RecordStream::open_counting(f)?;
        let mut count = 0u64;
        let mut error: Option<String> = None;

        for res in &mut stream {
            match res {
                Ok(_) => count += 1,
                Err(e) => {
                    error = Some(format!("{e}"));
                    all_ok = false;
                    break;
                }
            }
        }
        let decomp = stream.decompressed_bytes();

        results.push(serde_json::json!({
            "file": f.display().to_string(),
            "ok": error.is_none(),
            "records": count,
            "compressed_bytes": comp,
            "decompressed_bytes": decomp,
            "error": error,
        }));

        if !args.json {
            let tag = if error.is_some() { "FAIL" } else { "ok  " };
            match &error {
                Some(e) => println!(
                    "{tag} {:<48} {:>7} records  {:>10} / {:>10}  ERROR: {}",
                    f.display(),
                    count,
                    human_bytes(comp),
                    human_bytes(decomp),
                    short_error(e),
                ),
                None => println!(
                    "{tag} {:<48} {:>7} records  {:>10} / {:>10}",
                    f.display(),
                    count,
                    human_bytes(comp),
                    human_bytes(decomp),
                ),
            }
        }
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "all_ok": all_ok,
                "results": results,
            }))?
        );
    }

    if !all_ok {
        std::process::exit(1);
    }
    Ok(())
}

fn short_error(e: &str) -> String {
    // Keep verify output to one line.
    let single = e.replace(['\n', '\r'], " ");
    if single.len() > 120 {
        format!("{}...", &single[..117])
    } else {
        single
    }
}

// ---------------------------------------------------------------------------
// redaction presets
// ---------------------------------------------------------------------------

/// Named canned regex patterns for `edit --redact-preset`.
pub const REDACT_PRESETS: &[(&str, &str, &str)] = &[
    (
        "email",
        r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b",
        "email addresses",
    ),
    (
        "jwt",
        r"eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+",
        "JSON Web Tokens",
    ),
    (
        "apikey",
        r"sk-[A-Za-z0-9]{20,}|sk-ant-[A-Za-z0-9_-]{20,}|xai-[A-Za-z0-9]{20,}",
        "API keys (OpenAI/Anthropic/xAI)",
    ),
    ("bearer", r"(?i:bearer\s+[A-Za-z0-9._-]+)", "Bearer tokens"),
    ("aws", r"AKIA[0-9A-Z]{16}", "AWS access key IDs"),
    (
        "secretkey",
        r#"(?i)secret(?:\s+access)?\s+key\b[*`:='"\s]*[A-Za-z0-9/+=]{20,}"#,
        "Labeled secret access keys (any 'Secret key:' / 'Secret access key:' block)",
    ),
    ("ipv4", r"\b(?:\d{1,3}\.){3}\d{1,3}\b", "IPv4 addresses"),
    (
        "uuid",
        r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b",
        "UUIDs",
    ),
    (
        "creditcard",
        r"\b(?:\d[ -]?){13,16}\b",
        "credit card numbers",
    ),
    (
        "ssn",
        r"\b\d{3}-\d{2}-\d{4}\b",
        "US Social Security numbers",
    ),
];

/// Expand preset names into regex patterns. `all` expands to every preset.
pub fn expand_presets(names: &[String]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for name in names {
        if name == "all" {
            for (_, pat, _) in REDACT_PRESETS {
                out.push((*pat).to_string());
            }
            continue;
        }
        match REDACT_PRESETS.iter().find(|(n, _, _)| n == name) {
            Some((_, pat, _)) => out.push(pat.to_string()),
            None => {
                let valid: Vec<&str> = REDACT_PRESETS.iter().map(|(n, _, _)| *n).collect();
                return Err(anyhow!(
                    "unknown redact preset `{name}`; valid: {}, all",
                    valid.join(", ")
                ));
            }
        }
    }
    Ok(out)
}

/// Merge explicit `--redact` patterns with expanded `--redact-preset` names,
/// drop empties (an empty regex would match everywhere and redact the whole
/// body), and compile each into a `regex::Regex`. Shared by `edit` and `thread`.
fn compile_redact_regexes(redact: &[String], presets: &[String]) -> Result<Vec<regex::Regex>> {
    let mut all_patterns = redact.to_vec();
    all_patterns.extend(expand_presets(presets)?);
    all_patterns.retain(|p| !p.is_empty());
    all_patterns
        .iter()
        .map(|p| regex::Regex::new(p))
        .collect::<Result<_, _>>()
        .map_err(|e| anyhow!("invalid redact regex: {e}"))
}

// ---------------------------------------------------------------------------
// merge
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct MergeArgs {
    #[arg(required = true)]
    pub files: Vec<PathBuf>,
    #[arg(short, long, required = true)]
    pub output: PathBuf,
    /// zstd compression level 1..22 (default 9).
    #[arg(long, default_value_t = 9)]
    pub level: i32,
    #[command(flatten)]
    pub filter: FilterArgs,
}

/// Stream many `.cbor.zstd` inputs into a single output (CBOR -> CBOR, no
/// intermediate). Records are emitted in input-file order (directories expand
/// sorted), so an id/time-sorted dump stays sorted.
pub fn cmd_merge(args: &MergeArgs) -> Result<()> {
    let filter = args.filter.build()?;
    let mut packer = format::ZstdPacker::create(&args.output, args.level)?;
    let mut kept = 0u64;
    let dropped = for_each_matching_record(&args.files, &filter, |rec| {
        packer.write_record(&rec)?;
        kept += 1;
        Ok(())
    })?;
    packer.finish()?;
    eprintln!(
        "{kept} record(s) merged, {dropped} dropped -> {}",
        args.output.display()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// split
// ---------------------------------------------------------------------------

#[derive(clap::Args)]
pub struct SplitArgs {
    #[arg(required = true)]
    pub files: Vec<PathBuf>,
    /// Output directory (created if missing).
    #[arg(long, required = true, value_name = "DIR")]
    pub out_dir: PathBuf,
    /// Grouping dimension: day|session|model|provider|path.
    #[arg(long, required = true, value_name = "DIM")]
    pub by: String,
    /// zstd compression level 1..22 (default 9).
    #[arg(long, default_value_t = 9)]
    pub level: i32,
    /// Minimum records for a group to be emitted. Defaults to 2 for `--by session`
    /// (skips single-record throwaways), 1 otherwise.
    #[arg(long, value_name = "N")]
    pub min_records: Option<usize>,
    /// Print a manifest (groups, files, counts) as JSON instead of the table.
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub filter: FilterArgs,
}

#[derive(Clone, Copy)]
enum GroupBy {
    Day,
    Session,
    Model,
    Provider,
    Path,
}

fn parse_group_by(s: &str) -> Result<GroupBy> {
    match s {
        "day" => Ok(GroupBy::Day),
        "session" => Ok(GroupBy::Session),
        "model" => Ok(GroupBy::Model),
        "provider" => Ok(GroupBy::Provider),
        "path" => Ok(GroupBy::Path),
        other => Err(anyhow!(
            "unknown --by `{other}` (expected day|session|model|provider|path)"
        )),
    }
}

/// The grouping key for a record. `None` means the record can't be grouped
/// (missing the relevant field) and is skipped.
fn group_key(by: GroupBy, rec: &ciborium::Value) -> Result<Option<String>> {
    Ok(match by {
        GroupBy::Day => format::rec_str(rec, "timestamp").map(|t| {
            // calendar day = first 10 chars `YYYY-MM-DD`
            t.get(..10).unwrap_or(&t).to_string()
        }),
        GroupBy::Session => {
            // Aperture gives every request a unique session_id, so the raw
            // field is useless for grouping. Group by conversation root
            // instead (first message hash) — same conversation, branches
            // included. Falls back to raw session_id only if the record has
            // no parseable message path.
            match conversation_root(rec)? {
                Some(root) => Some(root.file_key()),
                None => format::rec_str(rec, "session_id").or_else(|| {
                    format::field(rec, "capture")
                        .and_then(|c| format::field(c, "sessionId"))
                        .and_then(format::as_str)
                }),
            }
        }
        GroupBy::Model => format::rec_str(rec, "model"),
        GroupBy::Provider => format::rec_str(rec, "model")
            .map(|m| m.split_once('/').map(|(p, _)| p.to_string()).unwrap_or(m)),
        GroupBy::Path => format::rec_str(rec, "path"),
    })
}

/// Make a group key safe to use as a filename (models/paths contain `/`).
fn sanitize_key(k: &str) -> String {
    k.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | ' ' | '\n' | '\t' => '_',
            c => c,
        })
        .collect()
}

/// Guard against exhausting file descriptors: when a user sets `--min-records 1`
/// on a high-cardinality dimension (e.g. session with thousands of ids), we'd
/// open one writer per group. Cap it and point them at `--min-records`.
const MAX_OPEN_GROUPS: usize = 512;

/// Split one logical stream into per-group `.cbor.zstd` files. Two-pass:
///   1. count records per group (cheap, streaming);
///   2. write only groups whose count >= min_records, one writer each.
///
/// This keeps memory flat and bounds open file handles, while correctly
/// handling interleaved groups (sessions are not time-contiguous).
pub fn cmd_split(args: &SplitArgs) -> Result<()> {
    let filter = args.filter.build()?;
    let by = parse_group_by(&args.by)?;
    let default_min = if matches!(by, GroupBy::Session) { 2 } else { 1 };
    let min_records = args.min_records.unwrap_or(default_min);

    // --- pass 1: counts ---
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut scanned = 0u64;
    for_each_matching_record(&args.files, &filter, |rec| {
        scanned += 1;
        if let Some(k) = group_key(by, &rec)? {
            *counts.entry(k).or_default() += 1;
        }
        Ok(())
    })?;

    let qualifying: BTreeMap<String, u64> = counts
        .iter()
        .filter(|(_, n)| **n >= min_records as u64)
        .map(|(k, n)| (k.clone(), *n))
        .collect();

    if args.json {
        let groups: Vec<serde_json::Value> = qualifying
            .iter()
            .map(|(k, n)| {
                serde_json::json!({
                    "key": k,
                    "file": format!("{}.cbor.zstd", sanitize_key(k)),
                    "records": n,
                })
            })
            .collect();
        let summary = serde_json::json!({
            "scanned": scanned,
            "distinct_groups": counts.len(),
            "min_records": min_records,
            "written": qualifying.len(),
            "groups": groups,
        });
        println!("{}", serde_json::to_string_pretty(&summary)?);
        // In --json mode we still write the files (it's a manifest + work).
    }

    if qualifying.is_empty() {
        eprintln!(
            "no groups have >= {min_records} record(s) (scanned {scanned}, {} distinct group(s))",
            counts.len()
        );
        return Ok(());
    }
    if qualifying.len() > MAX_OPEN_GROUPS {
        return Err(anyhow!(
            "{} group(s) qualify (> {MAX_OPEN_GROUPS} open-file cap); \
             raise --min-records to reduce cardinality",
            qualifying.len()
        ));
    }

    std::fs::create_dir_all(&args.out_dir)?;

    // --- pass 2: write ---
    let mut writers: BTreeMap<String, format::ZstdPacker> = BTreeMap::new();
    for k in qualifying.keys() {
        let path = args.out_dir.join(format!("{}.cbor.zstd", sanitize_key(k)));
        writers.insert(k.clone(), format::ZstdPacker::create(&path, args.level)?);
    }
    for_each_matching_record(&args.files, &filter, |rec| {
        if let Some(k) = group_key(by, &rec)? {
            if let Some(w) = writers.get_mut(&k) {
                w.write_record(&rec)?;
            }
        }
        Ok(())
    })?;
    for (_, w) in writers {
        w.finish()?;
    }

    let total_written: u64 = qualifying.values().sum();
    eprintln!(
        "split {total_written} record(s) into {} file(s) under {} (min_records={min_records}, {} group(s) skipped)",
        qualifying.len(),
        args.out_dir.display(),
        counts.len() - qualifying.len(),
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// thread
// ---------------------------------------------------------------------------

/// Reconstructs conversation threads from request message histories.
///
/// Each record's `capture.requestBody.messages` echoes its full parent path, so
/// the trie over message-content hashes recovers the branching structure.
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ThreadFormat {
    Json,
    Html,
    Mbox,
    Maildir,
}

#[derive(clap::Args)]
pub struct ThreadArgs {
    #[arg(required = true)]
    pub files: Vec<PathBuf>,
    /// Write JSON/HTML/MBOX/Maildir to this path instead of stdout (`-` for stdout).
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Output format: json (default), html (built-in renderer), mbox, maildir.
    #[arg(long, value_enum, default_value = "json")]
    pub format: ThreadFormat,
    /// Body rendering for mbox/maildir: plain, html (multipart/alternative), html-only.
    #[arg(long, value_name = "MODE", default_value = "html")]
    pub body: String,
    /// Dark mode for the built-in HTML renderer.
    #[arg(long)]
    pub dark: bool,
    /// Render HTML using an Adium `.AdiumMessageStyle` bundle instead of the
    /// built-in renderer.
    #[arg(long, value_name = "BUNDLE")]
    pub theme: Option<PathBuf>,
    /// Adium style variant to apply (e.g. "Dark").
    #[arg(long)]
    pub variant: Option<String>,
    /// Redact secrets matching this regex (repeatable). Applied to message
    /// bodies and tool text before rendering.
    #[arg(long, value_name = "REGEX")]
    pub redact: Vec<String>,
    /// Redact preset(s): email, jwt, apikey, bearer, aws, ipv4, uuid, creditcard,
    /// ssn, or `all` (repeatable).
    #[arg(long = "redact-preset", value_name = "NAME")]
    pub redact_presets: Vec<String>,
    /// Replacement text for redacted spans (default `[REDACTED]`).
    #[arg(long, value_name = "TOKEN", default_value = "[REDACTED]")]
    pub redact_replacement: String,
    #[command(flatten)]
    pub filter: FilterArgs,
}

pub fn cmd_thread(args: &ThreadArgs) -> Result<()> {
    let filter = args.filter.build()?;
    // Build the redaction regex set (mirrors `edit`).
    let redexes = compile_redact_regexes(&args.redact, &args.redact_presets)?;
    let redact_repl = args.redact_replacement.clone();
    let do_redact = !redexes.is_empty();
    let redact = |s: &str| -> String {
        let mut cur = s.to_string();
        for r in &redexes {
            cur = r.replace_all(&cur, redact_repl.as_str()).to_string();
        }
        cur
    };

    let mut builder = ThreadBuilder::new();
    let mut total = 0u64;
    let mut with_messages = 0u64;
    for_each_matching_record(&args.files, &filter, |mut rec| {
        if do_redact {
            // Scrub text and valid-UTF-8 byte bodies (e.g. raw HTTP) before
            // the thread builder extracts message content / metadata.
            format::redact_strings(&mut rec, &redact);
        }
        total += 1;
        if builder.add_record(&rec)? {
            with_messages += 1;
        }
        Ok(())
    })?;
    let j = builder.to_json(total, with_messages);

    // Built-in long-form HTML renderer (no external theme bundle).
    if args.format == ThreadFormat::Html && args.theme.is_none() {
        let html = builtin::render_html(&j, args.dark)?;
        write_output(args.output.as_ref(), html.as_bytes(), "html")?;
        eprintln!(
            "{}; built-in HTML{}",
            thread_summary(total, with_messages, &j),
            if args.dark { " (dark)" } else { "" }
        );
        return Ok(());
    }

    // HTML (themed) path: render the forest through an Adium message style.
    if let Some(theme_path) = &args.theme {
        let html = theme::render_forest(&j, theme_path, args.variant.as_deref())?;
        write_output(args.output.as_ref(), html.as_bytes(), "html")?;
        eprintln!(
            "{}; themed -> {}",
            thread_summary(total, with_messages, &j),
            theme_path.display()
        );
        return Ok(());
    }

    // MBOX / Maildir export: each trie node becomes one email, threaded by
    // Message-ID / In-Reply-To. Selected by --format mbox|maildir.
    if args.format == ThreadFormat::Mbox || args.format == ThreadFormat::Maildir {
        let mode = mailbox::BodyMode::parse(&args.body)?;
        if args.format == ThreadFormat::Mbox {
            // mbox may write to stdout (`-` or no -o) or a file.
            let n = match args.output.as_ref() {
                Some(p) if p.as_path() != Path::new("-") => mailbox::write_mbox(&j, mode, p)?,
                _ => {
                    let mut out = std::io::stdout();
                    mailbox::write_mbox_to(&mut out, &j, mode)?
                }
            };
            eprintln!(
                "{} record(s) ({} with messages) -> {} message(s) [{} body] (mbox)",
                total, with_messages, n, args.body,
            );
            return Ok(());
        }
        // maildir requires a real directory path.
        let out = args
            .output
            .as_ref()
            .filter(|p| p.as_path() != Path::new("-"))
            .ok_or_else(|| anyhow!("--format maildir requires -o DIR"))?;
        let n = mailbox::write_maildir(&j, mode, out)?;
        eprintln!(
            "{} record(s) ({} with messages) -> {} message(s) [{} body] -> {} (maildir)",
            total,
            with_messages,
            n,
            args.body,
            out.display()
        );
        return Ok(());
    }

    let pretty = serde_json::to_string_pretty(&j)?;
    let bytes = pretty.into_bytes();
    write_output(args.output.as_ref(), &bytes, "json")?;
    eprintln!("{}", thread_summary(total, with_messages, &j));
    Ok(())
}

/// Common summary line for `cmd_thread`: records scanned, how many carried
/// messages, and the reconstructed thread / branch-point counts. Used by the
/// built-in HTML, themed, and JSON output paths (each appends its own suffix).
fn thread_summary(total: u64, with_messages: u64, j: &serde_json::Value) -> String {
    format!(
        "{} record(s) ({} with messages) -> {} thread(s), {} branch point(s)",
        total, with_messages, j["root_count"], j["branch_count"],
    )
}

/// Write `bytes` to the configured output (file path, `-`/None = stdout).
fn write_output(output: Option<&PathBuf>, bytes: &[u8], kind: &str) -> Result<()> {
    match output {
        Some(p) if p != Path::new("-") => {
            std::fs::write(p, bytes)?;
            eprintln!("wrote {} bytes of {kind} -> {}", bytes.len(), p.display());
        }
        _ => {
            use std::io::Write;
            std::io::stdout().write_all(bytes)?;
            if kind == "json" {
                println!();
            }
        }
    }
    Ok(())
}
