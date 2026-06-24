mod commands;
mod filter;
mod format;

use anyhow::Result;
use clap::Parser;
use std::path::{Path, PathBuf};

/// `czsplicer`: inspect, extract, edit/redact, and repack `.cbor.zstd` log
/// streams (e.g. capture-log exports from Tailscale Aperture).
///
/// Files are concatenated streams of CBOR map records, zstd-compressed.
/// Editing model: `extract` -> edit JSON (jq/any editor) -> `repack`,
/// or use `edit` for scripted transforms (redact / strip / drop).
#[derive(Parser)]
#[command(name = "czsplicer", version, propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Subcommand)]
enum Cmd {
    /// Show per-file summary: record counts, sizes, id/timestamp ranges, schema.
    Info {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// List records as a table (or NDJSON with --json).
    Ls(LsCli),
    /// Extract records to JSON (NDJSON by default, or a JSON array).
    Extract(ExtractCli),
    /// Re-encode JSON (NDJSON or array) back to a .cbor.zstd file.
    Repack(RepackCli),
    /// Transform records in a single pass: redact secrets, strip fields, drop/select.
    Edit(EditCli),
    /// Search records for a regex pattern in their string/bytes values.
    Grep(GrepCli),
    /// Integrity-check files: fully decode every record, report any corruption.
    Verify(VerifyCli),
    /// Merge many `.cbor.zstd` files into one (CBOR -> CBOR, streaming).
    Merge(MergeCli),
    /// Split one stream into per-group `.cbor.zstd` files (by day/session/model/path).
    Split(SplitCli),
    /// Aggregate stats: tokens, cost, durations, by-model / by-path.
    Stats(StatsCli),
}

#[derive(clap::Args)]
struct LsCli {
    #[arg(required = true)]
    files: Vec<PathBuf>,
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    filter: filter::FilterArgs,
}

#[derive(clap::Args)]
struct ExtractCli {
    #[arg(required = true)]
    files: Vec<PathBuf>,
    /// Output file (default: stdout).
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Emit a single JSON array instead of NDJSON (buffers in memory).
    #[arg(long)]
    array: bool,
    /// Pretty-print (implies --array formatting).
    #[arg(long)]
    pretty: bool,
    /// Also dump capture.requestBody / responseBody to <id>.request / <id>.response in DIR.
    #[arg(long, value_name = "DIR")]
    bodies: Option<PathBuf>,
    /// Project only these dotted paths (e.g. id,model,usage.input_tokens).
    #[arg(long, value_name = "PATHS")]
    fields: Option<String>,
    #[command(flatten)]
    filter: filter::FilterArgs,
}

#[derive(clap::Args)]
struct RepackCli {
    /// Input JSON file (NDJSON or a JSON array). Use - for stdin.
    #[arg(required = true)]
    input: PathBuf,
    #[arg(short, long, required = true)]
    output: PathBuf,
    /// zstd compression level 1..22 (default 9).
    #[arg(long, default_value_t = 9)]
    level: i32,
    /// Emit raw uncompressed .cbor instead of .cbor.zstd.
    #[arg(long)]
    raw: bool,
}

#[derive(clap::Args)]
struct EditCli {
    #[arg(required = true)]
    files: Vec<PathBuf>,
    #[arg(short, long, required = true)]
    output: PathBuf,
    /// zstd compression level (default 9). Ignored with --json.
    #[arg(long, default_value_t = 9)]
    level: i32,
    /// Null out request/response headers.
    #[arg(long)]
    strip_headers: bool,
    /// Null out a dotted field path (e.g. capture.requestBody). Repeatable.
    #[arg(long, value_name = "PATH")]
    strip: Vec<String>,
    /// Regex to redact from string bodies; matches replaced with the replacement.
    /// Repeatable. Applies to capture.* by default, or whole record with --all-strings.
    #[arg(long, value_name = "REGEX")]
    redact: Vec<String>,
    /// Redact using a named preset: email, jwt, apikey, bearer, aws, ipv4,
    /// uuid, creditcard, ssn, or all. Repeatable. Combines with --redact.
    #[arg(long = "redact-preset", value_name = "PRESET")]
    redact_presets: Vec<String>,
    /// Apply redact regexes to every string in the record, not just capture bodies.
    #[arg(long)]
    all_strings: bool,
    /// Replacement text for redacted matches (default "[REDACTED]").
    #[arg(long, default_value = "[REDACTED]")]
    redact_replacement: String,
    /// Emit NDJSON instead of a .cbor.zstd file (output "-" for stdout).
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    filter: filter::FilterArgs,
}

#[derive(clap::Args)]
struct GrepCli {
    /// Regex pattern to search for.
    pattern: String,
    #[arg(required = true)]
    files: Vec<PathBuf>,
    /// Case-insensitive matching.
    #[arg(short = 'i', long)]
    ignore_case: bool,
    /// Search only this dotted field path (default: all string/bytes values).
    #[arg(long, value_name = "PATH")]
    field: Option<String>,
    /// Print "id: <snippet>" per match (grep-like) instead of the table.
    #[arg(long)]
    show_matches: bool,
    /// Print only the total match count.
    #[arg(long)]
    count: bool,
    /// Output matching records as NDJSON.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    filter: filter::FilterArgs,
}

#[derive(clap::Args)]
struct VerifyCli {
    #[arg(required = true)]
    files: Vec<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args)]
