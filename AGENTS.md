# AGENTS.md

Guidance for coding agents working on czsplicer. Read this before editing.

## What this is

`czsplicer` is a Rust CLI for inspecting, extracting, editing/redacting, and
repacking `.cbor.zstd` log streams — specifically the concatenated
CBOR-over-zstd captures exported by Tailscale Aperture, but it works on any
such stream. Each file is a **concatenated stream of independent CBOR map
records** (one record per captured request/response), *not* a single CBOR
array. Keep that framing in mind: every read path is record-by-record.

## Build, test, lint

- Rust 1.80+, edition 2021. Single binary, no workspace.
- `cargo build` / `cargo build --release` (binary at `target/release/czsplicer`).
- `cargo test` — 175 passed, 1 ignored (2 suites; integration in `tests/integration.rs`,
  unit in `src/mailbox.rs` + `src/mermaid.rs` + `src/csv.rs`), all synthetic.
- `cargo fmt --check` is enforced. The pre-commit hook (`hooks/pre-commit`,
  enable with `git config core.hooksPath hooks`) runs `fmt --check` + `cargo
  test` when `.rs`/`.toml`/`tests/` files are staged.
- `prod/`, `target/`, `.maki/` are git-ignored. `prod/` holds real (large)
  export data and must never be committed.

Note: the README and architecture.md agree with the live `cargo test` count
(175 passed, 1 ignored, 2 suites). Keep them in sync when the count changes.

## Repository layout

```
src/
  main.rs      clap Cli/Cmd dispatch + expand() (dir -> sorted *.cbor.zstd)
  commands.rs  one *Args (clap) struct + cmd_* fn per subcommand. ~2/3 of code.
  filter.rs    Filter + FilterArgs, shared by all selection commands
  format.rs    CBOR<->JSON bridge, RecordStream, ZstdPacker, redact/search, field accessors
  thread.rs    conversation-thread reconstruction (trie over message-content hashes) + RecordMeta
  render.rs    shared helpers for the HTML renderers (escape_html, truncate, sender_color, best_record_id, ...) + clip_chars
  markdown.rs  minimal safe Markdown->HTML subset for the built-in renderer
  mermaid.rs   Mermaid diagram emitters (pie/xychart/timeline) for `stats --format mermaid` / `failures --format mermaid`
  md_thread.rs Markdown thread renderer (`thread --format md`) — linear path flattening, mirrors builtin.rs
  builtin.rs   built-in long-form HTML renderer (wide column, markdown, status/tool chips)
  theme.rs     Adium .AdiumMessageStyle loader + renderer (--theme, optional)
  mailbox.rs   mbox/Maildir export (RFC822 + threading via Message-ID/In-Reply-To)
  builtin.css  stylesheet for builtin.rs (embedded via include_str!)
tests/
  integration.rs   end-to-end tests via assert_cmd
  common/mod.rs    Fixture builder (SOURCE_NDJSON / RICH_NDJSON truth sets)
  fixtures/Spike.AdiumMessageStyle/   MIT test-fixture Adium theme
vendor/             highlight.js (BSD-3-Clause) + CSS themes, embedded via include_str!
```

No `mod.rs` under `src/`; `main.rs` declares
`mod builtin; mod commands; mod csv; mod filter; mod format; mod mailbox; mod markdown; mod md_thread; mod mermaid; mod render; mod theme; mod thread;`.

## Architecture & data flow

Every selection command (`ls`, `extract`, `grep`, `edit`, `stats`, `merge`,
`split`, `thread`) follows the same shape:

1. `expand()` (main.rs) turns directory args into their sorted `*.cbor.zstd`
   contents.
2. `FilterArgs::build()` (filter.rs) compiles CLI flags into a `Filter`.
3. `RecordStream::open(path)` (format.rs) decodes zstd once and yields CBOR
   records lazily via `ciborium`'s streaming decoder.
4. Per record: `Filter::matches(rec)` gates the work, then the command-specific
   transform runs.
5. Output is written streaming (NDJSON / re-compressed CBOR).

`verify` and `repack` don't filter; `info` summarizes without streaming
transforms. `thread` builds an in-memory trie over the whole filtered set (one
synthetic Node per distinct message — see invariant 5).

The clap `*Args` structs live in `commands.rs` and are passed directly into
`cmd_*` from `main.rs` — there is no separate parallel struct layer.

## Critical invariants (easy to break, not obvious)

### 1. CBOR bytes vs text — the JSON bridge sentinels
`capture.rawRequestBody` / `rawResponseBody` are CBOR **bytes** (raw HTTP
bodies); the parallel `requestBody`/`responseBody` are text. `cbor_to_json` /
`json_to_cbor` (format.rs) preserve the distinction via two sentinels:
- `BYTES_KEY = "__cbor_bytes_b64"` → bytes encode as `{"__cbor_bytes_b64": "<b64>"}`
- `TAG_KEY = "__cbor_tag"` → CBOR tags encode as `{"__cbor_tag": [<u64>, <value>]}`

