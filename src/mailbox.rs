//! Email-format emitters (MBOX + Maildir) for the conversation-thread exporter.
//!
//! Each trie node becomes one email. Threading is reconstructed by RFC 5256:
//! every node has a unique `Message-ID`, and `In-Reply-To`/`References` point
//! at the parent's Message-ID. A branch point (node with N children) is just
//! N replies to the same parent — exactly what email clients render as a
//! nested, collapsible thread.
//!
//! One email per *node* (not per record_id): retries/edits that share a path
//! collapse to a single node; their record_ids are preserved as custom headers
//! and the body. Keeping one message per node is what makes threading clean.
//!
//! MBOX uses the mboxrd convention (body lines starting with `From ` are
//! `>`-escaped). Maildir writes cur/new/tmp with one file per message.
//! Bodies are MIME multipart/alternative (text/plain + text/html) when the
//! `--body html` mode is selected, or text/plain only under `--body plain`,
//! or text/html only under `--body html-only`.

use crate::markdown;
use anyhow::{anyhow, Result};
use serde_json::Value as Json;
use std::fs;
use std::io::Write;
use std::path::Path;

/// Body rendering mode selected at export time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyMode {
    /// `text/plain` only — raw message text.
    Plain,
    /// `multipart/alternative` with `text/plain` + `text/html`.
    Html,
    /// `text/html` only — rendered HTML, no plain fallback.
    HtmlOnly,
}

impl BodyMode {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "plain" => Self::Plain,
            "html" => Self::Html,
            "html-only" => Self::HtmlOnly,
            other => return Err(anyhow!("invalid --body {other:?} (plain|html|html-only)")),
        })
    }
}

/// Emit the forest as a single mbox file at `out`.
pub fn write_mbox(forest: &Json, mode: BodyMode, out: &Path) -> Result<usize> {
    let mut f = fs::File::create(out)?;
    let n = write_mbox_to(&mut f, forest, mode)?;
    f.flush()?;
    Ok(n)
}

/// Emit the forest as mbox to any writer (file or stdout).
pub fn write_mbox_to<W: Write>(w: &mut W, forest: &Json, mode: BodyMode) -> Result<usize> {
    let n = emit_forest(forest, mode, |msg| {
        // Compute the exact body bytes that will be written (with mboxrd
        // escaping and per-line newlines) so Content-Length is accurate.
        // mutt uses Content-Length to skip to the next message, which is
        // critical for multipart bodies that may contain "From " lines.
        let body = msg.body();
        let mut body_bytes = Vec::new();
        for line in body.lines() {
            if line.starts_with("From ") {
                body_bytes.push(b'>');
            }
            body_bytes.extend_from_slice(line.as_bytes());
            body_bytes.push(b'\n');
        }

        w.write_all(b"From ")?;
        w.write_all(msg.envelope_line().as_bytes())?;
        w.write_all(b"\n")?;
        // Write headers with Content-Length injected before the final
        // Content-Type line's trailing newline.
        let headers = msg.headers();
        // Find the position after the last header line content.
        let header_end = headers.rfind('\n').map(|i| i + 1).unwrap_or(headers.len());
        w.write_all(&headers.as_bytes()[..header_end])?;
        writeln!(w, "Content-Length: {}", body_bytes.len())?;
        w.write_all(&headers.as_bytes()[header_end..])?;
        w.write_all(b"\n")?;
        // Write the pre-computed body bytes.
        w.write_all(&body_bytes)?;
        w.write_all(b"\n")?;
        Ok(())
    })?;
    Ok(n)
}

