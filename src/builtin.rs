//! Built-in long-form HTML renderer for conversation threads.
//!
//! Designed for LLM transcripts (the opposite of a chat bubble UI): a wide
//! single column, slim role rails, full markdown rendering, tool-call blocks,
//! HTTP status chips, and a path selector when a conversation branches.
//!
//! Markdown is rendered to a safe HTML subset in Rust (`crate::markdown`);
//! syntax highlighting runs client-side from the vendored highlight.js bundle
//! (see `vendor/`). The output is a single self-contained `.html` file.

use crate::markdown;
use crate::render::{best_record_id, escape_html, model_of, sender_color, truncate};
use crate::thread;
use anyhow::Result;
use serde_json::Value as Json;

const HLJS_JS: &str = include_str!("../vendor/highlight.min.js");
const HLJS_CSS_LIGHT: &str = include_str!("../vendor/highlight-github.css");
const HLJS_CSS_DARK: &str = include_str!("../vendor/highlight-github-dark.css");

/// Render the forest JSON to a self-contained HTML document.
pub fn render_html(forest: &Json, dark: bool) -> Result<String> {
    let records = forest.get("records").and_then(|r| r.as_object());
    let paths = thread::all_paths(forest);

    let mut body = String::new();
    body.push_str("<header class=\"doc-header\">\n");
    body.push_str(&summary_section(forest));
    body.push_str("</header>\n");

    if paths.len() > 1 {
        body.push_str(&path_selector(&paths));
    }

    for (idx, path) in paths.iter().enumerate() {
        body.push_str(&format!(
            "<section class=\"path\" id=\"path-{idx}\" data-path=\"{idx}\">\n"
        ));
        if paths.len() > 1 {
            body.push_str(&format!(
                "<h2 class=\"path-title\">Path {} of {} <span class=\"path-meta\">{} messages</span></h2>\n",
                idx + 1,
                paths.len(),
                path.len()
            ));
        }
        body.push_str("<div class=\"turns\">\n");
        body.push_str(&render_path(path, records));
        body.push_str("</div>\n</section>\n");
    }

    Ok(document(&body, dark))
}

fn document(body: &str, dark: bool) -> String {
    let theme_attr = if dark { "dark" } else { "light" };
    let hljs_css = if dark { HLJS_CSS_DARK } else { HLJS_CSS_LIGHT };
    format!(
        "<!doctype html>\n<html lang=\"en\" data-theme=\"{theme_attr}\">\n\
         <head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n\
         <title>Conversation — czsplicer</title>\n\
         <style>\n{base_css}\n{hljs_css}\n</style>\n</head>\n<body>\n{body}\n\
         <script>{hljs_js}</script>\n\
         <script>hljs.highlightAll();</script>\n\
         </body>\n</html>\n",
        base_css = BASE_CSS,
        hljs_css = hljs_css,
        body = body,
        hljs_js = HLJS_JS,
        theme_attr = theme_attr,
    )
}