Do not collapse bytes into strings or you'll corrupt the round-trip and silently
change record types on repack.

### 2. Redaction vs search deliberately disagree on invalid-UTF-8 bytes
`format::redact_strings` (used by `edit --redact`) and
`format::search_value_strings` (used by `grep`) both visit `Text` **and**
`Bytes`, but handle invalid-UTF-8 bytes differently:

- `search_value_strings` decodes bytes **lossily** (`String::from_utf8_lossy`).
- `redact_strings` scrubs bytes **only when valid UTF-8** (`std::str::from_utf8`);
  invalid bytes are left byte-for-byte intact.

**Consequence:** grep can surface an ASCII secret embedded in a partially-invalid
byte body that `edit --redact` will NOT scrub. This is a deliberate, documented
tradeoff (see the doc comment on `redact_strings`): scrubbing a lossy decode
would rewrite the original bytes and corrupt binary payloads. Do not "fix" this
gap by redacting the lossy decode — it would break binary preservation. For
well-formed text bodies (the realistic case) the two agree exactly.

Raw HTTP bodies live in `capture.rawRequestBody` / `capture.rawResponseBody`.
`edit --redact` targets `capture` by default; `--all-strings` widens to the whole
record.

### 3. Streaming is load-bearing
A single 3.3 MB export decompresses to ~1.1 GB / 2000 records. All read paths
stream record-by-record. The **only** exception is `extract --array`, which
buffers the full result — keep it that way (a single, documented exception).
Don't introduce new full-file buffering.

### 4. Float precision
Lossless floats depend on `serde_json`'s `float_roundtrip` feature (enabled in
`Cargo.toml`) so `estimated_cost.dollars` survives the round-trip unchanged.
The one theoretical hole: CBOR negative integers below `i64::MIN` (down to
−2⁶⁴) fall back to f64 in `cbor_to_json` — never occurs in capture records.

### 5. Conversation-thread reconstruction (thread.rs)
The `thread` command reconstructs conversation branches from request message
histories. Each record's `capture.requestBody.messages` echoes its **full parent
path**, so the tree is a **trie over blake3 content-hashes of normalized
messages** — NOT grouped by `session_id` (in Aperture captures every request
gets a unique session_id; session_id is just metadata). A depth-0 node (usually
the system prompt) is a conversation root; a node with >1 child is a branch
point (the user went back and took a different path).

Content normalization is load-bearing: a bare-string message content and the
equivalent `[{"type":"text","text":s}]` block form MUST hash identically, or a
string-user-message request and its block-form continuation look like two
separate roots. `msg_info` normalizes strings to block form before hashing.
Assistant turns are reconstructed from the **next** request's echoed messages —
no response-body parsing is needed for structure. Verified on real prod data:
30/100 files contain genuine branches (fan-outs up to 19, depths up to 748).

## Testing conventions

- Tests are end-to-end via `assert_cmd`, driving the compiled binary.
- `Fixture::new()` (tests/common/mod.rs) writes `SOURCE_NDJSON`, then **builds
  the `.cbor.zstd` fixture via the tool's own `repack`** — this exercises the
  full CBOR↔JSON bridge and is intentional. A separate `RICH_NDJSON`
  (5 records) covers merge/split/session grouping.
- `read_ndjson` parses with serde_json (order-preserving); prefer semantic
  equality (`serde_json::Value` compare) over substring matching where possible.
- **Bytes-body redaction tests must base64-decode the body** and assert on the
  decoded bytes. A secret riding inside `__cbor_bytes_b64` is never plain text
  in extracted JSON, so `!out.contains(secret)` passes even on unfixed code
  (see `edit_redact_scrubs_byte_bodies`). Its counterpart
  `edit_redact_leaves_binary_bytes_untouched` pins the binary-preservation
  contract and was verified to fail under a lossy-decode-rewrite regression.

## Style notes

- `anyhow::Result` everywhere; add `context`/descriptive errors on I/O paths.
- Command output: NDJSON by default (streaming), `--json` for structured,
  `--array`/`--pretty` where a single buffered object is acceptable.
- zstd write level defaults to 9; `--level` exposes it.

## Commit conventions

- Every commit message ends with a `Co-Authored-By:` trailer crediting the
  AI model that authored the change. Format:
  ```
  Co-Authored-By: <Model Name> <<model-handle>@<domain>>
  ```
  e.g. `Co-Authored-By: GLM-5.2 <glm-5.2@z.ai>`. Fill in the actual model
  name and handle for the model you are running as; do not leave a placeholder.
  Append (don't replace) when a commit is co-authored by multiple models.
