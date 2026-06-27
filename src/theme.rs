//! Adium message-style (`.AdiumMessageStyle`) loader and renderer.
//!
//! This is a de-risking spike: it supports the *static substitution* model
//! (the `%keyword%` template system documented by Adium/Thunderbird), not the
//! classic WebKit `appendMessage` JS-coalescing model. Themes that rely on
//! `Template.html` JS functions won't render perfectly here — consecutive
//! messages are coalesced by selecting `NextContent.html` instead.
//!
//! Bundle layout we read:
//!   Foo.AdiumMessageStyle/Contents/
//!     Info.plist
//!     Resources/
//!       main.css
//!       Content.html                 (generic fallback)
//!       Incoming/{Content,NextContent}.html
//!       Outgoing/{Content,NextContent}.html
//!       Status.html
//!       Variants/<Name>.css
//!
//! Each Content/NextContent/Status template MUST contain `id="insert"` (where
//! consecutive messages attach). We honor the fallback chain: NextContent ->
//! Content; Outgoing/* -> Incoming/*; anything missing -> generic Content.

use crate::render::{best_record_id, escape_html, model_of, sender_color};
use anyhow::{anyhow, Context, Result};
use serde_json::Value as Json;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// A loaded Adium message-style bundle.
pub struct Theme {
    name: String,
    main_css: String,
    incoming_content: String,
    incoming_next: Option<String>,
    outgoing_content: String,
    outgoing_next: Option<String>,
    #[allow(dead_code)]
    status: Option<String>,
    variants: BTreeMap<String, PathBuf>,
}

impl Theme {
    /// Load and resolve a `.AdiumMessageStyle` bundle from disk.
    pub fn load(bundle: &Path) -> Result<Self> {
        let res = bundle.join("Contents/Resources");
        if !res.is_dir() {
            return Err(anyhow!(
                "not an Adium message style: missing Contents/Resources at {}",
                bundle.display()
            ));
        }
        let name = bundle
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.trim_end_matches(".AdiumMessageStyle").to_string())
            .unwrap_or_else(|| "Theme".into());

        // Fallback chain: per-direction content -> generic Content.
        let incoming_content = read_template(&res, &["Incoming/Content.html", "Content.html"])?;
        let outgoing_content = read_template(
            &res,
            &[
                "Outgoing/Content.html",
                "Incoming/Content.html",
                "Content.html",
            ],
        )?;
        let incoming_next = read_template_opt(&res, &["Incoming/NextContent.html"]);
        let outgoing_next = read_template_opt(
            &res,
            &["Outgoing/NextContent.html", "Incoming/NextContent.html"],
        );
        let status = read_template_opt(&res, &["Status.html"]);
        let main_css = read_template_opt(&res, &["main.css"]).unwrap_or_default();