/// Top-of-document summary: counts, model/status breakdown, time span.
fn summary_section(forest: &Json) -> String {
    let total = forest
        .get("records_total")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let with_msgs = forest
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

    let mut models = vec![];
    let mut statuses = vec![];
    let mut tool_calls = 0u64;
    let mut tool_results = 0u64;
    if let Some(recs) = forest.get("records").and_then(|r| r.as_object()) {
        let mut m = std::collections::BTreeMap::new();
        let mut s = std::collections::BTreeMap::new();
        for (_id, meta) in recs.iter() {
            if let Some(model) = meta.get("model").and_then(|v| v.as_str()) {
                *m.entry(model.to_string()).or_insert(0u64) += 1;
            }
            if let Some(status) = meta.get("status_code").and_then(|v| v.as_i64()) {
                *s.entry(status).or_insert(0u64) += 1;
            }
            tool_calls += meta.get("tool_calls").and_then(|v| v.as_u64()).unwrap_or(0);
            tool_results += meta
                .get("tool_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
        }
        models = m.into_iter().collect();
        statuses = s.into_iter().collect();
    }

    let mut out = String::new();
    out.push_str("<h1>Conversation</h1>\n");
    out.push_str("<dl class=\"summary-grid\">\n");
    out.push_str(&dt_dd("Records", &total.to_string()));
    out.push_str(&dt_dd("With messages", &with_msgs.to_string()));
    out.push_str(&dt_dd("Conversation roots", &roots.to_string()));
    out.push_str(&dt_dd("Branch points", &branches.to_string()));
    out.push_str(&dt_dd("Tool calls", &tool_calls.to_string()));
    out.push_str(&dt_dd("Tool results", &tool_results.to_string()));
    out.push_str("</dl>\n");

    if !models.is_empty() {
        out.push_str("<div class=\"chips\"><span class=\"chip-label\">models</span>");
        for (name, n) in &models {
            out.push_str(&format!(
                "<span class=\"chip chip-model\" style=\"--sender-color:{}\">{} <em>{}</em></span>",
                sender_color(name),
                escape_html(name),
                n
            ));
        }
        out.push_str("</div>\n");
    }
    if !statuses.is_empty() {
        out.push_str("<div class=\"chips\"><span class=\"chip-label\">status</span>");
        for (code, n) in &statuses {
            out.push_str(&format!(
                "<span class=\"chip chip-status\" data-status=\"{}\">{} <em>{}</em></span>\n",
                code, code, n
            ));
        }
        out.push_str("</div>\n");
    }
    out
}

fn dt_dd(term: &str, val: &str) -> String {
    format!(
        "<dt>{}</dt><dd>{}</dd>",
        escape_html(term),
        escape_html(val)
    )
}

/// A row of path buttons when the conversation branches.
fn path_selector(paths: &[Vec<&Json>]) -> String {
    let mut out = String::new();
    out.push_str("<nav class=\"path-selector\">\n<span class=\"chip-label\">paths</span>\n");
    for (idx, p) in paths.iter().enumerate() {
        // Describe a path by its leaf turn for a short label.
        let label = p
            .last()
            .and_then(|n| n.get("preview").and_then(|v| v.as_str()))
            .unwrap_or("");
        let label = truncate(label, 40);
        out.push_str(&format!(
            "<a href=\"#path-{idx}\" class=\"path-link\">{} <em>{} msg</em></a>\n",
            idx + 1,
            p.len()
        ));
        let _ = label;
    }
    out.push_str("</nav>\n");
    out
}

/// Render one root-to-leaf path as a sequence of turns.
fn render_path(path: &[&Json], records: Option<&serde_json::Map<String, Json>>) -> String {
    let mut out = String::new();
    for (i, node) in path.iter().enumerate() {
        let role = node.get("role").and_then(|v| v.as_str()).unwrap_or("");

        // Metadata anchor: the record whose request included this node's
        // message. For node i, that's the record that introduced node i+1
        // (intro_rid — distinct per node, gives correct per-turn timestamps).
        // We fall back to the last record_id if the intro record has a
        // non-2xx status (avoids poisoning from a single 404/500 that
        // happened to introduce a shared prefix node).
        let meta_rid = if i + 1 < path.len() {
            best_record_id(path[i + 1], records)
        } else {
            best_record_id(node, records)
        };
        let meta_rec = meta_rid.and_then(|id| records.and_then(|rmap| rmap.get(&id.to_string())));

        // Tool-event anchors for assistant turns. In a linear chain each
        // record adds one message, so:
        //   record_ids[i]     → response has the tool CALL for this turn
        //   record_ids[i+1]   → request echoes the tool RESULT for that call
        // We render call-then-result in the correct order. Non-assistant
        // turns never carry tool events.
        let (call_rec, result_rec) = if role == "assistant" {
            let ids = node.get("record_ids").and_then(|v| v.as_array());
            let call = ids
                .and_then(|ids| {
                    let idx = i.min(ids.len().saturating_sub(1));
                    ids.get(idx).and_then(|v| v.as_i64())
                })
                .and_then(|id| records.and_then(|rmap| rmap.get(&id.to_string())));
            let result = ids
                .and_then(|ids| {
                    let idx = (i + 1).min(ids.len().saturating_sub(1));
                    ids.get(idx).and_then(|v| v.as_i64())
                })
                .and_then(|id| records.and_then(|rmap| rmap.get(&id.to_string())));
            (call, result)
        } else {
            (None, None)
        };

        let (label, kind) = match role {
            "user" => ("you".to_string(), "user"),
            "assistant" => (
                model_of(meta_rec).unwrap_or_else(|| "assistant".into()),
                "assistant",
            ),
            "system" => ("system".to_string(), "system"),
            other => (other.to_string(), "other"),
        };

        let body = node
            .get("content")
            .and_then(|v| v.as_str())
            .or_else(|| node.get("preview").and_then(|v| v.as_str()))
            .unwrap_or("");
        let tools_html = if role == "assistant" {
            tool_events_html(call_rec, result_rec)
        } else {
            String::new()
        };
        let content_html = if role == "system" || role == "assistant" {
            let md = if body.is_empty() {
                String::new()
            } else {
                markdown::to_html(body)
            };
            if tools_html.is_empty() {
                md
            } else {
                format!("{}\n{}", md, tools_html)
            }
        } else {
            // User messages: preserve as-is but escaped + line breaks.
            format!("<p>{}</p>", escape_html(body).replace('\n', "<br>"))
        };

        let meta = meta_chips(meta_rec);

        out.push_str(&format!(
            "<article class=\"turn turn-{kind}\" style=\"--sender-color:{color}\">\n\
             <header class=\"turn-head\">\n\
             <span class=\"turn-sender turn-sender-{kind}\">{sender}</span>\n\
             {time}\n\
             </header>\n\
             <div class=\"turn-body md\">{content_html}</div>\n\
             {meta}\n\
             </article>\n",
            kind = kind,
            color = sender_color(&label),
            sender = escape_html(&label),
            time = time_html(meta_rec),
            content_html = content_html,
            meta = meta,
        ));
    }
    out
}

/// Per-turn chips: HTTP status, tool calls, tool results.
fn meta_chips(rec: Option<&Json>) -> String {
    let Some(rec) = rec else {
        return String::new();
    };
    let mut out = String::new();
    if let Some(s) = rec.get("status_code").and_then(|v| v.as_i64()) {
        out.push_str(&format!(
            "<span class=\"chip chip-status\" data-status=\"{}\">HTTP {}</span>\n",
            s, s
        ));
    }
    let tc = rec.get("tool_calls").and_then(|v| v.as_u64()).unwrap_or(0);
    let tr = rec
        .get("tool_results")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if tc > 0 {
        out.push_str(&format!(
            "<span class=\"chip chip-tool\">{} tool call(s)</span>\n",
            tc
        ));
    }
    if tr > 0 {
        out.push_str(&format!(
            "<span class=\"chip chip-tool\">{} tool result(s)</span>\n",
            tr
        ));
    }
    if out.is_empty() {
        String::new()
    } else {
        format!("<footer class=\"turn-meta\">{}</footer>\n", out.trim_end())
    }
}

fn time_html(rec: Option<&Json>) -> String {
    let Some(rec) = rec else {
        return String::new();
    };
    let ts = rec.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
    if ts.is_empty() {
        String::new()
    } else {
        format!("<time class=\"turn-time\">{}</time>", escape_html(ts))
    }
}

/// Render tool events as expandable HTML blocks. Tool *calls* (name + input)
/// come from the assistant response record; tool *results* (output content)
/// come from the next record's request (which echoes the result of this
/// call). Rendered in conversation order: call, then its result.
fn tool_events_html(call_rec: Option<&Json>, result_rec: Option<&Json>) -> String {
    let mut out = String::new();

    // Calls from the current record's response.
    if let Some(events) = call_rec
        .and_then(|r| r.get("tool_events"))
        .and_then(|v| v.as_array())
    {
        for ev in events {
            if ev.get("kind").and_then(|v| v.as_str()) == Some("call") {
                let name = ev.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
                let input = ev.get("input").and_then(|v| v.as_str()).unwrap_or("");
                out.push_str(&format!(
                    "<details class=\"tool-call\">\n\
                     <summary>tool call: <code>{}</code></summary>\n\
                     <pre class=\"tool-input\">{}</pre>\n\
                     </details>\n",
                    escape_html(name),
                    escape_html(input)
                ));
            }
        }
    }

    // Results from the next record's request (output of the call above).
    if let Some(events) = result_rec
        .and_then(|r| r.get("tool_events"))
        .and_then(|v| v.as_array())
    {
        for ev in events {
            if ev.get("kind").and_then(|v| v.as_str()) == Some("result") {
                let content = ev.get("content").and_then(|v| v.as_str()).unwrap_or("");
                if !content.is_empty() {
                    out.push_str(&format!(
                        "<details class=\"tool-result\">\n\
                         <summary>tool result</summary>\n\
                         <pre class=\"tool-output\">{}</pre>\n\
                         </details>\n",
                        escape_html(content)
                    ));
                }
            }
        }
    }

    out
}

const BASE_CSS: &str = include_str!("builtin.css");
