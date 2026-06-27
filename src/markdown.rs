//! Minimal, safe Markdown → HTML subset for the mailbox/HTML renderers.
//!
//! Deliberately tiny: we only render the constructs that appear in LLM message
//! content. No raw HTML pass-through (everything is escaped), no nested parsing
//! beyond the constructs below. The goal is "readable in an email client", not
//! a spec-complete CommonMark implementation.
//!
//! Supported:
//!   - fenced code blocks ```lang ... ```
//!   - paragraphs (blank-line separated)
//!   - inline: `code`, **bold**, *italic*
//!
//! Everything else passes through as escaped text.

use crate::render::escape_html;
use std::fmt::Write;

/// Render a markdown source string to an HTML fragment (no surrounding tags).
pub fn to_html(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + 64);
    let mut lines = src.lines().peekable();
    let mut para: Vec<&str> = Vec::new();

    // Flush accumulated paragraph lines as a single <p>.
    let flush_para = |para: &mut Vec<&str>, out: &mut String| {
        if para.is_empty() {
            return;
        }
        out.push_str("<p>");
        let joined = para.join("\n");
        out.push_str(&render_inline(&joined));
        out.push_str("</p>\n");
        para.clear();
    };

    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("```") {
            flush_para(&mut para, &mut out);
            let lang = rest.trim();
            if lang.is_empty() {
                out.push_str("<pre><code>");
            } else {
                let _ = write!(out, "<pre><code class=\"language-{lang}\">");
            }
            // Collect until closing fence.
            let mut body = String::new();
            let mut closed = false;
            for inner in lines.by_ref() {
                if inner.trim_start().starts_with("```") {
                    closed = true;
                    break;
                }
                body.push_str(inner);
                body.push('\n');
            }
            out.push_str(&escape_html(&body));
            out.push_str("</code></pre>\n");
            if !closed {
                break;
            }
            continue;
        }
        if trimmed.is_empty() {
            flush_para(&mut para, &mut out);
            continue;
        }
        para.push(line);
    }
    flush_para(&mut para, &mut out);
    out
}

/// Render inline constructs (code, bold, italic) with HTML escaping.
fn render_inline(s: &str) -> String {
    // First escape, then apply inline markers on the escaped text. Because the
    // markers (`*`, `` ` ``) are not HTML-special they survive escaping intact.
    let esc = escape_html(s);
    let mut out = String::with_capacity(esc.len());
    let bytes = esc.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'`' {
            if let Some(end) = find_byte(&esc[i + 1..], b'`') {
                out.push_str("<code>");
                out.push_str(&esc[i + 1..i + 1 + end]);
                out.push_str("</code>");
                i += 2 + end;
                continue;
            }
        }
        if b == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            if let Some(end) = find_subseq(&esc[i + 2..], "**") {
                out.push_str("<strong>");
                out.push_str(&esc[i + 2..i + 2 + end]);
                out.push_str("</strong>");
                i += 4 + end;
                continue;
            }
        }
        if b == b'*' {
            if let Some(end) = find_byte(&esc[i + 1..], b'*') {
                out.push_str("<em>");
                out.push_str(&esc[i + 1..i + 1 + end]);
                out.push_str("</em>");
                i += 2 + end;
                continue;
            }
        }
        // Advance one full UTF-8 character so multi-byte sequences
        // (em-dash, curly quotes, etc.) are preserved, not byte-wise
        // reinterpreted as Latin-1.
        let ch_len = utf8_len(b);
        out.push_str(&esc[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// Return the length of the UTF-8 sequence starting with byte `b`.
/// (ASCII => 1, leading byte => 2..=4). Falls back to 1 for invalid
/// continuation bytes, which cannot occur inside a valid Rust &str.
fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else if b >> 3 == 0b11110 {
        4
    } else {
        1
    }
}

fn find_byte(s: &str, b: u8) -> Option<usize> {
    s.as_bytes().iter().position(|&x| x == b)
}

fn find_subseq(s: &str, pat: &str) -> Option<usize> {
    s.find(pat)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_raw_html() {
        assert_eq!(
            to_html("<script>x</script>"),
            "<p>&lt;script&gt;x&lt;/script&gt;</p>\n"
        );
    }

    #[test]
    fn renders_inline() {
        let h = to_html("a `code` **b** *c*");
        assert!(h.contains("<code>code</code>"));
        assert!(h.contains("<strong>b</strong>"));
        assert!(h.contains("<em>c</em>"));
    }

    #[test]
    fn renders_code_fence() {
        let h = to_html("```\nlet x = 1;\n```");
        assert!(h.contains("<pre><code>let x = 1;\n</code></pre>"));
        assert!(!h.contains("<p>"));
    }
}
