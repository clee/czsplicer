//! Markdown thread renderer (`thread --format md`).
//!
//! Mirrors `builtin.rs`'s linear path-flattening approach. Each root-to-leaf
//! path (`thread::all_paths`) becomes one section; branch points are marked
//! inline and shared prefixes are noted rather than re-rendered. This is the
//! only representation that survives arbitrary-depth trees — real captures
//! contain depth-749 conversations and 20-way branches, far beyond Markdown's
//! 6-level heading/list cap.

use crate::render::{best_record_id, model_of, tool_event_records};
use crate::thread::all_paths;
use serde_json::Value as Json;

/// Render the full thread forest as a single Markdown document.
pub fn render_md(forest: &Json) -> String {
    let mut out = String::new();
    out.push_str("# Conversation threads\n\n");
    out.push_str(&summary_block(forest));
    out.push('\n');
    out.push_str(&render_conversations(forest));
    out
}

/// Render just the per-conversation sections (no top-level title/summary).
/// Used by `report` to embed conversations under its own `## Conversations`
/// heading without duplicating the document title.
pub fn render_conversations(forest: &Json) -> String {
    let mut out = String::new();
    let records = forest.get("records").and_then(|r| r.as_object());
    let paths = all_paths(forest);

    // Group paths by their root node so each conversation tree is one section.
    // `all_paths` walks trees in `forest["trees"]` order, so paths sharing a
    // root are contiguous; group them by the first node's hash.
    let mut path_idx = 0usize;
    for group in group_paths_by_root(&paths) {
        out.push_str(&render_tree_section(&group, records, path_idx));
        path_idx += group.len();
        out.push('\n');
    }
    out
}

fn summary_block(forest: &Json) -> String {
    let total = forest
        .get("records_total")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let with = forest
        .get("records_with_messages")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let roots = forest
        .get("root_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let branches = forest
        .get("branch_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    format!(
        "> **{total} records** ({with} with messages) → **{roots} thread(s)**, **{branches} branch point(s)**.\n\n"
    )
}

/// Group contiguous paths sharing the same root node hash. Each group is one
/// conversation tree (its paths are the distinct root-to-leaf walks).
fn group_paths_by_root<'a>(paths: &[Vec<&'a Json>]) -> Vec<Vec<Vec<&'a Json>>> {
    let mut groups: Vec<Vec<Vec<&'a Json>>> = Vec::new();
    for path in paths {
        let key = path
            .first()
            .and_then(|n| n.get("hash"))
            .and_then(|h| h.as_str())
            .unwrap_or("");
        match groups.last_mut() {
            Some(g) if !g.is_empty() => {
                let prev_key = g[0]
                    .first()
                    .and_then(|n| n.get("hash"))
                    .and_then(|h| h.as_str())
                    .unwrap_or("");
                if prev_key == key {
                    g.push(path.clone());
                    continue;
                }
            }
            _ => {}
        }
        groups.push(vec![path.clone()]);
    }
    groups
}

fn render_tree_section(
    group: &[Vec<&Json>],
    records: Option<&serde_json::Map<String, Json>>,
    start_idx: usize,
) -> String {
    let mut out = String::new();
    let root = match group.first().and_then(|p| p.first()) {
        Some(r) => r,
        None => return out,
    };

    let label = root_label(root);
    let npaths = group.len();
    out.push_str(&format!("## {label}\n\n"));
    out.push_str(&format!(
        "_depth {max_depth}, {npaths} path(s)_\n\n",
        max_depth = max_depth_of(group),
    ));

    for (i, path) in group.iter().enumerate() {
        out.push_str(&render_path(path, records, start_idx + i + 1));
        out.push('\n');
    }
    out
}

fn max_depth_of(group: &[Vec<&Json>]) -> usize {
    group.iter().map(|p| p.len()).max().unwrap_or(0)
}

/// Derive a readable section label from the root node (its preview).
fn root_label(root: &Json) -> String {
    let preview = root.get("preview").and_then(|v| v.as_str()).unwrap_or("");
    let single = preview.lines().next().unwrap_or("").trim();
    if single.is_empty() {
        "(empty conversation)".into()
    } else if single.chars().count() > 80 {
        format!("{}…", single.chars().take(79).collect::<String>())
    } else {
        single.to_string()
    }
}