/// Emit the forest as a Maildir at `out` (creates cur/new/tmp).
pub fn write_maildir(forest: &Json, mode: BodyMode, out: &Path) -> Result<usize> {
    fs::create_dir_all(out.join("cur"))?;
    fs::create_dir_all(out.join("new"))?;
    fs::create_dir_all(out.join("tmp"))?;
    let mut seq: u64 = 0;
    let pid = std::process::id();
    let n = emit_forest(forest, mode, |msg| {
        seq += 1;
        // Classic Maildir unique name: time.pid.seq.host
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let host = hostname();
        let name = format!("{now}.{pid}.{seq}.{host}");
        // Write atomically via tmp then rename.
        let tmp = out.join("tmp").join(&name);
        let new = out.join("new").join(&name);
        let mut w = fs::File::create(&tmp)?;
        w.write_all(msg.headers().as_bytes())?;
        w.write_all(b"\n")?;
        w.write_all(msg.body().as_bytes())?;
        w.flush()?;
        fs::rename(&tmp, &new)?;
        Ok(())
    })?;
    Ok(n)
}

/// Walk the forest and call `emit` once per node, threading each node to its
/// parent via Message-ID/In-Reply-To.
fn emit_forest<E: FnMut(Email) -> Result<()>>(
    forest: &Json,
    mode: BodyMode,
    mut emit: E,
) -> Result<usize> {
    let records = forest.get("records").and_then(|v| v.as_object());
    let mut count = 0usize;
    let empty: Vec<Json> = Vec::new();
    let trees: &Vec<Json> = forest
        .get("trees")
        .and_then(|t| t.as_array())
        .unwrap_or(&empty);
    for (thread_idx, root) in trees.iter().enumerate() {
        // Parent Message-ID for depth-0 is None (these are thread roots).
        walk_node(
            root, None, None, thread_idx, mode, records, &mut emit, &mut count,
        )?;
    }
    Ok(count)
}

/// Walk the forest and call `emit` once per *run* of consecutive same-role
/// nodes, threading each run to its parent via Message-ID/In-Reply-To.
///
/// A run is a linear chain: the entry node plus any following single child
/// that shares its role. The chain stops at a branch point (>1 child) or a
/// role change — those become children of the collapsed email. This collapses
/// e.g. `[system, system]` or `[assistant(tool_use), assistant(tool_use)]`
/// sequences into one email while keeping branch points as separate replies.
fn walk_node<E: FnMut(Email) -> Result<()>>(
    node: &Json,
    parent_msgid: Option<String>,
    parent_hash: Option<&str>,
    thread_idx: usize,
    mode: BodyMode,
    records: Option<&serde_json::Map<String, Json>>,
    emit: &mut E,
    count: &mut usize,
) -> Result<()> {
    // Collect the run: node + same-role single-child descendants.
    let mut run: Vec<&Json> = Vec::new();
    let mut cur = node;
    loop {
        run.push(cur);
        let kids = cur.get("children").and_then(|c| c.as_array());
        let only_child = kids
            .and_then(|k| k.iter().next())
            .filter(|_| kids.map(|k| k.len() == 1).unwrap_or(false));
        let next = match only_child {
            Some(c) => c,
            None => break, // branch point or leaf: end the run here
        };
        let same_role = next
            .get("role")
            .and_then(|v| v.as_str())
            .zip(cur.get("role").and_then(|v| v.as_str()))
            .map(|(a, b)| a == b)
            .unwrap_or(false);
        if !same_role {
            break;
        }
        cur = next;
    }

    let msgid = message_id(run[0], parent_hash, thread_idx);
    let email = Email::build(&run, &msgid, parent_msgid.as_deref(), mode, records)?;
    emit(email)?;
    *count += 1;

    // Recurse into the LAST node of the run's children (the run's exit),
    // replying to this run's Message-ID. The child's parent hash is the run's
    // entry-node hash, so distinct parents disambiguate Message-IDs.
    let exit = run.last().expect("run is non-empty");
    let entry_hash = run[0].get("hash").and_then(|v| v.as_str()).unwrap_or("");
    if let Some(kids) = exit.get("children").and_then(|c| c.as_array()) {
        for child in kids {
            walk_node(
                child,
                Some(msgid.clone()),
                Some(entry_hash),
                thread_idx,
                mode,
                records,
                emit,
                count,
            )?;
        }
    }
    Ok(())
}

