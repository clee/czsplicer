# czsplicer

A CLI for inspecting, extracting, editing/redacting, and repacking `.cbor.zstd`
log streams — designed for the capture-log exports produced by
[Tailscale Aperture](https://tailscale.com/blog/aperture), but works on any
concatenated CBOR-over-zstd data.

Aperture exports API-traffic captures as zstd-compressed [CBOR](https://cbor.io/)
files. Each file is a **concatenated stream of independent CBOR map records**
(one record per captured request/response), not a single CBOR array. This tool
streams those records and gives you fast, lossless round-trips between CBOR and
JSON, plus a set of investigative and redaction commands.

> Not affiliated with Tailscale. "Aperture" is a trademark of Tailscale; this
> project references it only for descriptive accuracy.

## Install

```sh
cargo install --path .
# or
cargo build --release   # binary at target/release/czsplicer
```

Requires Rust 1.80+.

## Commands

```
czsplicer <COMMAND>

Commands:
  info     Show per-file summary: record counts, sizes, id/timestamp ranges, schema
  ls       List records as a table (or NDJSON with --json)
  extract  Extract records to JSON (NDJSON by default, or a JSON array)
  repack   Re-encode JSON (NDJSON or array) back to a .cbor.zstd file
  edit     Transform records in a single pass: redact secrets, strip fields, drop/select
  grep     Search records for a regex pattern in their string/bytes values
  verify   Integrity-check files: fully decode every record, report any corruption
  merge    Merge many `.cbor.zstd` files into one (CBOR -> CBOR, streaming)
  split    Split one stream into per-group `.cbor.zstd` files (by day/session/model/path)
  stats    Aggregate stats: tokens, cost, durations, by-model / by-path
  thread   Reconstruct conversation threads (branching included); export as JSON/HTML/MBOX/Maildir
  failures  Error/failure analysis: sparkline histogram by hour-of-day, status×model breakdown
```

Run `czsplicer <command> --help` for full flags.

### Inspecting

```sh
# Summary of every export
czsplicer info prod/

# List records as a table
czsplicer ls prod/

# Find records whose bodies match a regex
czsplicer grep -i 'rate.?limit' prod/ --show-matches
czsplicer grep 'claude-' prod/ --field capture.responseBody --count

# Aggregate tokens / cost / latency
czsplicer stats prod/ --by model
czsplicer stats prod/ --by provider
czsplicer stats prod/ --by status
```

### Extracting & repacking (the round-trip)

`extract` produces NDJSON by default (streaming, low memory); `repack` turns
NDJSON or a JSON array back into `.cbor.zstd`. The round-trip is **lossless** for
the data these logs contain: CBOR `bytes` bodies are carried through JSON as
`{"__cbor_bytes_b64":"…"}`, and float precision is preserved. (The one
theoretical exception is a CBOR negative integer below `i64::MIN` — down to
−2⁶⁴ — which JSON has no native way to represent and falls back to f64; this
never occurs in capture-log records.)

```sh
# Full records to NDJSON (one JSON object per line)
czsplicer extract prod/ > all.ndjson

# Project only a few fields
czsplicer extract prod/ --fields id,model,usage.input_tokens,estimated_cost.dollars

# Dump request/response bodies to files
czsplicer extract prod/ --bodies ./bodies   # ./bodies/<id>.request, <id>.response

# Edit the JSON with jq/any editor, then repack
jq 'select(.status_code == 200)' all.ndjson | czsplicer repack - -o filtered.cbor.zstd
```

### Redacting

```sh
# Canned presets for common secret shapes (email, jwt, apikey, bearer, aws,
# ipv4, uuid, creditcard, ssn) — use `all` for everything
czsplicer edit prod/1782318290.cbor.zstd -o safe.cbor.zstd \
  --redact-preset all --strip-headers

# Custom regexes (repeatable), with a custom replacement token
czsplicer edit prod/1782318290.cbor.zstd -o safe.cbor.zstd \
  --redact '(?i)bearerr?\s+[A-Za-z0-9._-]+' \
  --redact 'internal-project-name' \
  --redact-replacement '***'

# Emit NDJSON instead of recompressing
czsplicer edit prod/ -o - --json --redact-preset email
```

### Splitting & merging

```sh
# Merge all exports into one file (CBOR->CBOR, no intermediate)
czsplicer merge prod/ -o all.cbor.zstd

# Split into per-day files
czsplicer split all.cbor.zstd --by day --out-dir days/

# Split into per-session files (session_id is auto-populated by Aperture;
# --min-records defaults to 2 to skip single-request throwaways)
czsplicer split all.cbor.zstd --by session --out-dir sessions/

# Also: --by model, --by provider, --by path. Use --json for a manifest of the output files.
```

### Threading

Reconstruct conversation branches from each request's echoed message history,
then render or export. Branch points (where the user went back and took a
different path) are recovered automatically; the trie keys on normalized
message-content hashes, so a string user message and its block-form
continuation collapse to one node.

```sh
# Default: JSON forest (roots, nodes, record_ids, tool_events) to stdout.
czsplicer thread prod/

# Self-contained long-form HTML (one file, no external theme).
czsplicer thread prod/ --format html --dark -o threads.html

# Render through an Adium .AdiumMessageStyle bundle (optional, --variant Dark).
czsplicer thread prod/ --theme Spike.AdiumMessageStyle -o threads.html

# Export as mbox (threaded by Message-ID / In-Reply-To) for a mail client.
czsplicer thread prod/ --format mbox -o threads.mbox

# Maildir (one file per message) with plain-text bodies instead of HTML.
czsplicer thread prod/ --format maildir --body plain -o maildir/

# Redact secrets in the rendered output (same presets as `edit`).
czsplicer thread prod/ --format html --redact-preset all -o threads.html
```

Formats: `json` (default), `html` (built-in), `mbox`, `maildir`. `--body`
controls mbox/maildir body rendering: `plain`, `html` (multipart/alternative,
default), `html-only`. Redaction runs on message bodies and tool text *before*
rendering, so secrets never reach the output file.

### Failure analysis

See when errors happen and which models are responsible. The default view shows a
sparkline histogram of non-2xx status codes by hour-of-day, plus a per-model
breakdown — so you can spot bursty provider incidents at a glance.

```sh
# Sparkline histogram + status×model breakdown (non-2xx only).
czsplicer failures prod/

# Include 2xx for baseline contrast (error rate vs. success).
czsplicer failures prod/ --all

# Focus on one status code (range shorthand works too: --status 5xx).
czsplicer failures prod/ --status 503

# Structured output for dashboards.
czsplicer failures prod/ --json
```

### Integrity check

```sh
czsplicer verify prod/          # ok / FAIL per file, exits 1 on corruption
czsplicer verify prod/ --json
```

## Filtering

Every selection command (`ls`, `extract`, `grep`, `edit`, `stats`, `merge`,
`split`, `thread`) shares the same filter flags:

| Flag | Matches |
|------|---------|
| `--id 5` / `--id 5-10` | record id or inclusive range (repeatable) |
| `--model NAME` | exact model (repeatable) |
| `--provider NAME` | model prefix before `/` (repeatable) |
| `--path PATH` | exact path (repeatable) |
| `--status CODE` | HTTP status code (repeatable) |
| `--api-type TYPE` | api_type (repeatable) |
| `--login-name NAME` | identity.login_name (repeatable) |
| `--client PREFIX` | User-Agent prefix, case-insensitive (repeatable) |
| `--since TIME` | `>=` this ISO-8601 time (prefix compare) |
| `--until TIME` | `<=` this time (bare date = inclusive whole day) |
| `--date YYYY-MM-DD` | exact calendar day |
| `--invert` | drop matching records instead of keeping them |

Directory arguments are expanded to their sorted `*.cbor.zstd` contents, so
`czsplicer ls prod/` works the same as `czsplicer ls prod/*.cbor.zstd`.

## Format notes

- **CBOR bytes vs text.** `capture.rawRequestBody` / `rawResponseBody` are stored
  as CBOR bytes (raw HTTP bodies); the parallel `requestBody`/`responseBody` are
  text. The JSON bridge preserves the distinction via the `__cbor_bytes_b64`
  sentinel so repacking reconstructs bytes, not strings.
- **Float precision.** Depends on `serde_json`'s `float_roundtrip` feature
  (enabled) so `estimated_cost.dollars` values survive unchanged.
- **Streaming.** Files can be large (a single 3.3 MB export decompresses to
  ~1.1 GB / 2000 records). All read paths stream record-by-record; only
  `extract --array` buffers the full result in memory.
- **Compression.** Write paths default to zstd level 9 (a good speed/ratio
  balance). Use `--level 19` for archival compression (~2x smaller, ~10x slower),
  or lower levels for faster output.

## Development

```sh
cargo test                       # 158 tests (146 integration + 12 unit, synthetic fixtures)
```

The repository includes a pre-commit hook (`hooks/pre-commit`) that runs
`cargo fmt --check` and `cargo test`. Enable it with:

```sh
git config core.hooksPath hooks
```

`prod/` (real export data), `target/`, and `.maki/` are git-ignored.