fn render_path(
    path: &[&Json],
    records: Option<&serde_json::Map<String, Json>>,
    path_num: usize,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("### Path {path_num}\n\n"));

    for (i, node) in path.iter().enumerate() {
        let role = node.get("role").and_then(|v| v.as_str()).unwrap_or("");

        // Metadata anchor: record whose request first included this node's
        // message. Mirrors builtin.rs (intro_rid of the next node, with a
        // best_record_id fallback for the terminal node).
        let meta_rid = if i + 1 < path.len() {
            best_record_id(path[i + 1], records)
        } else {
            best_record_id(node, records)
        };
        let meta_rec = meta_rid.and_then(|id| records.and_then(|rmap| rmap.get(&id.to_string())));

        let (label, is_user) = match role {
            "user" => ("you".to_string(), true),
            "assistant" => (
                model_of(meta_rec).unwrap_or_else(|| "assistant".into()),
                false,
            ),
            "system" => ("system".to_string(), false),
            other => (other.to_string(), false),
        };

        let body = node
            .get("content")
            .and_then(|v| v.as_str())
            .or_else(|| node.get("preview").and_then(|v| v.as_str()))
            .unwrap_or("");

        let time = meta_rec
            .and_then(|r| r.get("timestamp"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let status = meta_rec
            .and_then(|r| r.get("status_code"))
            .and_then(|v| v.as_u64());

        out.push_str(&format!("**{label}**"));
        if !time.is_empty() {
            out.push_str(&format!("  ·  _{time}_"));
        }
        if let Some(code) = status {
            out.push_str(&format!("  ·  `{code}`"));
        }
        out.push_str("\n\n");

        // Body: user messages as blockquotes (preserve verbatim); system /
        // assistant as markdown body (so rendered prose). The node content is
        // already markdown for assistant turns.
        if !body.is_empty() {
            if is_user {
                for line in body.lines() {
                    out.push_str("> ");
                    out.push_str(line);
                    out.push('\n');
                }
                out.push('\n');
            } else {
                out.push_str(body);
                if !body.ends_with('\n') {
                    out.push('\n');
                }
                out.push('\n');
            }
        }

        // Tool calls / results for assistant turns.
        if role == "assistant" {
            let (call_rec, result_rec) = tool_event_records(node, records);
            let tools = tool_events_md(call_rec, result_rec);
            if !tools.is_empty() {
                out.push_str(&tools);
                out.push('\n');
            }
        }

        out.push_str("---\n\n");
    }
    out
}

/// A Markdown code fence long enough that no run of backticks in `content`
/// can close it: one backtick longer than the longest backtick run (minimum 3,
/// the shortest legal fence). Per CommonMark a closing fence must be at least as
/// long as the opening fence, so a backtick run in the payload can't prematurely
/// end a block fenced with one more.
fn fence(content: &str) -> String {
    let mut longest = 0usize;
    let mut run = 0usize;
    for b in content.bytes() {
        if b == b'`' {
            run += 1;
            if run > longest {
                longest = run;
            }
        } else {
            run = 0;
        }
    }
    "`".repeat((longest + 1).max(3))
}

/// Render tool events as fenced Markdown code blocks. Calls carry the tool
/// name (language hint) and input; results follow. Mirrors the HTML rendering
/// order: call-then-result.
fn tool_events_md(call_rec: Option<&Json>, result_rec: Option<&Json>) -> String {
    let mut out = String::new();

    if let Some(events) = call_rec
        .and_then(|r| r.get("tool_events"))
        .and_then(|v| v.as_array())
    {
        for ev in events {
            if ev.get("kind").and_then(|v| v.as_str()) == Some("call") {
                let name = ev.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
                let input = ev.get("input").and_then(|v| v.as_str()).unwrap_or("");
                out.push_str(&format!("**tool call: `{name}`**\n\n"));
                let f = fence(input);
                out.push_str(&format!("{f}json\n"));
                out.push_str(input);
                if !input.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&format!("{f}\n\n"));
            }
        }
    }

    if let Some(events) = result_rec
        .and_then(|r| r.get("tool_events"))
        .and_then(|v| v.as_array())
    {
        for ev in events {
            if ev.get("kind").and_then(|v| v.as_str()) == Some("result") {
                let content = ev.get("content").and_then(|v| v.as_str()).unwrap_or("");
                if !content.is_empty() {
                    out.push_str("**tool result**\n\n");
                    let f = fence(content);
                    out.push_str(&format!("{f}\n"));
                    out.push_str(content);
                    if !content.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str(&format!("{f}\n\n"));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fence_is_at_least_three_backticks() {
        assert_eq!(fence(""), "```");
        assert_eq!(fence("no backticks here"), "```");
        // Runs shorter than 3 still floor to a 3-backtick fence.
        assert_eq!(fence("a ` b"), "```");
        assert_eq!(fence("a `` b"), "```");
    }

    #[test]
    fn fence_one_longer_than_longest_run() {
        // A 3-backtick run in the payload needs a 4-backtick fence to stay open.
        assert_eq!(fence("```"), "````");
        assert_eq!(fence("text\n```\nmore"), "````");
        // A 5-backtick run needs a 6-backtick fence.
        assert_eq!(fence("`````"), "``````");
    }
}