/// Stable, unique Message-ID per node position: `<hash-depth-thread@czsplicer>`,
/// disambiguated by the parent node's content hash. Two nodes that share the
/// same `(hash, depth, thread_idx)` but are reached via different parents (a
/// shared subtree fragment) get distinct Message-IDs, avoiding RFC 5322
/// duplicates that would otherwise break threading. Roots use the sentinel
/// `root` in place of a parent hash.
fn message_id(node: &Json, parent_hash: Option<&str>, thread_idx: usize) -> String {
    let hash = node.get("hash").and_then(|v| v.as_str()).unwrap_or("x");
    let depth = node.get("depth").and_then(|v| v.as_i64()).unwrap_or(0);
    let parent = parent_hash.unwrap_or("root");
    format!("<{hash}-{depth}-{thread_idx}-{parent}@czsplicer>")
}

struct Email {
    headers: String,
    body: String,
    env_line: String,
}

impl Email {
    fn envelope_line(&self) -> &str {
        &self.env_line
    }
    fn headers(&self) -> &str {
        &self.headers
    }
    fn body(&self) -> &str {
        &self.body
    }

    /// Build one email covering a *run* of consecutive same-role nodes.
    ///
    /// `nodes` is a non-empty slice: the first node is the run's entry point,
    /// and any following nodes are same-role single children that were folded
    /// in by `walk_node` (a linear chain ending before a branch point or a
    /// role change). Content and `record_ids` are aggregated across the run;
    /// metadata (timestamp/model/status/subject) comes from the first node's
    /// introducer record.
    fn build(
        nodes: &[&Json],
        msgid: &str,
        parent_msgid: Option<&str>,
        mode: BodyMode,
        records: Option<&serde_json::Map<String, Json>>,
    ) -> Result<Self> {
        let first = nodes[0];
        let role = first.get("role").and_then(|v| v.as_str()).unwrap_or("");
        let depth = first.get("depth").and_then(|v| v.as_i64()).unwrap_or(0);
        let intro_rid = first.get("intro_rid").and_then(|v| v.as_i64()).unwrap_or(0);

        // Aggregate content across the run, dropping empty/preview-only nodes.
        let mut content_parts: Vec<String> = Vec::new();
        let mut preview = String::new();
        let mut record_ids: Vec<i64> = Vec::new();
        for n in nodes {
            let c = n.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if !c.is_empty() {
                content_parts.push(c.to_string());
            }
            if preview.is_empty() {
                preview = n
                    .get("preview")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
            }
            if let Some(a) = n.get("record_ids").and_then(|v| v.as_array()) {
                record_ids.extend(a.iter().filter_map(|x| x.as_i64()));
            }
        }
        let content = content_parts.join("\n\n");

        // Resolve per-record metadata from the first node's introducer record.
        let meta = records.and_then(|m| m.get(&intro_rid.to_string()));
        let status = meta
            .and_then(|m| m.get("status_code"))
            .and_then(|v| v.as_i64())
            .unwrap_or(200);
        let ts = meta
            .and_then(|m| m.get("timestamp"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let model = meta
            .and_then(|m| m.get("model"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let login_name = meta
            .and_then(|m| m.get("login_name"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // Sum tool call/result counts across the whole run.
        let mut tool_calls = 0i64;
        let mut tool_results = 0i64;
        for n in nodes {
            let rid = n.get("intro_rid").and_then(|v| v.as_i64()).unwrap_or(0);
            if let Some(m) = records.and_then(|rmap| rmap.get(&rid.to_string())) {
                tool_calls += m.get("tool_calls").and_then(|v| v.as_i64()).unwrap_or(0);
                tool_results += m.get("tool_results").and_then(|v| v.as_i64()).unwrap_or(0);
            }
        }

        // The Date: *header* is RFC 2822 (correct for a mail header). The
        // From_ *postmark* (envelope_line) must use ctime/asctime format
        // instead: mutt's strict is_from() parser only accepts the ctime
        // timestamp on the postmark line, not the RFC 2822 form. Emitting
        // RFC 2822 here makes mutt reject every postmark and report
        // "[Msgs:0]" on an otherwise-valid mbox.
        let date = rfc2822_date(ts);
        let env_date = asctime_date(ts);
        let from = sender_for_role(role, model, login_name);
        let subject = subject_for_node(first, role);

        let mut headers = String::new();
        use std::fmt::Write;
        let _ = writeln!(headers, "Message-ID: {msgid}");
        if let Some(p) = parent_msgid {
            let _ = writeln!(headers, "In-Reply-To: {p}");
            let _ = writeln!(headers, "References: {p}");
        }
        let _ = writeln!(headers, "From: {from}");
        let _ = writeln!(headers, "Subject: {subject}");
        let _ = writeln!(headers, "Date: {date}");
        let _ = writeln!(headers, "X-Czsplicer-Role: {role}");
        // Depth is the run's starting depth; for a collapsed run we also note
        // the span so the original structure is recoverable.
        if nodes.len() > 1 {
            let last_depth = nodes
                .last()
                .and_then(|n| n.get("depth"))
                .and_then(|v| v.as_i64())
                .unwrap_or(depth);
            let _ = writeln!(headers, "X-Czsplicer-Depth: {depth}-{last_depth}");
        } else {
            let _ = writeln!(headers, "X-Czsplicer-Depth: {depth}");
        }
        let _ = writeln!(headers, "X-Czsplicer-Status: {status}");
        if !model.is_empty() {
            let _ = writeln!(headers, "X-Czsplicer-Model: {model}");
        }
        if !record_ids.is_empty() {
            // Only emit the record count, not the full list — 320+ IDs on one
            // line would exceed RFC 5322's 998-char header limit and break
            // conformant mail parsers (mutt, etc.).
            let _ = writeln!(headers, "X-Czsplicer-Record-Count: {}", record_ids.len());
        }
        if tool_calls > 0 {
            let _ = writeln!(headers, "X-Czsplicer-Tool-Calls: {tool_calls}");
        }
        if tool_results > 0 {
            let _ = writeln!(headers, "X-Czsplicer-Tool-Results: {tool_results}");
        }

        let body = body_mime(mode, &content, &preview);

        // For assistant turns, extract tool calls and results as attachments.
        // Pairing is re-derived per node from intro_rid's call events paired
        // with the next record's result events (see tool_call_attachments), so
        // it stays correct when consecutive same-role nodes are collapsed.
        let tool_attachments = if role == "assistant" {
            tool_call_attachments(nodes, records)
        } else {
            Vec::new()
        };

        let (ct, payload) = if tool_attachments.is_empty() {
            (body.content_type_header, body.payload)
        } else {
            wrap_mixed(&body, &tool_attachments)
        };
        let _ = writeln!(headers, "MIME-Version: 1.0");
        let _ = writeln!(headers, "Content-Type: {ct}");

        Ok(Email {
            headers,
            body: payload,
            env_line: format!("czsplicer@localhost {env_date}"),
        })
    }
}

/// Sender display string per role.
/// - user: uses the Tailscale login_name as the email address (e.g. `clee@github`)
/// - assistant: uses `<normalized-model>@<provider>` (e.g. `glm-5.2@ollama`)
/// - system/other: falls back to role-based addresses
fn sender_for_role(role: &str, model: &str, login_name: &str) -> String {
    match role {
        "user" => {
            if login_name.is_empty() {
                "User <user@czsplicer>".to_string()
            } else {
                // login_name from Tailscale is already an email-style identity
                // (e.g. "clee@github"); use it directly.
                format!("User <{}>", sanitize_header(login_name))
            }
        }
        "assistant" => {
            if model.is_empty() {
                "Assistant <assistant@czsplicer>".to_string()
            } else {
                let (local, domain) = normalize_model_email(model);
                format!(
                    "Assistant ({}) <{}@{}>",
                    sanitize_header(model),
                    sanitize_header(&local),
                    sanitize_header(&domain)
                )
            }
        }
        "system" => "System <system@czsplicer>".to_string(),
        _ => format!("{} <unknown@czsplicer>", sanitize_header(role)),
    }
}

/// Split a model string like `ollama/glm-5.2` into `(normalized_local, domain)`.
/// The provider (prefix before `/`) becomes the domain; the model name
/// (suffix after `/`) is normalized to an email-local-part-safe form
/// (slashes and dots → dashes). If there's no `/`, the whole string is
/// the local part and the domain defaults to `czsplicer`.
fn normalize_model_email(model: &str) -> (String, String) {
    match model.split_once('/') {
        Some((provider, name)) => {
            let local = name
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '-'
                    }
                })
                .collect::<String>();
            (local, provider.to_string())
        }
        None => {
            let local = model
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '-'
                    }
                })
                .collect::<String>();
            (local, "czsplicer".to_string())
        }
    }
}

