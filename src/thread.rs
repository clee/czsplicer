//! Conversation-thread reconstruction from `.cbor.zstd` log streams.
//!
//! Each request record echoes its full conversation history in
//! `capture.requestBody.messages` (an OpenAI/Anthropic-style message array).
//! By hashing each message's content and building a trie over hash sequences,
//! we reconstruct the branching structure: a place where the user "went back"
//! and took a different path shows up as a node with multiple children.
//!
//! `session_id` is NOT used for grouping — in Aperture captures each request
//! gets a unique session_id. The tree is reconstructed purely from message
//! prefix structure. A depth-0 node (usually the system prompt) is a
//! conversation root; its descendants are the turns.

use crate::format;
use crate::render::truncate;
use anyhow::Result;
use serde_json::{Map as JsonMap, Value as Json};
use std::collections::BTreeMap;

/// Truncated hash length (hex chars). 64 bits is collision-free for any
/// realistic dataset.
const HASH_HEX_LEN: usize = 16;
/// Max characters in a node preview string.
const PREVIEW_LEN: usize = 160;

/// A normalized, hashed message — one element of a conversation path.
pub struct MsgInfo {
    pub hash: String,
    pub role: String,
    pub preview: String,
    /// Full normalized text content (all text blocks joined). Used by
    /// renderers that want the complete message body (e.g. the built-in
    /// long-form HTML renderer). Empty for non-text content.
    pub content: String,
}

/// A trie node: one message in one or more conversation paths.
struct Node {
    hash: String,
    role: String,
    preview: String,
    content: String,
    depth: usize,
    /// Every record whose message path passes through this node.
    record_ids: Vec<i64>,
    /// The record that introduced this message (the record whose path first
    /// created this node). Used to resolve per-node metadata (timestamp,
    /// model) correctly instead of grabbing the arbitrary last record_id.
    intro_rid: i64,
    /// Children keyed by their content hash.
    children: BTreeMap<String, Node>,
}

impl Node {
    fn new(info: &MsgInfo, depth: usize, intro_rid: i64) -> Self {
        Self {
            hash: info.hash.clone(),
            role: info.role.clone(),
            preview: info.preview.clone(),
            content: info.content.clone(),
            depth,
            record_ids: Vec::new(),
            intro_rid,
            children: BTreeMap::new(),
        }
    }

    /// Recursively insert a path; every node along the path records `rid`.
    /// A child created via `or_insert_with` records `rid` as its introducer.
    fn insert(&mut self, path: &[MsgInfo], rid: i64) {
        self.record_ids.push(rid);
        if path.is_empty() {
            return;
        }
        let head = &path[0];
        let depth = self.depth + 1;
        let child = self
            .children
            .entry(head.hash.clone())
            .or_insert_with(|| Node::new(head, depth, rid));
        child.insert(&path[1..], rid);
    }

    /// Count nodes with more than one child (branch points).
    fn count_branches(&self) -> usize {
        let n = if self.children.len() > 1 { 1 } else { 0 };
        n + self
            .children
            .values()
            .map(|c| c.count_branches())
            .sum::<usize>()
    }

    fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("hash".into(), Json::String(self.hash.clone()));
        m.insert("role".into(), Json::String(self.role.clone()));
        m.insert("depth".into(), Json::Number(self.depth.into()));
        m.insert("preview".into(), Json::String(self.preview.clone()));
        m.insert("content".into(), Json::String(self.content.clone()));
        m.insert("intro_rid".into(), Json::Number(self.intro_rid.into()));
        m.insert(
            "record_ids".into(),
            Json::Array(
                self.record_ids
                    .iter()
                    .map(|&i| Json::Number(i.into()))
                    .collect(),
            ),
        );
        m.insert(
            "record_count".into(),
            Json::Number(self.record_ids.len().into()),
        );
        m.insert("is_branch".into(), Json::Bool(self.children.len() > 1));
        m.insert(
            "children".into(),
            Json::Array(self.children.values().map(|c| c.to_json()).collect()),
        );
        Json::Object(m)
    }
}

/// Builds a forest of conversation threads from streamed records.
pub struct ThreadBuilder {
    /// Depth-0 nodes (conversation roots), keyed by content hash.
    roots: BTreeMap<String, Node>,
    /// Per-record metadata, keyed by record id. Used by renderers to plot
    /// timelines, status colors, and tool-call outcomes without re-parsing
    /// the source records.
    records: BTreeMap<i64, RecordMeta>,
}

impl ThreadBuilder {
    pub fn new() -> Self {
        Self {
            roots: BTreeMap::new(),
            records: BTreeMap::new(),
        }
    }

