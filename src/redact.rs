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
use regex::Regex;
use std::borrow::Cow;

/// Clap-shared redaction flags. Flattened into `EditArgs`, `ReportArgs`, and
/// `ThreadArgs` so the three commands stay in sync. `--all-strings` (edit) and
/// `--i-know` (report/thread) stay on their own structs — they are not shared.
#[derive(clap::Args, Default)]
pub struct RedactArgs {
    /// Regex to redact from string bodies; matches replaced with the replacement.
    /// Repeatable.
    #[arg(long, value_name = "REGEX")]
    pub redact: Vec<String>,
    /// Redact using a named preset: email, jwt, apikey, bearer, aws, ipv4,
    /// uuid, creditcard, ssn, or all. Repeatable. Combines with --redact.
    #[arg(long = "redact-preset", value_name = "PRESET")]
    pub redact_presets: Vec<String>,
    /// Replacement text for redacted matches (default "[REDACTED]").
    #[arg(long, value_name = "TOKEN", default_value = "[REDACTED]")]
    pub redact_replacement: String,
}

impl RedactArgs {
    /// Compile into a `Redactor`. Empty when no `--redact`/`--redact-preset`
    /// was given (callers check `is_active()` before scrubbing).
    pub fn build(&self) -> Result<Redactor> {
        Redactor::new(&self.redact, &self.redact_presets, &self.redact_replacement)
    }
}

/// Compiled redaction set: regexes + replacement. The closure body that was
/// duplicated across `cmd_edit`/`cmd_report`/`cmd_thread` lives here once.
pub struct Redactor {
    regexes: Vec<Regex>,
    replacement: String,
}

impl Redactor {
    pub fn new(redact: &[String], presets: &[String], replacement: &str) -> Result<Self> {
        Ok(Self {
            regexes: compile_redact_regexes(redact, presets)?,
            replacement: replacement.to_string(),
        })
    }

    /// True when at least one regex was compiled (i.e. redaction will fire).
    pub fn is_active(&self) -> bool {
        !self.regexes.is_empty()
    }

    /// Apply every regex in order, replacing matches with the replacement.
    /// Only allocates when a regex actually matches: `is_match` guards each
    /// regex so a no-match never copies the string (it flows through as a
    /// borrowed `Cow` instead of being copied once per regex).
    pub fn apply(&self, s: &str) -> String {
        let mut cur: Cow<str> = Cow::Borrowed(s);
        for r in &self.regexes {
            if r.is_match(&cur) {
                cur = Cow::Owned(r.replace_all(&cur, self.replacement.as_str()).into_owned());
            }
        }
        cur.into_owned()
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_no_match_returns_input_unchanged() {
        // No regex matches: the input must pass through byte-for-byte without
        // the per-regex copy the old `to_string()`-on-every-iteration did.
        let r = Redactor::new(&["zzzznomatch".into()], &[], "[REDACTED]").unwrap();
        let s = "nothing to see here, move along";
        assert_eq!(r.apply(s), s);
    }

    #[test]
    fn apply_replaces_matches_and_leaves_rest() {
        let r = Redactor::new(&[r"sk-[A-Za-z0-9]{4}".into()], &[], "[REDACTED]").unwrap();
        assert_eq!(r.apply("key sk-abcd here"), "key [REDACTED] here");
        // A string the regex doesn't match is returned verbatim.
        assert_eq!(r.apply("no keys at all"), "no keys at all");
    }

    #[test]
    fn apply_chains_multiple_regexes() {
        let r = Redactor::new(
            &[r"sk-[A-Za-z0-9]{4}".into(), r"AKIA[0-9A-Z]{4}".into()],
            &[],
            "[X]",
        )
        .unwrap();
        assert_eq!(r.apply("sk-abcd and AKIA1234"), "[X] and [X]");
    }
}
