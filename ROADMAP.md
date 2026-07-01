# czsplicer roadmap

Status: brainstorm complete (2026-07-01). This document records the decisions
taken and the phased plan for implementation. Items below this line are
**decisions** unless marked *[open]*.

## Goals & framing

- **Audience:** open-source release for Tailscale Aperture users running
  czsplicer against their own `.cbor.zstd` exports.
- **Theme of this roadmap:** **output & consumption.** The input/decoding side
  (CBOR bridge, streaming, redaction, thread reconstruction) is mature and
  load-bearing; the leverage now is in what artifacts czsplicer *produces* and
  how a non-engineer reads them.
- **Anti-goal:** a JavaScript dashboard. Charts are Mermaid (text, GitHub-native)
  or CSV (let the user chart it). No chart.js, no build pipeline, no frontend
  maintenance burden.

## The two graph problems (kept distinct)

1. **Structure graphs** — the conversation trie (branches, turns, tool calls).
   Rendered as **flattened root-to-leaf path prose** (the only scheme that
   survives depth-749 trees — see the resolved item below). Mermaid
   `flowchart`/`sequenceDiagram` renderings of structure are deferred (they cap
   out well below real conversation depth).
2. **Analysis graphs** — aggregates over records (cost/day, tokens by model,
   status by provider, error rate by hour). Computed by `stats`/`failures`,
   rendered by Mermaid pie/xychart/timeline + emitted as CSV.

Both graph problems live in **one unified export** (the `report` command),
interleaved with prose. There is no structural-vs-content split — diagrams and
content share the same Markdown document.

## Decisions

### Format family

- **Markdown** with embedded **Mermaid** blocks is the primary prose+diagram
  format. GitHub renders the whole thing natively.
- **CSV/TSV** is added for the spreadsheet / own-tool audience. Tabular
  commands (`stats`, `failures`, `ls`) gain `--format csv`. JSON stays; CSV is a
  new sibling, not a replacement.
- **Mermaid diagram types:**
  - Analysis (v0.4): `pie` (model / status share), `xychart-beta` (cost or
    tokens over time), `timeline` (incident bursts).
  - Structure (deferred to Phase 4): `flowchart` (conversation tree outline
    with branch points), `sequenceDiagram` (turn-by-turn
    request→response→tool). Both cap out well below real conversation depth, so
    v0.4 renders structure as flattened prose instead.
  - *Not* doing Mermaid `gitGraph` for v1 — see "deferred."
- **Aggregation policy for scale:** top-N + "other" collapse for pies; per-day
  or per-hour bucketing for xychart. Naive full-resolution Mermaid chokes past
  ~50 elements; the aggregation code is the real work, not the string emission.

### `report` — the centerpiece

- New subcommand: `czsplicer report <files> -o report.md` (and `report.html`
  as a stretch goal).
- **Single self-contained Markdown document:**
  ```
  # Aperture export report
  ## Summary          — counts, total $ cost, date range, top models
  ## Usage            — Mermaid pie (model share) + xychart (cost/day) + CSV block
  ## Failures         — Mermaid timeline (incident bursts) + status breakdown
  ## Conversations
    ### <thread title>
      <flattened root-to-leaf paths; branch points noted inline>
      <full Markdown prose of the turns, tool calls fenced>
  ```
- The underlying Mermaid emitters, Markdown thread renderer, and CSV emitter
  are individually reusable (`thread --format md`, `stats --format mermaid`,
  `stats --format csv`). `report` composes them.

### Markdown thread renderer

- Headings per turn; fenced code blocks for tool-call bodies.
- Reuses the existing trie walker (`thread::all_paths`); conceptually the
  reverse flavor of the existing `markdown.rs` md→HTML parser.
- *[resolved 2026-07-01]* **Linear root-to-leaf path flattening.** Real
  `days/` data contains depth-749 trees and 20-way branches; Markdown headings
  cap at 6 and nested lists get unreadable past ~6, so headings-per-turn /
  nested-section / anchor schemes are impossible. Each root-to-leaf path is one
  section; branch points are noted inline and shared prefixes are not
  re-rendered. Mirrors `builtin.rs`.

### Secrets-safety net