    /// Extract and insert one record's message path. Returns true if the
    /// record contributed a parseable message path.
    pub fn add_record(&mut self, rec: &ciborium::Value) -> Result<bool> {
        let rid = format::rec_int(rec, "id").unwrap_or(0);
        // Capture per-record metadata regardless of whether the message path
        // is parseable — a 500 with an empty body still belongs on the timeline.
        self.records.insert(rid, RecordMeta::from_record(rec));
        let path = match record_message_path(rec)? {
            Some(p) if !p.is_empty() => p,
            _ => return Ok(false),
        };
        let head = &path[0];
        let root = self
            .roots
            .entry(head.hash.clone())
            .or_insert_with(|| Node::new(head, 0, rid));
        root.insert(&path[1..], rid);
        Ok(true)
    }

    /// Serialize the forest as JSON.
    pub fn to_json(&self, records_total: u64, records_with_messages: u64) -> Json {
        let branch_count: usize = self.roots.values().map(|n| n.count_branches()).sum();
        let mut trees: Vec<&Node> = self.roots.values().collect();
        trees.sort_by_key(|t| std::cmp::Reverse(t.record_ids.len()));
        let mut m = JsonMap::new();
        m.insert("records_total".into(), Json::Number(records_total.into()));
        m.insert(
            "records_with_messages".into(),
            Json::Number(records_with_messages.into()),
        );
        m.insert("root_count".into(), Json::Number(self.roots.len().into()));
        m.insert("branch_count".into(), Json::Number(branch_count.into()));
        m.insert(
            "records".into(),
            Json::Object(
                self.records
                    .iter()
                    .map(|(&id, meta)| (id.to_string(), meta.to_json()))
                    .collect(),
            ),
        );
        m.insert(
            "trees".into(),
            Json::Array(trees.iter().map(|t| t.to_json()).collect()),
        );
        Json::Object(m)
    }
}

/// A single tool call (from the assistant response) or tool result
/// (from the next request's echoed messages), captured for rendering.
#[derive(Clone)]
enum ToolEvent {
    /// Assistant invoked a tool. `name` is the function/tool name;
    /// `input` is the arguments (JSON string, pretty-printed).
    Call { name: String, input: String },
    /// A tool result returned to the model. `content` is the result body.
    Result { content: String },
}

impl ToolEvent {
    fn to_json(&self) -> Json {
        match self {
            ToolEvent::Call { name, input } => {
                let mut o = JsonMap::new();
                o.insert("kind".into(), Json::String("call".into()));
                o.insert("name".into(), Json::String(name.clone()));
                o.insert("input".into(), Json::String(input.clone()));
                Json::Object(o)
            }
            ToolEvent::Result { content } => {
                let mut o = JsonMap::new();
                o.insert("kind".into(), Json::String("result".into()));
                o.insert("content".into(), Json::String(content.clone()));
                Json::Object(o)
            }
        }
    }
}

/// Per-record metadata for rendering (timeline dots, status colors, tool
/// outcomes). Kept intentionally flat so the HTML renderer can look up a
/// record by id without re-parsing CBOR.
#[derive(Default)]
struct RecordMeta {
    status_code: Option<i64>,
    timestamp: Option<String>,
    duration_ms: Option<i64>,
    model: Option<String>,
    api_type: Option<String>,
    path: Option<String>,
    login_name: Option<String>,
    /// Number of tool-call requests in the assistant response (tool_use).
    tool_calls: usize,
    /// Number of tool-result blocks observed in this record's request messages
    /// (the echoed results from the previous turn).
    tool_results: usize,
    /// Structured tool events: calls (from the response) and results (from
    /// the next request's echoed messages), in conversation order.
    tool_events: Vec<ToolEvent>,
}