/// Subject: derive from the first user text under this tree if possible.
fn subject_for_node(node: &Json, role: &str) -> String {
    if role == "system" {
        // Use the system prompt preview as the subject, so distinct roots get
        // distinct threads (avoids Gmail merging unrelated trees).
        let preview = node.get("preview").and_then(|v| v.as_str()).unwrap_or("");
        return truncate_subject(preview, 100);
    }
    let content = node.get("content").and_then(|v| v.as_str()).unwrap_or("");
    truncate_subject(content, 100)
}

fn truncate_subject(s: &str, max: usize) -> String {
    // Collapse all whitespace (including newlines) to single spaces so the
    // result is a single-line, RFC 2822-safe header value.
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s: String = s.chars().take(max).collect();
    if s.is_empty() {
        "(empty)".to_string()
    } else {
        s
    }
}

/// Build the body MIME structure for the chosen mode.
fn body_mime(mode: BodyMode, content: &str, preview: &str) -> BodyMime {
    let plain = if content.is_empty() {
        preview.to_string()
    } else {
        content.to_string()
    };
    match mode {
        BodyMode::Plain => BodyMime {
            content_type_header: "text/plain; charset=utf-8".into(),
            payload: plain,
        },
        BodyMode::HtmlOnly => {
            let html = wrap_html(&markdown::to_html(&plain));
            BodyMime {
                content_type_header: "text/html; charset=utf-8".into(),
                payload: html,
            }
        }
        BodyMode::Html => {
            let boundary = format!("cz_{}", blake3::hash(plain.as_bytes()).to_hex());
            let html = wrap_html(&markdown::to_html(&plain));
            let payload = format!(
                "--{b}\nContent-Type: text/plain; charset=utf-8\n\n{plain}\n\n--{b}\nContent-Type: text/html; charset=utf-8\n\n{html}\n\n--{b}--\n",
                b = boundary
            );
            BodyMime {
                content_type_header: format!("multipart/alternative; boundary=\"{boundary}\""),
                payload,
            }
        }
    }
}

