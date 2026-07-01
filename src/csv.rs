//! Minimal RFC 4180 CSV writer (no external dependency).
//!
//! One helper: [`row`] renders a slice of fields as a single CSV line (trailing
//! `\n`, no trailing carriage return — consistent with the rest of the CLI's
//! `\n`-only output). Fields needing quoting (comma, `"`, `\n`, `\r`) are
//! wrapped in `"..."` with embedded `"` doubled.

/// Render one CSV record from field slices, terminated by `\n`.
pub fn row(fields: &[&str]) -> String {
    let mut s = String::with_capacity(fields.iter().map(|f| f.len() + 1).sum());
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&quote(f));
    }
    s.push('\n');
    s
}

/// Quote a single field per RFC 4180: wrap in `"..."` and double embedded `"`
/// when the field contains comma, `"`, `\n`, or `\r`; otherwise emit as-is.
fn quote(f: &str) -> String {
    let needs = f.contains(',') || f.contains('"') || f.contains('\n') || f.contains('\r');
    if !needs {
        return f.to_string();
    }
    let mut s = String::with_capacity(f.len() + 2);
    s.push('"');
    for c in f.chars() {
        if c == '"' {
            s.push('"');
        }
        s.push(c);
    }
    s.push('"');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_fields_unquoted() {
        assert_eq!(row(&["a", "b", "c"]), "a,b,c\n");
    }

    #[test]
    fn comma_quoted() {
        assert_eq!(row(&["a,b", "c"]), "\"a,b\",c\n");
    }

    #[test]
    fn quote_doubled() {
        assert_eq!(row(&["he said \"hi\"", "x"]), "\"he said \"\"hi\"\"\",x\n");
    }

    #[test]
    fn newline_quoted() {
        assert_eq!(row(&["line1\nline2"]), "\"line1\nline2\"\n");
    }

    #[test]
    fn empty_and_numeric_strings_pass_through() {
        assert_eq!(row(&["", "200", "12.5"]), ",200,12.5\n");
    }

    #[test]
    fn single_empty_field() {
        assert_eq!(row(&[""]), "\n");
    }
}
