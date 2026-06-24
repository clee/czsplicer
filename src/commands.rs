use crate::filter::FilterArgs;
use crate::format::{self, CountingRecordStream, RecordStream};
use anyhow::{anyhow, Result};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// small helpers
// ---------------------------------------------------------------------------

fn output_writer(path: &Option<PathBuf>) -> Result<Box<dyn Write>> {
    match path {
        Some(p) => Ok(Box::new(BufWriter::new(File::create(p)?))),
        None => Ok(Box::new(BufWriter::new(std::io::stdout()))),
    }
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

pub struct InfoArgs {
    pub files: Vec<PathBuf>,
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
        let mut keys = 0usize;
        let mut models: std::collections::BTreeSet<String> = Default::default();

        let mut stream = CountingRecordStream::open(f)?;
        while let Some(rec) = stream.next() {
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
                keys = m.len();
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
            "top_level_keys": keys,
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

pub struct LsArgs {
    pub files: Vec<PathBuf>,
    pub filter: FilterArgs,
    pub json: bool,
}

pub fn cmd_ls(args: &LsArgs) -> Result<()> {
    let filter = args.filter.build()?;
    let mut out = output_writer(&None)?;
    let mut shown = 0u64;

    if !args.json {
        writeln!(
            out,
            "{:>6}  {:<26} {:<28} {:<26} {:>6} {:>9} {:>9} {:>9}",
            "id", "timestamp", "model", "path", "status", "in_tok", "out_tok", "cost$"
        )?;
        writeln!(out, "{}", "-".repeat(130))?;
    }

    for f in &args.files {
        let mut stream = RecordStream::open(f)?;
        while let Some(rec) = stream.next() {
            let rec = rec?;
            if !filter.matches(&rec) {
                continue;
            }
            shown += 1;
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
                let ts = if ts.len() > 26 {
                    ts[..26].to_string()
                } else {
                    ts
                };
                let model = if model.len() > 28 {
                    model[..28].to_string()
                } else {
                    model
                };
                let path = if path.len() > 26 {
                    path[..26].to_string()
                } else {
                    path
                };
                writeln!(
                    out,
                    "{:>6}  {:<26} {:<28} {:<26} {:>6} {:>9} {:>9} {:>9.4}",
                    id, ts, model, path, status, in_tok, out_tok, cost
                )?;
            }
        }
    }
    if !args.json {
        eprintln!("{shown} record(s)");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// extract
// ---------------------------------------------------------------------------

pub struct ExtractArgs {
    pub files: Vec<PathBuf>,
    pub filter: FilterArgs,
    pub output: Option<PathBuf>,
    pub array: bool,
    pub pretty: bool,
    pub bodies: Option<PathBuf>,
    pub fields: Vec<String>,
}

pub fn cmd_extract(args: &ExtractArgs) -> Result<()> {
    let filter = args.filter.build()?;
    let mut out = output_writer(&args.output)?;
    let mut count = 0u64;

    if args.array {
        // Collect into a JSON array (buffers all matched records in memory).
        let mut arr: Vec<serde_json::Value> = Vec::new();
        for f in &args.files {
            let mut stream = RecordStream::open(f)?;
            while let Some(rec) = stream.next() {
                let rec = rec?;
                if !filter.matches(&rec) {
                    continue;
                }
                arr.push(extract_record(&rec, args)?);
                count += 1;
            }
        }
        if args.pretty {
            serde_json::to_writer_pretty(&mut out, &serde_json::Value::Array(arr))?;
        } else {
            serde_json::to_writer(&mut out, &serde_json::Value::Array(arr))?;
        }
        writeln!(out)?;
    } else {
        // NDJSON: one record per line, streaming.
        for f in &args.files {
            let mut stream = RecordStream::open(f)?;
            while let Some(rec) = stream.next() {
                let rec = rec?;
                if !filter.matches(&rec) {
                    continue;
                }
                let jv = extract_record(&rec, args)?;
                serde_json::to_writer(&mut out, &jv)?;
                writeln!(out)?;
                count += 1;
            }
        }
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
                    std::fs::create_dir_all(bodies_dir).ok();
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

pub struct RepackArgs {
    pub input: PathBuf,
    pub output: PathBuf,
    pub level: i32,
    pub raw: bool, // emit uncompressed .cbor
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

    let mut count = 0u64;

    if args.raw {
        let mut file = BufWriter::new(File::create(&args.output)?);
        let write_one = |v: &ciborium::Value, w: &mut dyn Write| -> Result<()> {
            format::write_cbor_record(v, w)
        };
        if is_array {
            let arr: serde_json::Value = serde_json::from_reader(reader)?;
            if let serde_json::Value::Array(items) = arr {
                for item in items {
                    let cbor = format::json_to_cbor(&item);
                    write_one(&cbor, &mut file)?;
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
                write_one(&cbor, &mut file)?;
                count += 1;
            }
        }
        file.flush()?;
    } else {
        let mut packer = format::ZstdPacker::create(&args.output, args.level)?;
        if is_array {
            let arr: serde_json::Value = serde_json::from_reader(reader)?;
            if let serde_json::Value::Array(items) = arr {
                for item in items {
                    let cbor = format::json_to_cbor(&item);
                    packer.write_record(&cbor)?;
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
                packer.write_record(&cbor)?;
                count += 1;
            }
        }
        packer.finish()?;
    }
    eprintln!("{count} record(s) repacked -> {}", args.output.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// edit (redact / strip / drop)
// ---------------------------------------------------------------------------

pub struct EditArgs {
    pub files: Vec<PathBuf>,
    pub filter: FilterArgs,
    pub output: PathBuf,
    pub level: i32,
    pub strip_headers: bool,
    pub strip: Vec<String>,
    pub redact: Vec<String>,
    pub redact_presets: Vec<String>,
    pub all_strings: bool,
    pub redact_replacement: String,
    pub json: bool,
}

pub fn cmd_edit(args: &EditArgs) -> Result<()> {
    let filter = args.filter.build()?;

    // Merge explicit redact patterns with expanded presets.
    let mut all_patterns = args.redact.clone();
    all_patterns.extend(expand_presets(&args.redact_presets)?);
    all_patterns = all_patterns.into_iter().filter(|p| !p.is_empty()).collect();

    let regexes: Vec<regex::Regex> = all_patterns
        .iter()
        .map(|p| regex::Regex::new(p))
        .collect::<Result<_, _>>()
        .map_err(|e| anyhow!("invalid redact regex: {e}"))?;

    if args.json && args.output.to_string_lossy() != "-" {
        // writing JSON NDJSON; ensure parent dir exists
        if let Some(p) = args.output.parent() {
            std::fs::create_dir_all(p).ok();
        }
    }

    let mut kept = 0u64;
    let mut dropped = 0u64;

    if args.json {
        let mut out = BufWriter::new(match args.output.to_string_lossy().as_ref() {
            "-" => Box::new(std::io::stdout()) as Box<dyn Write>,
            _ => Box::new(File::create(&args.output)?),
        });
        for f in &args.files {
            let mut stream = RecordStream::open(f)?;
            while let Some(rec) = stream.next() {
                let mut rec = rec?;
                if !filter.matches(&rec) {
                    dropped += 1;
                    continue;
                }
                transform(&mut rec, args, &regexes);
                serde_json::to_writer(&mut out, &format::cbor_to_json(&rec))?;
                writeln!(out)?;
                kept += 1;
            }
        }
        out.flush()?;
    } else {
        let mut packer = format::ZstdPacker::create(&args.output, args.level)?;
        for f in &args.files {
            let mut stream = RecordStream::open(f)?;
            while let Some(rec) = stream.next() {
                let mut rec = rec?;
                if !filter.matches(&rec) {
                    dropped += 1;
                    continue;
                }
                transform(&mut rec, args, &regexes);
                packer.write_record(&rec)?;
                kept += 1;
            }
        }
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

pub struct StatsArgs {
    pub files: Vec<PathBuf>,
    pub filter: FilterArgs,
    pub json: bool,
    pub by: Option<String>,
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
    let mut min_ts: Option<String> = None;
    let mut max_ts: Option<String> = None;

    for f in &args.files {
        let mut stream = RecordStream::open(f)?;
        while let Some(rec) = stream.next() {
            let rec = rec?;
            if !filter.matches(&rec) {
                continue;
            }
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
                if min_ts.as_ref().map_or(true, |m| m > &ts) {
                    min_ts = Some(ts.clone());
                }
                if max_ts.as_ref().map_or(true, |m| m < &ts) {
                    max_ts = Some(ts);
                }
            }
        }
    }

    if args.json {
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
        let k = if k.len() > 34 { &k[..34] } else { k };
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

// expose RecordStream type for main if needed
#[allow(dead_code)]
pub fn _ensure_stream_link(_: &RecordStream) {}

#[allow(dead_code)]
pub fn _ensure_counting_link(_: &CountingRecordStream, _: &Path) {}

// ---------------------------------------------------------------------------
// grep
// ---------------------------------------------------------------------------

pub struct GrepArgs {
    pub files: Vec<PathBuf>,
    pub pattern: String,
    pub ignore_case: bool,
    pub field: Option<String>,
    pub show_matches: bool,
    pub count: bool,
    pub json: bool,
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
    let s_start = s.floor_char_boundary(start.min(s.len()));
    let s_end = s.ceil_char_boundary(end);
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

    let mut out = output_writer(&None)?;
    let mut total = 0u64;

    for f in &args.files {
        let mut stream = RecordStream::open(f)?;
        while let Some(rec) = stream.next() {
            let rec = rec?;
            if !filter.matches(&rec) {
                continue;
            }

            // Find first matching snippet.
            let mut found: Option<String> = None;
            let mut visitor = |s: &str| {
                if found.is_none() {
                    found = snippet_around(&re, s);
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
                continue;
            }
            total += 1;

            if args.count {
                continue;
            }
            if args.json {
                serde_json::to_writer(&mut out, &format::cbor_to_json(&rec))?;
                writeln!(out)?;
            } else {
                let id = rec_id(&rec);
                let model = format::rec_str(&rec, "model").unwrap_or_default();
                let model = if model.len() > 24 {
                    model[..24].to_string()
                } else {
                    model
                };
                let snip = found.unwrap();
                if args.show_matches {
                    writeln!(out, "{id}: {snip}")?;
                } else {
                    writeln!(out, "{:>5}  {:<24}  {}", id, model, snip)?;
                }
            }
        }
    }

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

pub struct VerifyArgs {
    pub files: Vec<PathBuf>,
    pub json: bool,
}

/// Integrity-check each file: fully decompress and decode every record,
/// reporting any decode error with context. Exits non-zero if any file is bad.
pub fn cmd_verify(args: &VerifyArgs) -> Result<()> {
    let mut all_ok = true;
    let mut results: Vec<serde_json::Value> = Vec::new();

    for f in &args.files {
        let comp = std::fs::metadata(f)?.len();
        let mut stream = CountingRecordStream::open(f)?;
        let mut count = 0u64;
        let mut error: Option<String> = None;

        while let Some(res) = stream.next() {
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

/// Print available presets (for `edit --list-presets`).
#[allow(dead_code)]
pub fn print_presets() {
    println!("{:<12} {}", "preset", "matches");
    println!("{}", "-".repeat(50));
    for (name, _pat, desc) in REDACT_PRESETS {
        println!("{:<12} {desc}", name);
    }
    println!("{:<12} all of the above", "all");
}

// ---------------------------------------------------------------------------
// merge
// ---------------------------------------------------------------------------

pub struct MergeArgs {
    pub files: Vec<PathBuf>,
    pub output: PathBuf,
    pub level: i32,
    pub filter: FilterArgs,
}

/// Stream many `.cbor.zstd` inputs into a single output (CBOR -> CBOR, no
/// intermediate). Records are emitted in input-file order (directories expand
/// sorted), so an id/time-sorted dump stays sorted.
pub fn cmd_merge(args: &MergeArgs) -> Result<()> {
    let filter = args.filter.build()?;
    let mut packer = format::ZstdPacker::create(&args.output, args.level)?;
    let mut kept = 0u64;
    let mut dropped = 0u64;
    for f in &args.files {
        let mut stream = RecordStream::open(f)?;
        while let Some(rec) = stream.next() {
            let rec = rec?;
            if !filter.matches(&rec) {
                dropped += 1;
                continue;
            }
            packer.write_record(&rec)?;
            kept += 1;
        }
    }
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

pub struct SplitArgs {
    pub files: Vec<PathBuf>,
    pub out_dir: PathBuf,
    pub by: String,
    pub level: i32,
    pub min_records: Option<usize>,
    pub filter: FilterArgs,
    pub json: bool,
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
fn group_key(by: GroupBy, rec: &ciborium::Value) -> Option<String> {
    match by {
        GroupBy::Day => format::rec_str(rec, "timestamp").map(|t| {
            // calendar day = first 10 chars `YYYY-MM-DD`
            t.get(..10).unwrap_or(&t).to_string()
        }),
        GroupBy::Session => format::rec_str(rec, "session_id").or_else(|| {
            format::field(rec, "capture")
                .and_then(|c| format::field(c, "sessionId"))
                .and_then(format::as_str)
        }),
        GroupBy::Model => format::rec_str(rec, "model"),
        GroupBy::Provider => format::rec_str(rec, "model")
            .map(|m| m.split_once('/').map(|(p, _)| p.to_string()).unwrap_or(m)),
        GroupBy::Path => format::rec_str(rec, "path"),
    }
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
    for f in &args.files {
        let mut stream = RecordStream::open(f)?;
        while let Some(rec) = stream.next() {
            let rec = rec?;
            if !filter.matches(&rec) {
                continue;
            }
            scanned += 1;
            if let Some(k) = group_key(by, &rec) {
                *counts.entry(k).or_default() += 1;
            }
        }
    }

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
    for f in &args.files {
        let mut stream = RecordStream::open(f)?;
        while let Some(rec) = stream.next() {
            let rec = rec?;
            if !filter.matches(&rec) {
                continue;
            }
            if let Some(k) = group_key(by, &rec) {
                if let Some(w) = writers.get_mut(&k) {
                    w.write_record(&rec)?;
                }
            }
        }
    }
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
