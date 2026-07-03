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

use crate::markdown;
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
        // CSS fallback chain: main.css is the Adium convention, but some
        // themes (e.g. Pushpin) put their base CSS in Styles/Base.css.
        // @import url("...") references inside the CSS are inlined so the
        // rendered file is self-contained (no server to resolve relative
        // URLs against).
        let main_css = read_template_opt(&res, &["main.css", "Styles/Base.css"])
            .map(|css| inline_css_imports(&css, &res))
            .unwrap_or_default();

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
        let mut expandables: Vec<String> = Vec::new();
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
            body.push_str(&self.render_path(path, records, &mut expandables));
            body.push_str("</div>\n");
        }

        let modal = expand_modal(&expandables);
        let title = format!("{} — czsplicer", self.name);
        format!(
            "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\
             <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
             <title>{title}</title>\n<style>\n{css}{variant_css}\n{modal_css}\n</style>\n</head>\n<body>\
             {body}\n{modal}\n</body>\n</html>\n",
            css = self.main_css,
            variant_css = variant_css,
            modal_css = MODAL_CSS,
            body = body,
            modal = modal,
            title = title,
        )
    }

    fn variant_css(&self, variant: Option<&str>) -> String {
        match variant {
            Some(name) => match self.variants.get(name) {
                Some(p) => match fs::read_to_string(p) {
                    Ok(css) => {
                        // Inline any relative @imports (variants commonly
                        // import "../style/foo.css"); resolve against the
                        // variant file's own directory.
                        let base = p.parent().unwrap_or_else(|| Path::new("."));
                        let inlined = inline_css_imports(&css, base);
                        format!("\n/* variant: {name} */\n{inlined}")
                    }
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
        expandables: &mut Vec<String>,
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
            let message_text = node
                .get("preview")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // Adium bubbles show a 160-char preview. When the full message is
            // longer, wrap the preview in a clickable span that opens a modal
            // with the complete message rendered as Markdown (see expand_modal).
            let full_text = node.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let text_html = if full_text.chars().count() > crate::thread::PREVIEW_LEN {
                let idx = expandables.len();
                expandables.push(markdown::to_html(full_text));
                format!(
                    "<span class=\"cz-expand\" data-cz-idx=\"{idx}\" role=\"button\" tabindex=\"0\">{}</span>",
                    escape_html(&message_text)
                )
            } else {
                escape_html(&message_text)
            };
            // For assistant turns, build tool-call/result HTML (from
            // tool_events) to append to the message. Adium themes have no
            // separate %tool_calls% keyword, so the HTML is concatenated onto
            // the (escaped) message text at substitution time. When the turn
            // has no text content (tool-only), this is the whole message.
            let mut tools_html = String::new();
            if role == "assistant" {
                let (call_rec, result_rec) = crate::render::tool_event_records(node, records);
                tools_html = tool_events_html(call_rec, result_rec);
                if tools_html.is_empty() && message_text.is_empty() {
                    // No text and no renderable tool events: keep the count
                    // placeholder so the bubble isn't blank.
                    tools_html = escape_html(&tool_placeholder(meta_rec));
                }
            }
            // Escape the text portion but keep tool HTML raw (it's already
            // escaped internally by tool_events_html).
            let message_field = if tools_html.is_empty() {
                text_html
            } else if message_text.is_empty() {
                tools_html
            } else {
                format!("{}\n\n{}", text_html, tools_html)
            };
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
                    message: &message_field,
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
    let bytes = template.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(template.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(end) = find_keyword_end(template, i) {
                let kw = &template[i + 1..end];
                if let Some(repl) = resolve_keyword(kw, s) {
                    out.extend_from_slice(repl.as_bytes());
                    i = end + 1;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Styles for the expandable-message modal. Theme-agnostic (system colors) so
/// it stays readable over any Adium bubble palette.
const MODAL_CSS: &str = r#"
.cz-expand{cursor:pointer;text-decoration:underline dotted #888}
.cz-expand:hover{background:rgba(127,127,127,.12)}
.cz-full{display:none}
#cz-modal{position:fixed;inset:0;display:none;z-index:9999}
#cz-modal.cz-open{display:block}
#cz-modal .cz-backdrop{position:fixed;inset:0;background:rgba(0,0,0,.5)}
#cz-modal .cz-panel{position:relative;background:Canvas;color:CanvasText;max-width:48rem;width:calc(100% - 2rem);max-height:85vh;overflow:auto;margin:3rem auto;border-radius:8px;padding:1.5rem 2rem;box-shadow:0 8px 32px rgba(0,0,0,.3)}
#cz-modal .cz-close{position:absolute;top:.4rem;right:.6rem;font-size:1.4rem;line-height:1;cursor:pointer;border:none;background:none;color:inherit;padding:.25rem .5rem}
#cz-modal-body pre{background:rgba(127,127,127,.1);padding:.5rem;overflow:auto;border-radius:4px}
#cz-modal-body code{font-family:ui-monospace,SFMono-Regular,Menlo,monospace}
"#;

/// Hidden full-content divs (one per expandable) plus the modal shell and the
/// JS that wires `.cz-expand` clicks to open the matching content. The divs
/// carry Markdown-rendered HTML produced by `markdown::to_html`.
fn expand_modal(expandables: &[String]) -> String {
    let mut out = String::new();
    for (i, html) in expandables.iter().enumerate() {
        out.push_str(&format!(
            "<div class=\"cz-full\" id=\"cz-full-{i}\">{html}</div>\n"
        ));
    }
    out.push_str(
        r#"<div id="cz-modal"><div class="cz-backdrop"></div><div class="cz-panel"><button class="cz-close" aria-label="Close">&times;</button><div id="cz-modal-body"></div></div></div>
<script>
(function(){
var m=document.getElementById('cz-modal');if(!m)return;
var body=document.getElementById('cz-modal-body');
function open(i){var s=document.getElementById('cz-full-'+i);if(s){body.innerHTML=s.innerHTML;m.classList.add('cz-open');}}
function close(){m.classList.remove('cz-open');body.innerHTML='';}
m.querySelector('.cz-backdrop').addEventListener('click',close);
m.querySelector('.cz-close').addEventListener('click',close);
document.addEventListener('keydown',function(e){if(e.key==='Escape')close();});
document.querySelectorAll('.cz-expand').forEach(function(el){
var i=el.getAttribute('data-cz-idx');
el.addEventListener('click',function(){open(i);});
el.addEventListener('keydown',function(e){if(e.key==='Enter'||e.key===' '){e.preventDefault();open(i);}});
});
})();
</script>
"#,
    );
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
        | "userIcons"
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
        // Case-insensitive fallback: many real themes ship mixed-case paths
        // (e.g. "Pretty Simple" uses Main.css; Fluffy uses incoming/Content.html
        // with a lowercase "incoming/"). Walk every path segment
        // case-insensitively.
        if let Some(content) = read_case_insensitive(res, rel) {
            return Some(content);
        }
    }
    None
}

/// Resolve `rel` (a forward-slash relative path like "Incoming/Content.html")
/// against `base` case-insensitively on every segment, returning file contents
/// on a match. Returns None if any segment has no case-insensitive match or the
/// final path isn't a file.
fn read_case_insensitive(base: &Path, rel: &str) -> Option<String> {
    let mut cur = base.to_path_buf();
    for seg in rel.split('/') {
        let entries = fs::read_dir(&cur).ok()?;
        let hit = entries
            .flatten()
            .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(seg))?;
        cur = hit.path();
    }
    if cur.is_file() {
        fs::read_to_string(&cur)
            .with_context(|| format!("reading {}", cur.display()))
            .ok()
    } else {
        None
    }
}

/// Recursively inline relative `@import url("...")` references in a CSS
/// string, so the rendered HTML is self-contained. Relative paths resolve
/// against `base_dir` (the theme's `Contents/Resources`). Absolute/remote
/// URLs (http://, https://, data:) are left untouched — many themes phone
/// home to update-check URLs that are long dead, and we don't want to fetch.
///
/// Guards against cycles with a small visit cap (8 levels).
fn inline_css_imports(css: &str, base_dir: &Path) -> String {
    inline_css_imports_inner(css, base_dir, 0)
}

fn inline_css_imports_inner(css: &str, base_dir: &Path, depth: usize) -> String {
    if depth >= 8 {
        return css.to_string();
    }
    // Match: @import [url(]["']path["'][)];
    let re = regex::Regex::new(r#"(?i)@import\s+(?:url\(\s*)?["']([^"']+)["']\s*\)?\s*;"#).unwrap();
    re.replace_all(css, |caps: &regex::Captures| {
        let target = &caps[1];
        // Skip absolute URLs — leave them as-is (or effectively drop, since
        // browsers won't fetch http:// update URLs from a local file either).
        if target.starts_with("http://")
            || target.starts_with("https://")
            || target.starts_with("//")
            || target.starts_with("data:")
        {
            // Replace with a comment so the line doesn't break parsing.
            return format!("/* @import skipped (remote): {} */", target);
        }
        // Resolve relative to base_dir. Normalize "./foo" and "foo".
        let cleaned = target.trim_start_matches("./").trim_start_matches(".\\");
        let path = base_dir.join(cleaned);
        if path.is_file() {
            if let Ok(inner) = fs::read_to_string(&path) {
                // Variants live in Variants/ and import "../style/foo.css";
                // resolve nested imports against the importing file's dir.
                let nested_base = path
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| base_dir.to_path_buf());
                return inline_css_imports_inner(&inner, &nested_base, depth + 1);
            }
        }
        // Unresolvable: replace with a comment rather than leave the @import.
        format!("/* @import skipped (not found): {} */", target)
    })
    .into_owned()
}

/// Render a record's tool events as HTML `<details>` blocks (calls then
/// results), mirroring the built-in renderer. Appended to the message text
/// for assistant turns. Returns empty if no renderable events.
fn tool_events_html(call_rec: Option<&Json>, result_rec: Option<&Json>) -> String {
    use crate::render::escape_html;
    let mut out = String::new();
    if let Some(events) = call_rec
        .and_then(|r| r.get("tool_events"))
        .and_then(|v| v.as_array())
    {
        for ev in events {
            if ev.get("kind").and_then(|v| v.as_str()) == Some("call") {
                let name = ev.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
                let input = ev.get("input").and_then(|v| v.as_str()).unwrap_or("");
                out.push_str(&format!(
                    "<details class=\"tool-call\"><summary>tool call: <code>{}</code></summary><pre class=\"tool-input\">{}</pre></details>",
                    escape_html(name),
                    escape_html(input)
                ));
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
                    out.push_str(&format!(
                        "<details class=\"tool-result\"><summary>tool result</summary><pre class=\"tool-output\">{}</pre></details>",
                        escape_html(content)
                    ));
                }
            }
        }
    }
    out
}

/// When an assistant message has no text content and no renderable tool
/// events, produce a count placeholder instead of an empty bubble.
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