impl RecordMeta {
    fn from_record(rec: &ciborium::Value) -> Self {
        let mut m = Self {
            status_code: format::rec_int(rec, "status_code"),
            timestamp: format::rec_str(rec, "timestamp"),
            duration_ms: format::rec_int(rec, "duration_ms"),
            model: format::rec_str(rec, "model"),
            api_type: format::rec_str(rec, "api_type"),
            path: format::rec_str(rec, "path"),
            login_name: format::path_get(rec, "identity.login_name").and_then(format::as_str),
            tool_calls: 0,
            tool_results: 0,
            tool_events: Vec::new(),
        };
        // Tool results live in the request messages (echoed from the prior turn):
        // OpenAI uses role:"tool"; Anthropic uses content blocks of type
        // "tool_result". We count only the tool results that are NEW to this
        // request — i.e. those appearing AFTER the last assistant message in
        // the history. Otherwise every record in a long thread re-counts the
        // accumulated prior results (and every turn would show "227").
        if let Ok(Some(body)) = record_request_body(rec) {
            if let Some(msgs) = body.get("messages").and_then(|v| v.as_array()) {
                // Find the index of the last assistant message; everything
                // after it is the new user-side turn (possibly tool results).
                let last_asst = msgs
                    .iter()
                    .rposition(|m| m.get("role").and_then(|v| v.as_str()) == Some("assistant"));
                let new_start = last_asst.map(|i| i + 1).unwrap_or(0);
                for msg in &msgs[new_start..] {
                    let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
                    if role == "tool" {
                        m.tool_results += 1;
                        let content = msg
                            .get("content")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        m.tool_events.push(ToolEvent::Result { content });
                    }
                    if let Some(blocks) = msg.get("content").and_then(|v| v.as_array()) {
                        for b in blocks {
                            if b.get("type").and_then(|v| v.as_str()) == Some("tool_result") {
                                m.tool_results += 1;
                                let content = tool_result_content(b);
                                m.tool_events.push(ToolEvent::Result { content });
                            }
                        }
                    }
                }
            }
        }
        // Tool calls live in the assistant response: OpenAI exposes them as
        // choices[0].message.tool_calls; Anthropic as content blocks of type
        // "tool_use". We count either shape and capture name + input.
        if let Some(resp) = record_response_json(rec) {
            if let Some(choices) = resp.get("choices").and_then(|v| v.as_array()) {
                for ch in choices {
                    if let Some(tc) = ch
                        .get("message")
                        .and_then(|v| v.get("tool_calls"))
                        .and_then(|v| v.as_array())
                    {
                        for call in tc {
                            m.tool_calls += 1;
                            let name = call
                                .get("function")
                                .and_then(|f| f.get("name"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("tool")
                                .to_string();
                            let input = call
                                .get("function")
                                .and_then(|f| f.get("arguments"))
                                .map(|v| match v {
                                    Json::String(s) => pretty_json(s),
                                    other => pretty_json_value(other),
                                })
                                .unwrap_or_default();
                            m.tool_events.push(ToolEvent::Call { name, input });
                        }
                    }
                }
            }
            if let Some(blocks) = resp.get("content").and_then(|v| v.as_array()) {
                for b in blocks {
                    if b.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                        m.tool_calls += 1;
                        let name = b
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("tool")
                            .to_string();
                        let input = b.get("input").map(pretty_json_value).unwrap_or_default();
                        m.tool_events.push(ToolEvent::Call { name, input });
                    }
                }
            }
        }
        m
    }

    fn to_json(&self) -> Json {
        let mut o = JsonMap::new();
        if let Some(s) = self.status_code {
            o.insert("status_code".into(), Json::Number(s.into()));
        }
        if let Some(ref t) = self.timestamp {
            o.insert("timestamp".into(), Json::String(t.clone()));
        }
        if let Some(d) = self.duration_ms {
            o.insert("duration_ms".into(), Json::Number(d.into()));
        }
        if let Some(ref s) = self.model {
            o.insert("model".into(), Json::String(s.clone()));
        }
        if let Some(ref s) = self.api_type {
            o.insert("api_type".into(), Json::String(s.clone()));
        }
        if let Some(ref s) = self.path {
            o.insert("path".into(), Json::String(s.clone()));
        }
        if let Some(ref s) = self.login_name {
            o.insert("login_name".into(), Json::String(s.clone()));
        }
        o.insert("tool_calls".into(), Json::Number(self.tool_calls.into()));
        o.insert(
            "tool_results".into(),
            Json::Number(self.tool_results.into()),
        );
        if !self.tool_events.is_empty() {
            o.insert(
                "tool_events".into(),
                Json::Array(self.tool_events.iter().map(|e| e.to_json()).collect()),
            );
        }
        Json::Object(o)
    }
}

/// Parse and return a record's `capture.requestBody` as JSON, if present.
fn record_request_body(rec: &ciborium::Value) -> Result<Option<Json>> {
    let body_val = match format::path_get(rec, "capture.requestBody") {
        Some(v) => v,
        None => return Ok(None),
    };
    match body_val {
        // Body may fail to parse as JSON if a prior redaction pass split an
        // escape sequence; treat that as "no parseable body" rather than fatal.
        ciborium::Value::Text(s) if !s.is_empty() => Ok(serde_json::from_str::<Json>(s).ok()),
        _ => Ok(None),
    }
}

/// Parse and return a record's `capture.responseBody` as JSON, if present.
fn record_response_json(rec: &ciborium::Value) -> Option<Json> {
    let v = format::path_get(rec, "capture.responseBody")?;
    match v {
        ciborium::Value::Text(s) if !s.is_empty() => serde_json::from_str::<Json>(s).ok(),
        _ => None,
    }
}

/// Pretty-print a JSON string (e.g. OpenAI `arguments`). Falls back to the
/// raw string if it isn't valid JSON.
fn pretty_json(s: &str) -> String {
    serde_json::from_str::<Json>(s)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| s.to_string())
}

