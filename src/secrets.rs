//! Secrets-safety net: a detection-only heuristic that warns when
//! human-readable output (HTML, Markdown) is emitted *without* `--redact*`
//! yet appears to contain likely secrets.
//!
//! This is NOT a guarantee — it runs the high-precision subset of
//! `REDACT_PRESETS` (the low-false-positive secret shapes: API keys, bearer
//! tokens, JWTs, AWS keys, credit cards, SSNs) as detectors over the rendered
//! bytes. Noisy patterns (email, IPv4, UUID) are deliberately excluded: they
//! appear routinely in legitimate metadata and would cry wolf. A custom token
//! shape the presets don't cover will sail through unflagged; the warning text
//! says so explicitly so it never creates false confidence.
//!
//! Detection never mutates output. The redaction invariant (see AGENTS.md,
//! "Critical invariants" §2) is preserved: this module reads only.

use regex::Regex;
use std::sync::OnceLock;

/// High-precision preset names treated as "likely secrets" for the warning.
/// Deliberately excludes email / ipv4 / uuid (routine in metadata → false
/// positives). Kept as names so the patterns stay sourced from
/// `REDACT_PRESETS` (single source of truth).
const LIKELY_SECRET_PRESETS: &[&str] = &[
    "jwt",
    "apikey",
    "bearer",
    "aws",
    "secretkey",
    "creditcard",
    "ssn",
];

/// Compiled (preset-name, regex) set, built once per process.
static SECRET_REGEXES: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();

fn secret_regexes() -> &'static Vec<(&'static str, Regex)> {
    SECRET_REGEXES.get_or_init(|| {
        let mut out = Vec::new();
        for &name in LIKELY_SECRET_PRESETS {
            if let Some((_, pat, _)) = crate::redact::REDACT_PRESETS
                .iter()
                .find(|(n, _, _)| *n == name)
            {
                if let Ok(re) = Regex::new(pat) {
                    out.push((name, re));
                }
            }
        }
        out
    })
}

/// Result of scanning rendered output for likely secrets.
#[derive(Default)]
pub struct Report {
    /// (preset_name, hit_count) for each preset with ≥1 hit, in preset order.
    pub hits: Vec<(&'static str, usize)>,
}

impl Report {
    pub fn total_hits(&self) -> usize {
        self.hits.iter().map(|(_, n)| *n).sum()
    }

    pub fn is_clean(&self) -> bool {
        self.hits.is_empty()
    }