        // Variants: <Name>.css under Variants/, keyed by stem.
        let mut variants = BTreeMap::new();
        let vdir = res.join("Variants");
        if vdir.is_dir() {
            for entry in fs::read_dir(&vdir)? {
                let p = entry?.path();
                if p.extension().and_then(|e| e.to_str()) == Some("css") {
                    if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                        variants.insert(stem.to_string(), p);
                    }
                }
            }
        }

        Ok(Self {
            name,
            main_css,
            incoming_content,
            incoming_next,
            outgoing_content,
            outgoing_next,
            status,
            variants,
        })
    }

    #[allow(dead_code)]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[allow(dead_code)]
    pub fn variants(&self) -> Vec<&str> {
        self.variants.keys().map(|s| s.as_str()).collect()
    }

    /// Render the whole forest as a self-contained HTML document. Every
    /// root-to-leaf path becomes one chat section (branches decomposed into
    /// linear conversations the Adium model can style).
    pub fn render_html(&self, forest: &Json, variant: Option<&str>) -> String {
        let variant_css = self.variant_css(variant);
        let records = forest.get("records").and_then(|r| r.as_object());

        let mut body = String::new();
        let paths = crate::thread::all_paths(forest);
        for (idx, path) in paths.iter().enumerate() {
            if paths.len() > 1 {
                body.push_str(&format!(
                    "<div class=\"path-separator\">path {} of {}: {} messages</div>\n",
                    idx + 1,
                    paths.len(),
                    path.len()
                ));
            }
            body.push_str("<div id=\"Chat\">\n");
            body.push_str(&self.render_path(path, records));
            body.push_str("</div>\n");
        }

        let title = format!("{} — czsplicer", self.name);
        format!(
            "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
             <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n\
             <title>{title}</title>\n<style>\n{css}{variant_css}\n</style>\n</head>\n<body>\n\
             {body}\n</body>\n</html>\n",
            css = self.main_css,
            variant_css = variant_css,
            body = body,
            title = title,
        )
    }

    fn variant_css(&self, variant: Option<&str>) -> String {
        match variant {
            Some(name) => match self.variants.get(name) {
                Some(p) => match fs::read_to_string(p) {
                    Ok(css) => format!("\n/* variant: {name} */\n{css}"),
                    Err(e) => format!("\n/* failed to read variant {name}: {e} */\n"),
                },
                None => {
                    let avail: Vec<&str> = self.variants.keys().map(|s| s.as_str()).collect();
                    format!(
                        "\n/* unknown variant {name}; available: {} */\n",
                        avail.join(", ")
                    )
                }
            },
            None => String::new(),
        }
    }

    fn render_path(
        &self,
        path: &[&Json],
        records: Option<&serde_json::Map<String, Json>>,
    ) -> String {
        let mut out = String::new();
        let mut prev_sender: Option<String> = None;
        for (i, node) in path.iter().enumerate() {
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
            let meta_rec =
                meta_rid.and_then(|id| records.and_then(|rmap| rmap.get(&id.to_string())));

            let role = node.get("role").and_then(|v| v.as_str()).unwrap_or("");
            let (sender, direction) = match role {
                "user" => ("you".to_string(), "outgoing"),
                "assistant" => (
                    model_of(meta_rec).unwrap_or_else(|| "assistant".into()),
                    "incoming",
                ),
                "system" => ("system".to_string(), "incoming"),
                _ => (role.to_string(), "incoming"),
            };
            let consecutive = prev_sender.as_deref() == Some(sender.as_str());
            let mut message = node
                .get("preview")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // When the assistant only issued tool calls, the preview/content
            // is empty. Show a placeholder instead of a blank bubble.
            if message.is_empty() && role == "assistant" {
                message = tool_placeholder(meta_rec);
            }
            let time = node_time(meta_rec);

            // Pick template: NextContent for consecutive, else Content.
            let (template, has_next) = match direction {
                "outgoing" => match &self.outgoing_next {
                    Some(t) if consecutive => (t.as_str(), true),
                    _ => (self.outgoing_content.as_str(), false),
                },
                _ => match &self.incoming_next {
                    Some(t) if consecutive => (t.as_str(), true),
                    _ => (self.incoming_content.as_str(), false),
                },
            };
            let _ = has_next; // NextContent selection is the only coalescing we do for now.

            let classes = build_message_classes(direction, consecutive);
            let color = sender_color(&sender);
            let substituted = substitute(
                template,
                &Subst {
                    message: &escape_html(&message),
                    sender: &escape_html(&sender),
                    sender_color: color,
                    time: &time,
                    message_classes: &classes,
                    service: "",
                },
            );
            out.push_str(&substituted);
            out.push('\n');
            prev_sender = Some(sender);
        }
        out
    }
}

/// Arguments bound for a single `%keyword%` substitution pass.
struct Subst<'a> {
    message: &'a str,
    sender: &'a str,
    sender_color: &'a str,
    time: &'a str,
    message_classes: &'a str,
    service: &'a str,
}