/// Pretty-print a JSON value (e.g. Anthropic `input` object).
fn pretty_json_value(v: &Json) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

/// Extract the text content from an Anthropic `tool_result` block. The
/// `content` field may be a string, an array of text blocks, or absent.
fn tool_result_content(b: &Json) -> String {
    match b.get("content") {
        Some(Json::String(s)) => s.clone(),
        Some(Json::Array(arr)) => {
            let parts: Vec<String> = arr
                .iter()
                .filter_map(|block| {
                    block
                        .as_object()
                        .and_then(|o| o.get("text").and_then(|v| v.as_str()).map(String::from))
                        .or_else(|| block.as_str().map(String::from))
                })
                .collect();
            parts.join("\n")
        }
        _ => String::new(),
    }
}

/// Parse a record's `capture.requestBody.messages` into a hashed path.
fn record_message_path(rec: &ciborium::Value) -> Result<Option<Vec<MsgInfo>>> {
    let body_json = match record_request_body(rec)? {
        Some(b) => b,
        None => return Ok(None),
    };
    let messages = match body_json.get("messages").and_then(|m| m.as_array()) {
        Some(a) => a,
        None => return Ok(None),
    };
    let path: Vec<MsgInfo> = messages
        .iter()
        .filter(|m| {
            matches!(
                m.get("role").and_then(|r| r.as_str()),
                Some("system" | "user" | "assistant")
            )
        })
        .map(msg_info)
        .collect();
    Ok(Some(path))
}

/// Compute the content hash, role, and preview for a single message.
fn msg_info(msg: &Json) -> MsgInfo {
    let role = msg
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let content = msg.get("content").unwrap_or(&Json::Null);
    // Normalize string content to block form so that a bare-string message and
    // the equivalent [{type:text}] message hash identically.
    let normalized = match content {
        Json::String(s) => Json::Array(vec![{
            let mut b = JsonMap::new();
            b.insert("type".into(), Json::String("text".into()));
            b.insert("text".into(), Json::String(s.clone()));
            Json::Object(b)
        }]),
        other => other.clone(),
    };
    let canon = serde_json::to_string(&serde_json::json!({
        "r": &role,
        "c": &normalized,
    }))
    .unwrap_or_default();
    let full = blake3::hash(canon.as_bytes()).to_hex().to_string();
    let hash = full[..HASH_HEX_LEN.min(full.len())].to_string();
    let preview = extract_preview(&normalized);
    let content = full_content(&normalized);
    MsgInfo {
        hash,
        role,
        preview,
        content,
    }
}

/// Extract displayable text from a content block, handling all known types
/// (text, tool_use, tool_result, thinking, image, document, refusal, etc.).
fn block_text(obj: &serde_json::Map<String, Json>) -> Option<String> {
    let typ = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match typ {
        "text" => obj.get("text").and_then(|v| v.as_str()).map(String::from),
        "tool_use" => {
            let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
            Some(format!("[tool_use: {name}]"))
        }
        "tool_result" => obj
            .get("content")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| Some("[tool_result]".to_string())),
        "thinking" => obj
            .get("thinking")
            .and_then(|v| v.as_str())
            .map(String::from),
        "redacted_thinking" => Some("[redacted_thinking]".to_string()),
        "image" => Some("[image]".to_string()),
        "document" => Some("[document]".to_string()),
        "refusal" => obj
            .get("refusal")
            .and_then(|v| v.as_str())
            .map(String::from),
        "image_url" => Some("[image_url]".to_string()),
        "input_audio" => Some("[input_audio]".to_string()),
        _ => None,
    }
}

/// Full text of a message (all text blocks joined by blank lines), untruncated.
/// Used by renderers that need the complete body (e.g. the built-in long-form
/// HTML renderer, which runs markdown over it).
fn full_content(normalized: &Json) -> String {
    let parts: Vec<String> = match normalized {
        Json::Array(arr) => arr
            .iter()
            .filter_map(|b| b.as_object())
            .filter_map(block_text)
            .collect(),
        Json::String(s) => vec![s.clone()],
        _ => vec![],
    };
    parts.join("\n\n")
}