struct MergeCli {
    #[arg(required = true)]
    files: Vec<PathBuf>,
    #[arg(short, long, required = true)]
    output: PathBuf,
    /// zstd compression level 1..22 (default 9).
    #[arg(long, default_value_t = 9)]
    level: i32,
    #[command(flatten)]
    filter: filter::FilterArgs,
}

#[derive(clap::Args)]
struct SplitCli {
    #[arg(required = true)]
    files: Vec<PathBuf>,
    /// Output directory (created if missing).
    #[arg(long, required = true, value_name = "DIR")]
    out_dir: PathBuf,
    /// Grouping dimension: day|session|model|provider|path.
    #[arg(long, required = true, value_name = "DIM")]
    by: String,
    /// zstd compression level 1..22 (default 9).
    #[arg(long, default_value_t = 9)]
    level: i32,
    /// Minimum records for a group to be emitted. Defaults to 2 for `--by session`
    /// (skips single-record throwaways), 1 otherwise.
    #[arg(long, value_name = "N")]
    min_records: Option<usize>,
    /// Print a manifest (groups, files, counts) as JSON instead of the table.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    filter: filter::FilterArgs,
}

#[derive(clap::Args)]
struct StatsCli {
    #[arg(required = true)]
    files: Vec<PathBuf>,
    #[arg(long)]
    json: bool,
    /// Breakdown dimension: "model" (default), "provider", "path", or "status".
    #[arg(long)]
    by: Option<String>,
    #[command(flatten)]
    filter: filter::FilterArgs,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Info { files, json } => commands::cmd_info(&commands::InfoArgs {
            files: expand(files)?,
            json,
        }),
        Cmd::Ls(c) => commands::cmd_ls(&commands::LsArgs {
            files: expand(c.files)?,
            filter: c.filter,
            json: c.json,
        }),
        Cmd::Extract(c) => commands::cmd_extract(&commands::ExtractArgs {
            files: expand(c.files)?,
            filter: c.filter,
            output: c.output,
            array: c.array,
            pretty: c.pretty,
            bodies: c.bodies,
            fields: c
                .fields
                .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
                .unwrap_or_default(),
        }),
        Cmd::Repack(c) => commands::cmd_repack(&commands::RepackArgs {
            input: c.input,
            output: c.output,
            level: c.level,
            raw: c.raw,
        }),
        Cmd::Edit(c) => commands::cmd_edit(&commands::EditArgs {
            files: expand(c.files)?,
            filter: c.filter,
            output: c.output,
            level: c.level,
            strip_headers: c.strip_headers,
            strip: c.strip,
            redact: c.redact,
            redact_presets: c.redact_presets,
            all_strings: c.all_strings,
            redact_replacement: c.redact_replacement,
            json: c.json,
        }),
        Cmd::Grep(c) => commands::cmd_grep(&commands::GrepArgs {
            files: expand(c.files)?,
            pattern: c.pattern,
            ignore_case: c.ignore_case,
            field: c.field,
            show_matches: c.show_matches,
            count: c.count,
            json: c.json,
            filter: c.filter,
        }),
        Cmd::Verify(c) => commands::cmd_verify(&commands::VerifyArgs {
            files: expand(c.files)?,
            json: c.json,
        }),
        Cmd::Merge(c) => commands::cmd_merge(&commands::MergeArgs {
            files: expand(c.files)?,
            output: c.output,
            level: c.level,
            filter: c.filter,
        }),
        Cmd::Split(c) => commands::cmd_split(&commands::SplitArgs {
            files: expand(c.files)?,
            out_dir: c.out_dir,
            by: c.by,
            level: c.level,
            min_records: c.min_records,
            filter: c.filter,
            json: c.json,
        }),
        Cmd::Stats(c) => commands::cmd_stats(&commands::StatsArgs {
            files: expand(c.files)?,
            filter: c.filter,
            json: c.json,
            by: c.by,
        }),
    }
}

/// Expand directory arguments into their sorted `*.cbor.zstd` contents.
fn expand(paths: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        if p.is_dir() {
            let mut entries: Vec<PathBuf> = std::fs::read_dir(&p)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.extension().and_then(|e| e.to_str()) == Some("zstd")
                        && p.file_stem()
                            .and_then(|e| e.to_str())
                            .map_or(false, |s| s.ends_with(".cbor"))
                })
                .collect();
            entries.sort();
            out.extend(entries);
        } else {
            out.push(p);
        }
    }
    Ok(out)
}

#[allow(dead_code)]
fn _path_link(_: &Path) {}
