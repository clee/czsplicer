//! Shared redaction presets and regex compilation.
//!
//! Single source of truth for the canned secret-shape patterns used by:
//! - `edit --redact-preset` (mutating scrub of capture bodies / whole records),
//! - `thread`/`report` redaction (mutating scrub before rendering),
//! - `secrets` safety net (read-only detection over rendered bytes).
//!
//! All three consume the same `REDACT_PRESETS` table so a pattern added here is
//! immediately available to every path. The redaction invariant (this program
//! never silently mutates raw CBOR except on explicit `edit`/redact paths) is
//! preserved: the detector in `secrets.rs` reads only, and mutating callers
//! invoke `compile_redact_regexes` explicitly.

use anyhow::{anyhow, Result};

/// Named canned regex patterns: `(name, regex, human description)`.
pub const REDACT_PRESETS: &[(&str, &str, &str)] = &[
    (
        "email",
        r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b",
        "email addresses",
    ),
    (
        "jwt",
        r"eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+",
        "JSON Web Tokens",
    ),
    (
        "apikey",
        r"sk-[A-Za-z0-9]{20,}|sk-ant-[A-Za-z0-9_-]{20,}|xai-[A-Za-z0-9]{20,}",
        "API keys (OpenAI/Anthropic/xAI)",
    ),
    ("bearer", r"(?i:bearer\s+[A-Za-z0-9._-]+)", "Bearer tokens"),
    ("aws", r"AKIA[0-9A-Z]{16}", "AWS access key IDs"),
    (
        "secretkey",
        r#"(?i)secret(?:\s+access)?\s+key\b[*`:='"\s]*[A-Za-z0-9/+=]{20,}"#,
        "Labeled secret access keys (any 'Secret key:' / 'Secret access key:' block)",
    ),
    ("ipv4", r"\b(?:\d{1,3}\.){3}\d{1,3}\b", "IPv4 addresses"),
    (
        "uuid",
        r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b",
        "UUIDs",
    ),
    (
        "creditcard",
        r"\b(?:\d[ -]?){13,16}\b",
        "credit card numbers",
    ),
    (
        "ssn",
        r"\b\d{3}-\d{2}-\d{4}\b",
        "US Social Security numbers",
    ),
];

/// Expand preset names into regex patterns. `all` expands to every preset.
pub fn expand_presets(names: &[String]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for name in names {
        if name == "all" {
            for (_, pat, _) in REDACT_PRESETS {
                out.push((*pat).to_string());
            }
            continue;
        }
        match REDACT_PRESETS.iter().find(|(n, _, _)| n == name) {
            Some((_, pat, _)) => out.push(pat.to_string()),
            None => {
                let valid: Vec<&str> = REDACT_PRESETS.iter().map(|(n, _, _)| *n).collect();
                return Err(anyhow!(
                    "unknown redact preset `{name}`; valid: {}, all",
                    valid.join(", ")
                ));
            }
        }
    }
    Ok(out)
}

/// Merge explicit `--redact` patterns with expanded `--redact-preset` names,
/// drop empties (an empty regex would match everywhere and redact the whole
/// body), and compile each into a `regex::Regex`.
pub fn compile_redact_regexes(redact: &[String], presets: &[String]) -> Result<Vec<regex::Regex>> {
    let mut all_patterns = redact.to_vec();
    all_patterns.extend(expand_presets(presets)?);
    all_patterns.retain(|p| !p.is_empty());
    all_patterns
        .iter()
        .map(|p| regex::Regex::new(p))
        .collect::<Result<_, _>>()
        .map_err(|e| anyhow!("invalid redact regex: {e}"))
}