/// Pull a human-readable preview from normalized content blocks.
fn extract_preview(normalized: &Json) -> String {
    let parts: Vec<String> = match normalized {
        Json::Array(arr) => arr
            .iter()
            .filter_map(|b| b.as_object())
            .filter_map(block_text)
            .filter(|s| !s.is_empty())
            .collect(),
        Json::String(s) => vec![s.clone()],
        _ => vec![],
    };
    let joined = parts.join("  ");
    truncate(&joined, PREVIEW_LEN)
}

// ---------------------------------------------------------------------------
// Forest traversal helpers (shared by renderers)
// ---------------------------------------------------------------------------

/// Collect every root-to-leaf path as a Vec of node references. Used by the
/// HTML renderers to decompose a branched forest into linear conversations.
pub fn all_paths(forest: &Json) -> Vec<Vec<&Json>> {
    let mut out = Vec::new();
    let Some(trees) = forest.get("trees").and_then(|t| t.as_array()) else {
        return out;
    };
    for root in trees {
        let mut cur = Vec::new();
        walk(root, &mut cur, &mut out);
    }
    out
}

fn walk<'a>(node: &'a Json, cur: &mut Vec<&'a Json>, out: &mut Vec<Vec<&'a Json>>) {
    cur.push(node);
    let children = node.get("children").and_then(|c| c.as_array());
    let empty = children.as_ref().map_or(true, |c| c.is_empty());
    if empty {
        out.push(cur.clone());
    } else if let Some(kids) = children {
        for k in kids {
            walk(k, cur, out);
        }
    }
    cur.pop();
}

// ---------------------------------------------------------------------------
// Conversation-root key (shared by `split --by session`)
// ---------------------------------------------------------------------------

/// A stable, human-readable key identifying a record's conversation root.
///
/// `--by session` in Aperture captures cannot use the raw `session_id` field:
/// every request gets a unique session_id, so grouping by it yields one file
/// per record. Instead we group by the conversation root — identified by the
/// first *user* message (the actual question that starts the conversation),
/// not the system prompt (which is generic boilerplate shared across many
/// unrelated conversations). The grouping hash combines the system-prompt hash
/// with the first-user-message hash so two different system prompts that
/// happen to share a trivial first user message ("hi") don't collide.
pub struct ConversationRoot {
    /// Short blake3 hash identifying the conversation (16 hex chars).
    pub hash: String,
    /// Readable label derived from the first user message preview (filename-safe).
    pub label: String,
}

impl ConversationRoot {
    /// Filename key: `<label>__<hash>`. Unique and self-documenting.
    pub fn file_key(&self) -> String {
        format!("{}__{}", self.label, self.hash)
    }
}

/// Compute a record's conversation root. Returns None if the record has no
/// parseable message path (e.g. an empty-body 500).
///
/// The conversation is identified by its first *user* turn: two records share
/// a session iff they share the same first user message (and system prompt).
/// The label/title comes from that first user message, since the system prompt
/// is generic. Falls back to the system prompt (or first message) if the
/// record has no user message.
pub fn conversation_root(rec: &ciborium::Value) -> Result<Option<ConversationRoot>> {
    let path = match record_message_path(rec)? {
        Some(p) if !p.is_empty() => p,
        _ => return Ok(None),
    };
    let sys = path.iter().find(|m| m.role == "system");
    let first_user = path.iter().find(|m| m.role == "user");
    // The label comes from the first user message (the topic), falling back to
    // the system prompt / first message when there's no user turn.
    let label_msg = first_user.or(sys).unwrap_or(&path[0]);
    // The grouping hash combines system + first-user so distinct system
    // prompts with identical user messages don't merge.
    let hash = match (sys, first_user) {
        (Some(s), Some(u)) => {
            let mut h = blake3::Hasher::new();
            h.update(s.hash.as_bytes());
            h.update(u.hash.as_bytes());
            let full = h.finalize().to_hex().to_string();
            full[..HASH_HEX_LEN.min(full.len())].to_string()
        }
        _ => label_msg.hash.clone(),
    };
    // Build a filename-safe label from the preview, capped to keep filenames
    // reasonable. Spaces -> underscores, other unsafe chars stripped.
    let label: String = label_msg
        .preview
        .chars()
        .take(40)
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let label = label.trim_matches('_').to_string();
    let label = if label.is_empty() {
        "session".into()
    } else {
        label
    };
    Ok(Some(ConversationRoot { hash, label }))
}