    /// Comma-separated list of preset names that fired with counts, e.g. "bearer×1, apikey×2".
    fn types(&self) -> String {
        self.hits
            .iter()
            .map(|(name, n)| format!("{name}×{n}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The stderr warning text (no trailing newline). Returns None if clean.
    pub fn warning(&self) -> Option<String> {
        if self.is_clean() {
            return None;
        }
        Some(format!(
            "warning: output may contain un-redacted secrets; pattern-based check found {} likely hit(s) across {} type(s) ({}). \
             This is a best-effort heuristic — custom token shapes are NOT caught. \
             Re-run with `--redact-preset all` to scrub, or pass `--i-know` to suppress this warning.",
            self.total_hits(),
            self.hits.len(),
            self.types()
        ))
    }
}

/// Scan rendered output text for likely-secret patterns. Reads only; never
/// mutates. The `text` should be the final bytes about to be written.
pub fn scan(text: &str) -> Report {
    let mut hits = Vec::new();
    for (name, re) in secret_regexes() {
        // The creditcard preset matches any 13-16 digit run, which includes hex
        // dumps, zero-runs, and UUID fragments. Require a valid Luhn checksum
        // so the warning doesn't cry wolf on those (real card numbers are
        // Luhn-valid); other presets are counted as-is.
        let n = if *name == "creditcard" {
            re.find_iter(text)
                .filter(|m| luhn_ok(&m.as_str().replace([' ', '-'], "")))
                .count()
        } else {
            re.find_iter(text).count()
        };
        if n > 0 {
            hits.push((*name, n));
        }
    }
    Report { hits }
}

/// Luhn (mod-10) checksum over a digit string. Returns true for valid card
/// numbers and false for the digit-only fragments the bare creditcard regex also
/// matches (e.g. "3030303030303030", "1111111111111111").
fn luhn_ok(digits: &str) -> bool {
    let mut sum = 0u32;
    let mut dbl = false;
    for b in digits.bytes().rev() {
        if !b.is_ascii_digit() {
            return false;
        }
        let mut d = (b - b'0') as u32;
        if dbl {
            d *= 2;
            if d > 9 {
                d -= 9;
            }
        }
        sum += d;
        dbl = !dbl;
    }
    !digits.is_empty() && sum % 10 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_text_reports_nothing() {
        let r = scan("just a normal conversation about rust and cookies");
        assert!(r.is_clean());
        assert!(r.warning().is_none());
    }

    #[test]
    fn detects_openai_api_key() {
        let r = scan("Authorization: sk-abcdefghijklmnopqrstuvwxyz1234567890");
        assert!(r.hits.iter().any(|(n, _)| *n == "apikey"));
        assert!(r.warning().is_some());
    }

    #[test]
    fn detects_bearer_token() {
        let r = scan("bearer eyJhbGc.somepayload.sig");
        assert!(r.hits.iter().any(|(n, _)| *n == "bearer"));
    }

    #[test]
    fn detects_jwt() {
        let r = scan("eyJhbGci.eyJzdWIi.e30signature");
        assert!(r.hits.iter().any(|(n, _)| *n == "jwt"));
    }

    #[test]
    fn detects_aws_key() {
        let r = scan("role arn with AKIAIOSFODNN7EXAMPLE key");
        assert!(r.hits.iter().any(|(n, _)| *n == "aws"));
    }

    #[test]
    fn detects_ssn() {
        let r = scan("ssn on file: 123-45-6789");
        assert!(r.hits.iter().any(|(n, _)| *n == "ssn"));
    }

    #[test]
    fn ignores_routine_metadata() {
        // Email, IPv4, UUID appear in normal logs and must NOT trigger the
        // secret warning (they're excluded from LIKELY_SECRET_PRESETS).
        let r =
            scan("from user@example.com at 192.168.1.1 (id 550e8400-e29b-41d4-a716-446655440000)");
        assert!(
            r.is_clean(),
            "routine metadata should not warn: {:?}",
            r.hits
        );
    }

    #[test]
    fn counts_multiple_hits() {
        let r = scan(
            "sk-abcdefghijklmnopqrstuvwxyz1234567890 and sk-abcdefghijklmnopqrstuvwxyz0987654321",
        );
        assert_eq!(r.total_hits(), 2);
    }

    #[test]
    fn warning_lists_types_and_counts() {
        let r = scan("sk-abcdefghijklmnopqrstuvwxyz1234567890");
        let w = r.warning().unwrap();
        assert!(
            w.contains("apikey×1"),
            "warning should name type+count: {w}"
        );
        assert!(
            w.contains("best-effort heuristic"),
            "warning must disclaim: {w}"
        );
        assert!(w.contains("--i-know"), "warning must mention opt-out: {w}");
    }

    #[test]
    fn creditcard_requires_luhn_checksum() {
        // Hex/UUID fragments match the bare 13-16 digit regex but fail Luhn:
        // they must NOT trigger the warning.
        let r = scan("card: 3030 3030 3030 3030");
        assert!(
            !r.hits.iter().any(|(n, _)| *n == "creditcard"),
            "non-Luhn digit run should not warn: {:?}",
            r.hits
        );
        // A Luhn-valid test card number should be flagged.
        let r2 = scan("card: 4111 1111 1111 1111");
        assert!(
            r2.hits.iter().any(|(n, _)| *n == "creditcard"),
            "Luhn-valid card should warn: {:?}",
            r2.hits
        );
    }
}