- When emitting any human-readable output (HTML/Markdown/mbox/EPUB) **without**
  `--redact*`, run the canned preset regexes (`bearer`, `apikey`, `jwt`, …) as
  a **detector** over the to-be-written bytes.
- On a hit: print a stderr warning naming the offending field/record pointer,
  quoting the matched pattern (not the secret), and pointing at
  `--redact-preset all`. Opt out with `--i-know`.
- **Detection only — never mutates output.** Reuses `compile_redact_regexes` +
  the preset table.
- Warning wording must make clear it's a **best-effort heuristic**, not a
  guarantee — custom token shapes (`x-ts-internal-…`) will not be caught. The
  goal is to prevent the embarrassing day-one leak, not promise safety.

### EPUB + Kindle (azw3) output — later release

Deliverable is **both EPUB and Kindle**, not EPUB alone. Native AZW3 is realistic
because the proprietary-compression half is already built in `~/src/huffcomp`.

**Three separable stages:**

1. **Author content (XHTML/NCX/opf).** Needed for EPUB regardless; the EPUB
   *is* structured XHTML. This stage is shared work — no Kindle-specific
   decision here.
2. **Wrap into a MOBI/AZW3 skeleton** — PDB header + section table + MOBI record
   header + EXTH metadata + (uncompressed or PalmDOC) text records. Fully
   specified, format-stable since the Kindle Keyboard era. Two sub-options:
   - **2a.** Shell out to the vendored `kindlegen` (already in huffcomp/, 27 MB,
     proven on the Steelheart/Thunderball/Dead Beat corpus) or Calibre's
     `ebook-convert`. Fast to ship; adds a binary dependency.
   - **2b.** Native minimal MOBI/AZW3 *author* in Rust (uncompressed text records
     are trivial; headers are fiddly but documented). Removes the external
     binary. *[open]* whether to do 2a first then graduate to 2b, or write 2b
     natively from the start.
3. **HUFF/CDIC recompression** — vendor or path-invoke `~/src/huffcomp`. This is
   **the moat**: it makes czsplicer's Kindle output genuinely Kindle-native and
   smaller than `ebook-convert` or kindlegen's own compression. The hard
   proprietary work (package-merge length-limited Huffman + BPE phrase
   dictionary + canonical-code/CDIC record emission) is already done in pure
   `std` Rust, zero deps, edition 2024 — trivial to vendor.

**Kindle-renderer constraints (affect EPUB design regardless of container):**

- No JavaScript → **no Mermaid inside the book.** Stats "charts" must be static
  SVG/PNG (pre-rendered) or simple tables. This is a real divergence from the
  Markdown report.
- Kindle handles `<pre>` poorly past a few hundred lines → tool-call bodies must
  be collapsed/condensed.
- Long captures (thousands of turns) are unreadable on e-ink → needs a
  per-conversation length cap or a "highlights only" mode.

### Deferred (not v1, not forgotten)

- **Interactive HTML viewer** (sidebar + collapsible branches + in-page search,
  dedicated tree-outline pane + linear read pane). Single self-contained
  `index.html`. Will consume the same trie/stats JSON the `report` composer
  produces. Stage for the release after the Markdown/Mermaid report ships.
- **Git fast-import export** — one commit per turn, branches at branch points,
  navigable with `git log --graph` / `tig` / GitHub. The genuinely novel
  "weird" idea; uses branchiness as an asset. Later.
- **Mermaid `gitGraph`** — appealing branch metaphor, deferred with the git
  export since they share a release theme.
- **Graphviz DOT** — cheap add-on for huge structures; bundle when convenient.
- **Adium theme polish** — niche retro renderer; don't over-invest.

## Phased plan

### Phase 1 — polish for release (fix first, then keep updated as we go)

1. **README + first-run correctness.** Fix the test-count drift: the README
   says 65, AGENTS.md says 83, architecture.md says 134 — the real number is
   **144 passed, 1 ignored, 2 suites** (recompute from `cargo test` before each
   edit). Drop the dead `cargo test -- --ignored` real-data round-trip
   reference (those tests no longer exist). Add a 5-line "how to look at an
   export" block to bare `czsplicer` output. Clarify `thread --html` directory
   behavior. **Convention going forward: any PR that changes the command
   surface or test count updates the README in the same change.**

