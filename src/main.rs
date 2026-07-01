mod builtin;
mod commands;
mod csv;
mod filter;
mod format;
mod mailbox;
mod markdown;
mod md_thread;
mod mermaid;
mod redact;
mod render;
mod secrets;
mod theme;
mod thread;

use anyhow::Result;
use clap::Parser;
use commands::*;
use std::path::PathBuf;

/// `czsplicer`: inspect, extract, edit/redact, and repack `.cbor.zstd` log
/// streams (e.g. capture-log exports from Tailscale Aperture).
///
/// Files are concatenated streams of CBOR map records, zstd-compressed.
/// Editing model: `extract` -> edit JSON (jq/any editor) -> `repack`,
/// or use `edit` for scripted transforms (redact / strip / drop).
///
/// Quick start — looking at an export:
///
///   czsplicer info prod/             # what's in here? counts, sizes, ranges
///   czsplicer ls prod/               # one row per record
///   czsplicer stats prod/ --by model # tokens / cost / latency, grouped
///   czsplicer thread prod/ --html -o threads.html   # readable conversations
///   czsplicer verify prod/           # integrity-check every record
///
/// Every selection command (ls/extract/edit/grep/stats/thread/...) shares the
/// same filter flags (`--id`, `--model`, `--since`, `--invert`, ...).
#[derive(Parser)]
#[command(
    name = "czsplicer",
    version,
    propagate_version = true,
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Subcommand)]
enum Cmd {
    /// Show per-file summary: record counts, sizes, id/timestamp ranges, schema.
    Info(InfoArgs),
    /// List records as a table (or NDJSON with --json).
    Ls(LsArgs),
    /// Extract records to JSON (NDJSON by default, or a JSON array).
    Extract(ExtractArgs),
    /// Re-encode JSON (NDJSON or array) back to a .cbor.zstd file.
    Repack(RepackArgs),
    /// Transform records in a single pass: redact secrets, strip fields, drop/select.
    Edit(EditArgs),
    /// Search records for a regex pattern in their string/bytes values.
    Grep(GrepArgs),
    /// Integrity-check files: fully decode every record, report any corruption.
    Verify(VerifyArgs),
    /// Merge many `.cbor.zstd` files into one (CBOR -> CBOR, streaming).
    Merge(MergeArgs),
    /// Split one stream into per-group `.cbor.zstd` files (by day/session/model/path).
    Split(SplitArgs),
    /// Reconstruct conversation threads from request message histories (branches included).
    Thread(ThreadArgs),
    /// Show error/failure patterns: sparkline histogram by hour-of-day, status×model breakdown.
    Failures(FailuresArgs),
    /// Aggregate stats: tokens, cost, durations, by-model / by-path.
    Stats(StatsArgs),
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Info(mut a) => {
            a.files = expand(a.files)?;
            cmd_info(&a)
        }
        Cmd::Ls(mut a) => {
            a.files = expand(a.files)?;
            cmd_ls(&a)
        }
        Cmd::Extract(mut a) => {
            a.files = expand(a.files)?;
            cmd_extract(&a)
        }
        Cmd::Repack(a) => cmd_repack(&a),
        Cmd::Edit(mut a) => {
            a.files = expand(a.files)?;
            cmd_edit(&a)
        }
        Cmd::Grep(mut a) => {
            a.files = expand(a.files)?;
            cmd_grep(&a)
        }
        Cmd::Verify(mut a) => {
            a.files = expand(a.files)?;
            cmd_verify(&a)
        }
        Cmd::Merge(mut a) => {
            a.files = expand(a.files)?;
            cmd_merge(&a)
        }
        Cmd::Split(mut a) => {
            a.files = expand(a.files)?;
            cmd_split(&a)
        }
        Cmd::Stats(mut a) => {
            a.files = expand(a.files)?;
            cmd_stats(&a)
        }
        Cmd::Thread(mut a) => {
            a.files = expand(a.files)?;
            cmd_thread(&a)
        }
        Cmd::Failures(mut a) => {
            a.files = expand(a.files)?;
            cmd_failures(&a)
        }
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
                            .is_some_and(|s| s.ends_with(".cbor"))
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
