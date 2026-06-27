//! Shared helpers for the conversation-thread HTML renderers (`builtin`,
//! `theme`) and a couple of small string utilities reused across the crate.
//!
//! `best_record_id` / `last_record_id` / `model_of` / `sender_color` previously
//! lived as byte-identical private copies in both `builtin.rs` and `theme.rs`;
//! `escape_html` was duplicated in `builtin.rs` and `markdown.rs` (with a
//! subtly-divergent third copy in `theme.rs`); `truncate` was duplicated in
//! `builtin.rs` and `thread.rs`. They are collected here so the renderers and
//! the thread/preview paths share one implementation.

use serde_json::Value as Json;

/// Escape the HTML metacharacters `&`, `<`, `>`, and `"`.
///
/// Escaping `"` is harmless in HTML text content (it renders as a literal
/// quote) and required inside double-quoted attributes, so this single helper
/// is safe for both positions. `\n` and other control chars are left as-is —
/// the caller (or CSS `white-space: pre-wrap`) is responsible for line breaks.
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Truncate `s` to at most `max` characters, collapsing runs of whitespace
/// to a single space and appending an ellipsis if anything was dropped.
///
/// Char-aware (operates on `char`, not bytes), so it is safe on non-ASCII
/// content — unlike a naive `&s[..max]` slice, which panics if `max` lands
/// in the middle of a multibyte codepoint.
pub fn truncate(s: &str, max: usize) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i >= max {
            out.push('…');
            break;
        }
        if c.is_whitespace() {
            if !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Resolve the best record id for metadata anchoring. Prefers `intro_rid`
/// (distinct per node → correct per-turn timestamps), but falls back to the
/// last `record_id` if the intro record has a non-2xx HTTP status (avoids
/// poisoning from a single 404/500 that introduced a shared prefix node).
pub fn best_record_id(node: &Json, records: Option<&serde_json::Map<String, Json>>) -> Option<i64> {
    let intro = node.get("intro_rid").and_then(|v| v.as_i64());
    let last = last_record_id(node);
    let rmap = records?;
    let status_ok = |id: i64| {
        rmap.get(&id.to_string())
            .and_then(|r| r.get("status_code").and_then(|v| v.as_i64()))
            .map(|s| (200..300).contains(&s))
            .unwrap_or(true)
    };
    match intro {
        Some(id) if status_ok(id) => Some(id),
        _ => last,
    }
}

/// Resolve the last record_id for a node (the most recent record that
/// passed through this node).
pub fn last_record_id(node: &Json) -> Option<i64> {
    node.get("record_ids")
        .and_then(|v| v.as_array())
        .and_then(|ids| ids.last().and_then(|v| v.as_i64()))
}

/// The model name recorded for `rec`, if any.
pub fn model_of(rec: Option<&Json>) -> Option<String> {
    rec.and_then(|r| r.get("model"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Deterministic per-sender color from a fixed palette. Hashes the sender
/// name with blake3 so the same model always maps to the same color across
/// paths and across runs.
pub fn sender_color(sender: &str) -> &'static str {
    const PALETTE: &[&str] = &[
        "#0a84ff", "#bf5af2", "#ff375f", "#30d158", "#ff9f0a", "#64d2ff", "#5e5ce6", "#ac8e68",
    ];
    let h = blake3::hash(sender.as_bytes());
    let n = u64::from_le_bytes(h.as_bytes()[0..8].try_into().unwrap_or([0u8; 8]));
    PALETTE[(n as usize) % PALETTE.len()]
}

/// Resolve the tool-event record anchors for an assistant node, for renderers
/// that emit tool-call/result details.
///
/// Tool *calls* live in a record's **response** body. The record whose
/// response generated an assistant turn is the one whose **request** first
/// included that assistant message in its history — i.e. the node's
/// `intro_rid`. (The same record's response carries the call.) So the call
/// anchor is `node.intro_rid`, falling back to the last `record_id` only if
/// intro is absent.
///
/// Tool *results* live in the **next** request's echoed messages. The next
/// request is the one that introduces this node's first child (or, in a
/// linear chain, the next `record_id`). We prefer the first child's
/// `intro_rid` and fall back to the record after `intro_rid` in
/// `record_ids`.
///
/// Both anchors are looked up against the optional `records` metadata map
/// (keyed by record id as a string). Returns `(call_rec, result_rec)`.
///
/// This replaces the older `record_ids[depth]` / `record_ids[depth+1]`
/// positional scheme, which silently dropped calls whenever the assistant
/// node's `record_ids` didn't include the call-bearing record (common when
/// a branch or re-introduction narrows the record_ids list).
pub fn tool_event_records<'a>(
    node: &'a Json,
    records: Option<&'a serde_json::Map<String, Json>>,
) -> (Option<&'a Json>, Option<&'a Json>) {
    let lookup = |id: Option<i64>| id.and_then(|id| records.and_then(|m| m.get(&id.to_string())));
    let intro_rid = node.get("intro_rid").and_then(|v| v.as_i64());
    let call_rec = lookup(intro_rid.or_else(|| {
        node.get("record_ids")
            .and_then(|v| v.as_array())
            .and_then(|ids| ids.last().and_then(|v| v.as_i64()))
    }));
    // Result: prefer the first child's intro_rid (the next request), else
    // the record after intro_rid in this node's record_ids.
    let result_id = node
        .get("children")
        .and_then(|v| v.as_array())
        .and_then(|kids| kids.first())
        .and_then(|c| c.get("intro_rid"))
        .and_then(|v| v.as_i64())
        .or_else(|| {
            // Fall back: record_ids[index after intro_rid].
            let ids = node.get("record_ids").and_then(|v| v.as_array())?;
            let after = intro_rid
                .and_then(|rid| ids.iter().position(|v| v.as_i64() == Some(rid)))
                .map(|p| p + 1)
                .unwrap_or(1);
            ids.get(after).and_then(|v| v.as_i64())
        });
    let result_rec = lookup(result_id);
    (call_rec, result_rec)
}