### Phase 2 — foundations (ship as standalone flags)

Each piece is individually useful and unblocks Phase 3.

2. **Mermaid emitters** — `pie`, `xychart-beta`, `timeline`, `flowchart`,
   `sequenceDiagram`. Pure string emission over buckets `stats`/`failures`
   already compute. Add top-N/bucketing policy. Expose via
   `stats --format mermaid`, `failures --format mermaid`.
3. **Markdown thread renderer** — `thread --format md`. Trie walker →
   **linear root-to-leaf path flattening** (the only scheme surviving
   depth-749 trees; nested headings/lists cap at 6). Each path is one section;
   branch points noted inline. User turns as blockquotes, tool calls fenced.
4. **CSV emitter** — `stats --format csv`, `failures --format csv`, `ls --csv`.
   Tiny; tabular sibling to `--json`.
5. **Secrets-safety net** — detector on all human-readable output paths.

### Phase 3 — the centerpiece

6. **`report` command** — composes summary + stats-mermaid + failures-mermaid +
   per-thread flattened prose into one `.md`. This is the OSS-launch headline
   artifact.
7. **HTML report (stretch)** — same composition, rendered: vendored mermaid.js +
   existing builtin thread HTML. Opt-in, not the default.

### Phase 4 — the weird/novel formats (post-launch)

8. **EPUB + Kindle (azw3)** — three-stage pipeline above. EPUB native,
   container via 2a (then 2b), HUFF/CDIC via huffcomp.
9. **Interactive HTML viewer** — the deferred consumption centerpiece.
10. **Git fast-import export + Mermaid gitGraph** — the branch-as-asset theme.

## Open questions to resolve before each phase

- *Phase 2, item 3:* **RESOLVED 2026-07-01 against real `days/` data.** A
  single day (2026-06-21) contains a tree of depth **749** (752 nodes) and
  20/21 trees have branches. Markdown headings cap at 6 levels and indented
  lists get unreadable past ~6, so **headings-per-turn / nested-list schemes
  are impossible.** The renderer mirrors `builtin.rs`: each root-to-leaf path
  (`thread::all_paths`) becomes one linear section. Branch points are marked
  inline and cross-referenced via anchors ("→ continues from path N at turn
  M"); shared prefixes are noted, not re-rendered.
- *Phase 2, item 2:* **RESOLVED 2026-07-01 against real `days/` data
  (17,484 records, $1,199, 25 models, ~110-day span):**
  - **Top-N = 8 + "other"** for model aggregation (top-8 ≈ 95% of cost; the
    natural knee). Applied wherever model cardinality would overflow a legend.
  - **Per-day bucketing** for the time/xychart axis (~110 points — scannable;
    per-hour is unreadable at this span, per-week loses the late-June burst).
  - **No collapsing** for path (6 distinct) or status code (9 distinct) —
    render in full.
  - **Cost curves are extremely spiky** (06-20 = $367.96, typical day < $30,
    several < $1). A linear-scale xychart flattens all but the peak. Default
    the cost/time chart to a **log y-axis** when max/min ratio > ~20×, else
    linear; or pair the absolute curve with a small "daily $ table" so the
    quiet days aren't lost. Decide at emitter time.
  - Re-check these thresholds if a future export is an order of magnitude
    larger; they are tuned to this corpus.
- *Phase 3:* whether `report --html` (item 7) is in the launch release or slips
  to Phase 4.
- *Phase 4, item 8:* container authoring strategy (2a shell-out vs 2b native),
  decided when EPUB work starts; likely 2a-first given the vendored kindlegen.

## Non-goals (explicitly out of scope)

- A JavaScript chart dashboard or any frontend build pipeline.
- Native azw3/KFX authoring from scratch where huffcomp already covers the hard
  part — no point duplicating HUFF/CDIC.
- DRM handling (Kindle DRM, KFX) — out of scope; this tool processes the user's
  own exports.
- TUI client — the interactive HTML viewer reaches more users for less upkeep.
- New input-side features (streaming `watch` modes, non-Aperture ingest,
  library/API extraction) — input/decode side is mature; this roadmap is
  output-only.