struct BodyMime {
    content_type_header: String,
    payload: String,
}

/// A tool-call attachment: the call parameters followed by `---` and the
/// tool result, as a single text/plain part.
struct ToolAttachment {
    filename: String,
    payload: String,
}

/// Extract tool calls and their matching results as attachments for a run of
/// collapsed same-role (assistant) nodes.
///
/// Pairing is re-derived from each node's own `tool_events` directly. For each
/// assistant node in the run:
///   - its `intro_rid` is the record whose *response* issued this turn's tool
///     call(s) (call events, kind=="call"). In real Aperture captures the
///     record that first includes an assistant message in its request path is
///     the same record whose response generated that turn, so `intro_rid`
///     holds the call;
///   - the record *after* `intro_rid` in that node's `record_ids` echoes the
///     matching tool *result(s)* in its request (result events, kind=="result").
///     Calls are paired positionally with results (call[i] ↔ result[i]).
///
/// Anchoring on each node's own `intro_rid`/`record_ids` keeps the pairing
/// correct when consecutive same-role nodes are collapsed: every node is
/// resolved independently, so collapsing shifts no indices.
fn tool_call_attachments(
    run: &[&Json],
    records: Option<&serde_json::Map<String, Json>>,
) -> Vec<ToolAttachment> {
    let rmap = match records {
        Some(m) => m,
        None => return Vec::new(),
    };

    /// Collect the tool_events of `kind` ("call" or "result") for one record.
    fn events_of<'a>(
        rmap: &'a serde_json::Map<String, Json>,
        rid: i64,
        kind: &str,
    ) -> Vec<&'a Json> {
        rmap.get(&rid.to_string())
            .and_then(|r| r.get("tool_events"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter(|e| e.get("kind").and_then(|v| v.as_str()) == Some(kind))
                    .collect()
            })
            .unwrap_or_default()
    }

    let mut out = Vec::new();
    let mut tool_seq = 0usize;
    for node in run {
        let intro_rid = node.get("intro_rid").and_then(|v| v.as_i64()).unwrap_or(0);
        let call_events = events_of(rmap, intro_rid, "call");
        if call_events.is_empty() {
            continue;
        }
        // The record echoing this turn's tool results is the one immediately
        // after intro_rid in this node's record_ids (intro_rid is first; the
        // next entry is the following captured request, which echoes results).
        let result_rid = node
            .get("record_ids")
            .and_then(|v| v.as_array())
            .and_then(|a| a.get(1))
            .and_then(|v| v.as_i64())
            .unwrap_or(intro_rid);
        let result_events = events_of(rmap, result_rid, "result");
        for (i, call) in call_events.iter().enumerate() {
            let name = call.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
            let input = call.get("input").and_then(|v| v.as_str()).unwrap_or("");
            let result = result_events
                .get(i)
                .and_then(|r| r.get("content").and_then(|v| v.as_str()))
                .unwrap_or("");
            let payload = if result.is_empty() {
                input.to_string()
            } else {
                format!("{input}\n---\n{result}")
            };
            out.push(ToolAttachment {
                filename: format!("tool-{tool_seq}-{name}.txt"),
                payload,
            });
            tool_seq += 1;
        }
    }
    out
}