/// Single-pass `%keyword%` substitution. Unknown keywords are left untouched.
/// Format-string keywords (`%time{...}%`, `%textbackgroundcolor{N}%`) are
/// recognized but the payload is currently ignored — handled as their base
/// keyword. (Spike scope.)
fn substitute(template: &str, s: &Subst) -> String {
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(end) = find_keyword_end(template, i) {
                let kw = &template[i + 1..end];
                if let Some(repl) = resolve_keyword(kw, s) {
                    out.push_str(repl);
                    i = end + 1;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Find the index of the closing `%` of a keyword starting at `i` (the opening
/// `%`). A keyword may contain a `{...}` format payload. Returns None if no
/// closing `%` is found before a word boundary or end of string.
fn find_keyword_end(template: &str, start: usize) -> Option<usize> {
    let bytes = template.as_bytes();
    let mut j = start + 1;
    let mut in_braces = false;
    while j < bytes.len() {
        let c = bytes[j];
        match c {
            b'{' => in_braces = true,
            b'}' => in_braces = false,
            b'%' if !in_braces => return Some(j),
            // Identifier chars (braces handled above).
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'.' | b'-' => {}
            _ => return None,
        }
        j += 1;
    }
    None
}

fn resolve_keyword<'a>(kw: &str, s: &'a Subst) -> Option<&'a str> {
    // Strip a `{...}` payload if present, e.g. `time{HH:mm}` -> `time`.
    let base = kw.split_once('{').map(|(b, _)| b).unwrap_or(kw);
    match base {
        "message" => Some(s.message),
        "sender" | "senderDisplayName" => Some(s.sender),
        "senderColor" => Some(s.sender_color),
        "senderScreenName" => Some(s.sender),
        "time" | "shortTime" | "date" => Some(s.time),
        "messageClasses" => Some(s.message_classes),
        "service" => Some(s.service),
        // Keywords we recognize but don't populate in the spike.
        "userIconPath"
        | "senderStatusIcon"
        | "messageDirection"
        | "variant"
        | "textbackgroundcolor"
        | "status" => Some(""),
        _ => None,
    }
}

/// Build the `%messageClasses%` string: the message type + direction +
/// consecutive flag, matching Adium's documented class vocabulary.
fn build_message_classes(direction: &str, consecutive: bool) -> String {
    let mut v = vec!["message".to_string(), direction.to_string()];
    if consecutive {
        v.push("consecutive".to_string());
    }
    v.join(" ")
}

/// Read the first existing template from a fallback chain. Errors if none.
fn read_template(res: &Path, chain: &[&str]) -> Result<String> {
    read_template_opt(res, chain).ok_or_else(|| {
        anyhow!(
            "no template in chain [{}] under {}",
            chain.join(", "),
            res.display()
        )
    })
}

fn read_template_opt(res: &Path, chain: &[&str]) -> Option<String> {
    for rel in chain {
        let p = res.join(rel);
        if p.is_file() {
            return fs::read_to_string(&p)
                .with_context(|| format!("reading {}", p.display()))
                .ok();
        }
    }
    None
}

/// When an assistant message has no text content (e.g. it only issued tool
/// calls), produce a placeholder string instead of an empty bubble.
fn tool_placeholder(rec: Option<&Json>) -> String {
    let Some(rec) = rec else {
        return String::new();
    };
    let tc = rec.get("tool_calls").and_then(|v| v.as_u64()).unwrap_or(0);
    let tr = rec
        .get("tool_results")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let mut parts = Vec::new();
    if tc > 0 {
        parts.push(format!("{} tool call(s)", tc));
    }
    if tr > 0 {
        parts.push(format!("{} tool result(s)", tr));
    }
    if parts.is_empty() {
        String::new()
    } else {
        parts.join(", ")
    }
}

fn node_time(rec: Option<&Json>) -> String {
    let Some(rec) = rec else {
        return String::new();
    };
    rec.get("timestamp")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// Convenience for `cmd_thread`: render an already-built forest to themed HTML.
/// Path decomposition uses `crate::thread::all_paths`.
pub fn render_forest(forest: &Json, theme_path: &Path, variant: Option<&str>) -> Result<String> {
    let theme = Theme::load(theme_path)?;
    Ok(theme.render_html(forest, variant))
}
