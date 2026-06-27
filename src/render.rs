//! Shared string utilities for the conversation-thread code paths.
//!
//! `truncate` builds compact preview labels from message content. The HTML
//! renderers (added later) collect the rest of their helpers here too, so the
//! renderers and the thread/preview paths share one implementation.

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