/// Wrap the message body and tool attachments in a `multipart/mixed` MIME
/// structure. The first part is the original body (which may itself be
/// multipart/alternative for Html mode); subsequent parts are the tool
/// call+result attachments.
fn wrap_mixed(body: &BodyMime, attachments: &[ToolAttachment]) -> (String, String) {
    let mixed_boundary = format!(
        "cz_mixed_{}",
        blake3::hash(format!("{}{}", body.payload, attachments.len()).as_bytes()).to_hex()
    );

    let mut payload = String::new();
    // First part: the original body (with its own Content-Type).
    payload.push_str(&format!("--{mixed_boundary}\n"));
    payload.push_str(&format!("Content-Type: {}\n\n", body.content_type_header));
    payload.push_str(&body.payload);
    if !body.payload.ends_with('\n') {
        payload.push('\n');
    }
    payload.push('\n');

    // Tool attachments.
    for att in attachments {
        payload.push_str(&format!("--{mixed_boundary}\n"));
        payload.push_str(&format!(
            "Content-Type: text/plain; charset=utf-8\nContent-Disposition: attachment; filename=\"{}\"\n\n",
            sanitize_header(&att.filename)
        ));
        payload.push_str(&att.payload);
        if !att.payload.ends_with('\n') {
            payload.push('\n');
        }
        payload.push('\n');
    }

    payload.push_str(&format!("--{mixed_boundary}--\n"));

    (
        format!("multipart/mixed; boundary=\"{mixed_boundary}\""),
        payload,
    )
}

fn wrap_html(inner: &str) -> String {
    format!("<!DOCTYPE html>\n<html><body>\n{inner}\n</body></html>")
}

/// Convert an ISO-8601 timestamp to an RFC 2822 date. Falls back to the
/// current time on parse failure (and emits a warning-free fallback).
fn rfc2822_date(iso: &str) -> String {
    // We accept `YYYY-MM-DDTHH:MM:SS[.ffffff][Z|+HH:MM]`. Use chrono-free
    // parsing: the capture timestamps are always UTC with a trailing Z.
    if let Some(rest) = strip_to_iso_basic(iso) {
        let secs = parse_iso_utc_secs(&rest);
        if let Some(secs) = secs {
            return unix_to_rfc2822(secs);
        }
    }
    // Fallback: epoch.
    unix_to_rfc2822(0)
}

/// Keep only `YYYY-MM-DDTHH:MM:SS` (truncate fractional and drop zone).
fn strip_to_iso_basic(iso: &str) -> Option<String> {
    let bytes = iso.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let head = std::str::from_utf8(&bytes[..19]).ok()?;
    if head.len() == 19
        && head.as_bytes()[4] == b'-'
        && head.as_bytes()[7] == b'-'
        && head.as_bytes()[10] == b'T'
        && head.as_bytes()[13] == b':'
        && head.as_bytes()[16] == b':'
    {
        Some(head.to_string())
    } else {
        None
    }
}

/// Parse `YYYY-MM-DDTHH:MM:SS` to seconds since epoch (UTC). No leap seconds.
fn parse_iso_utc_secs(iso: &str) -> Option<i64> {
    let b = iso.as_bytes();
    if b.len() != 19 {
        return None;
    }
    let y: i64 = std::str::from_utf8(&b[0..4]).ok()?.parse().ok()?;
    let mo: u32 = std::str::from_utf8(&b[5..7]).ok()?.parse().ok()?;
    let d: u32 = std::str::from_utf8(&b[8..10]).ok()?.parse().ok()?;
    let h: u32 = std::str::from_utf8(&b[11..13]).ok()?.parse().ok()?;
    let mi: u32 = std::str::from_utf8(&b[14..16]).ok()?.parse().ok()?;
    let s: u32 = std::str::from_utf8(&b[17..19]).ok()?.parse().ok()?;
    Some(unix_from_ymdhms(y, mo, d, h, mi, s))
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    // Howard Hinnant's algorithm (proleptic Gregorian, days since 1970-01-01).
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + (if m > 2 { -3 } else { 9 })) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn unix_from_ymdhms(y: i64, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
    let days = days_from_civil(y, mo as i64, d as i64);
    days * 86400 + (h as i64) * 3600 + (mi as i64) * 60 + s as i64
}

/// Unix seconds -> RFC 2822 (e.g. "Mon, 26 Jun 2026 00:06:12 +0000").
fn unix_to_rfc2822(secs: i64) -> String {
    let (y, mo, d, h, mi, s) = civil_from_unix(secs);
    let wd = weekday_from_unix(secs);
    let mon = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][(mo - 1) as usize];
    let wdays = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][wd as usize];
    format!("{wdays}, {d:02} {mon} {y:04} {h:02}:{mi:02}:{s:02} +0000")
}

/// Unix seconds -> ctime/asctime (e.g. "Mon Jun 26 00:06:12 2026"), the
/// timestamp format mutt's is_from() requires on the mbox From_ postmark
/// line. The day-of-month is space-padded (matching C `asctime()`), which
/// mutt accepts alongside zero-padded forms.
fn unix_to_asctime(secs: i64) -> String {
    let (y, mo, d, h, mi, s) = civil_from_unix(secs);
    let wd = weekday_from_unix(secs);
    let mon = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][(mo - 1) as usize];
    let wdays = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][wd as usize];
    format!("{wdays} {mon} {d:>2} {h:02}:{mi:02}:{s:02} {y:04}")
}

/// ISO-8601 timestamp -> ctime/asctime for the From_ postmark. Falls back
/// to epoch on parse failure (mirroring rfc2822_date).
fn asctime_date(iso: &str) -> String {
    if let Some(rest) = strip_to_iso_basic(iso) {
        if let Some(secs) = parse_iso_utc_secs(&rest) {
            return unix_to_asctime(secs);
        }
    }
    unix_to_asctime(0)
}

fn civil_from_unix(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let h = rem / 3600;
    let mi = (rem % 3600) / 60;
    let s = rem % 60;
    // Inverse of days_from_civil.
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32, h as u32, mi as u32, s as u32)
}

fn weekday_from_unix(secs: i64) -> u32 {
    let days = secs.div_euclid(86400);
    // 1970-01-01 was a Thursday (4).
    ((days + 4).rem_euclid(7)) as u32
}

fn sanitize_header(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '\n' | '\r' | '\t'))
        .collect()
}

fn hostname() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_roundtrip_zero() {
        assert_eq!(unix_to_rfc2822(0), "Thu, 01 Jan 1970 00:00:00 +0000");
    }

    #[test]
    fn unix_known() {
        // 2026-06-26T00:06:12Z
        assert_eq!(
            unix_to_rfc2822(1782432372),
            "Fri, 26 Jun 2026 00:06:12 +0000"
        );
    }

    #[test]
    fn iso_parses() {
        assert_eq!(
            rfc2822_date("2026-06-26T00:06:12.123456789Z"),
            "Fri, 26 Jun 2026 00:06:12 +0000"
        );
    }

    #[test]
    fn iso_fractional_no_z() {
        // 2026-03-10T02:20:00Z
        assert_eq!(
            rfc2822_date("2026-03-10T02:20:00.000Z"),
            "Tue, 10 Mar 2026 02:20:00 +0000"
        );
    }

    #[test]
    fn body_plain_mode_is_text_plain() {
        let b = body_mime(BodyMode::Plain, "hello", "");
        assert_eq!(b.content_type_header, "text/plain; charset=utf-8");
        assert_eq!(b.payload, "hello");
    }

    #[test]
    fn body_html_only_mode_renders() {
        let b = body_mime(BodyMode::HtmlOnly, "**bold**", "");
        assert!(b.content_type_header.starts_with("text/html"));
        assert!(b.payload.contains("<strong>bold</strong>"));
    }

    #[test]
    fn body_html_mode_is_multipart() {
        let b = body_mime(BodyMode::Html, "**bold**", "");
        assert!(b.content_type_header.starts_with("multipart/alternative"));
        assert!(b.payload.contains("text/plain"));
        assert!(b.payload.contains("text/html"));
        assert!(b.payload.contains("<strong>bold</strong>"));
    }

    #[test]
    fn message_id_is_unique_per_node() {
        let n1 = serde_json::json!({"hash":"abcd","depth":0});
        let n2 = serde_json::json!({"hash":"abcd","depth":1});
        assert_ne!(message_id(&n1, None, 0), message_id(&n2, None, 0));
    }

    #[test]
    fn message_id_disambiguates_same_hash_different_parents() {
        // Two nodes sharing (hash, depth, thread_idx) but reached via different
        // parents must get distinct Message-IDs (a shared subtree fragment).
        let n = serde_json::json!({"hash":"abcd","depth":2});
        assert_ne!(
            message_id(&n, Some("parent1"), 0),
            message_id(&n, Some("parent2"), 0),
            "distinct parents -> distinct Message-IDs"
        );
        // Roots (no parent) use the "root" sentinel.
        assert_eq!(
            message_id(&n, None, 0),
            message_id(&n, Some("root"), 0),
            "None parent hashes the same as the 'root' sentinel"
        );
    }
}
