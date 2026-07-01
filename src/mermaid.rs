//! Mermaid diagram emitters.
//!
//! Pure functions over already-aggregated data. Each returns a String
//! containing a single Mermaid diagram block (no surrounding fences — callers
//! compose them). Diagrams target the GitHub-rendered Mermaid subset.
//!
//! Aggregation policy (decided 2026-07-01 against a real `days/` corpus:
//! 17,484 records, 25 models, ~110-day span):
//! - model cardinality: collapse to top-N + "other" (callers pass pre-collapsed
//!   slices; `top_n` helper is provided).
//! - time axis: per-day bucketing.
//! - cost curves are spiky: `xychart` picks log-vs-linear automatically.
//! - path/status cardinality is small enough to render in full.

/// Format an f64 for Mermaid, trimming trailing zeros. NaN/inf (which never
/// occur in cost/token aggregates, but can sneak in via upstream parsing) fall
/// back to 0 so the rendered diagram is always well-formed.
fn num(n: f64) -> String {
    let n = if n.is_finite() { n } else { 0.0 };
    if n == n.trunc() {
        format!("{n:.0}")
    } else {
        format!("{n:.4}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

/// Sanitize a label for safe use inside Mermaid node/series text. Mermaid uses
/// `"..."` to quote arbitrary text; we escape embedded quotes and strip newlines.
fn clean(s: &str) -> String {
    s.replace(['\n', '\r'], " ").replace('"', "'")
}

/// A labelled numeric slice for `pie` and `bar`.
#[derive(Clone)]
pub struct Slice {
    pub label: String,
    pub value: f64,
}

/// Collapse a (label, value) list to at most `n` entries by value, summing the
/// remainder into a single "other" entry. Drops zero-value "other" when empty.
/// Used by callers facing high-cardinality dimensions (model). The sort is
/// value-descending with label-ascending tiebreaker, so equal-valued rows keep
/// a deterministic order regardless of input permutation.
pub fn top_n(mut rows: Vec<(String, f64)>, n: usize, other_label: &str) -> Vec<(String, f64)> {
    rows.sort_by(|(la, va), (lb, vb)| {
        vb.partial_cmp(va)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| la.cmp(lb))
    });
    if rows.len() <= n {
        return rows;
    }
    let other: f64 = rows[n..].iter().map(|(_, v)| *v).sum();
    rows.truncate(n);
    if other > 0.0 {
        rows.push((other_label.to_string(), other));
    }
    rows
}

/// Render a `pie` diagram. Slices should be pre-collapsed (see `top_n`).
/// Values must be non-negative; zero-value slices are dropped.
pub fn pie(title: &str, slices: &[Slice]) -> String {
    if slices.iter().all(|s| s.value <= 0.0) {
        return String::new();
    }
    let mut s = String::new();
    s.push_str("```mermaid\n");
    s.push_str(&format!("%% {title}\n"));
    s.push_str("pie showData\n");
    s.push_str(&format!("    title {title}\n"));
    for sl in slices {
        if sl.value <= 0.0 {
            continue;
        }
        s.push_str(&format!(
            "    \"{}\" : {}\n",
            clean(&sl.label),
            num(sl.value)
        ));
    }
    s.push_str("```\n");
    s
}

/// A point on a chart's value axis. Callers must pass points in axis order
/// (e.g. sorted by day); the emitter plots values sequentially.
#[derive(Clone)]
pub struct Point {
    pub x: String,
    pub y: f64,
}

/// Render an `xychart-beta` line chart. Points are plotted in the order given
/// (callers should sort by `x`). Zero-valued points ARE plotted (so a gap in the
/// data isn't silently compressed onto its neighbours); an all-zero or empty
/// series returns an empty string. The y-axis switches to log scale
/// automatically when the non-zero max/min ratio exceeds 20x (real cost curves
/// are spiky: a single $300+ day flattens a linear axis).
pub fn xychart(title: &str, points: &[Point], x_label: &str, y_label: &str) -> String {
    if !points.iter().any(|p| p.y > 0.0) {
        return String::new();
    }
    let ymax = points
        .iter()
        .map(|p| p.y)
        .filter(|&v| v > 0.0)
        .fold(0.0f64, f64::max);
    let ymin = points
        .iter()
        .map(|p| p.y)
        .filter(|&v| v > 0.0)
        .fold(f64::MAX, f64::min);
    let log_y = ymin > 0.0 && ymax / ymin > 20.0;

    let mut s = String::new();
    s.push_str("```mermaid\n");
    s.push_str(&format!("%% {title}\n"));
    s.push_str("xychart-beta\n");
    s.push_str(&format!("    title \"{title}\"\n"));
    s.push_str(&format!(
        "    x-axis \"{x_label}\" [{}]\n",
        points
            .iter()
            .map(|p| clean(&p.x))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    if log_y {
        s.push_str(&format!("    y-axis \"{y_label}\" log\n"));
    } else {
        s.push_str(&format!("    y-axis \"{y_label}\"\n"));
    }
    s.push_str("    line [");
    s.push_str(
        &points
            .iter()
            .map(|p| num(p.y))
            .collect::<Vec<_>>()
            .join(", "),
    );
    s.push_str("]\n");
    s.push_str("```\n");
    s
}

/// A grouped event for `timeline`: events sharing a group are rendered as a
/// time period heading, each `event` is a line beneath it.
pub struct TimelineGroup {
    pub period: String,
    pub events: Vec<String>,
}

/// Render a `timeline` diagram. Groups are emitted in the order given.
pub fn timeline(title: &str, groups: &[TimelineGroup]) -> String {
    let mut s = String::new();
    s.push_str("```mermaid\n");
    s.push_str(&format!("%% {title}\n"));
    s.push_str("timeline\n");
    s.push_str(&format!("    title {title}\n"));
    for g in groups {
        // Mermaid timeline periods cannot be quoted strings; sanitize by
        // dropping characters that break the parser.
        let period = g.period.replace(['"', ':', '\n', '\r'], "");
        s.push_str(&format!("    {period}\n"));
        for ev in &g.events {
            s.push_str(&format!("        : {}\n", clean(ev)));
        }
    }
    s.push_str("```\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_n_collapses_tail_to_other() {
        let rows = vec![
            ("a".into(), 10.0),
            ("b".into(), 5.0),
            ("c".into(), 3.0),
            ("d".into(), 2.0),
        ];
        let out = top_n(rows, 2, "other");
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].0, "a");
        assert_eq!(out[1].0, "b");
        assert_eq!(out[2].0, "other");
        assert!((out[2].1 - 5.0).abs() < 1e-9);
    }

    #[test]
    fn top_n_no_other_when_under_threshold() {
        let rows = vec![("a".into(), 1.0), ("b".into(), 2.0)];
        let out = top_n(rows, 5, "other");
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn top_n_breaks_value_ties_by_label_ascending() {
        // Equal-valued rows must keep a deterministic (label-ascending) order
        // regardless of input permutation, so output is stable across runs.
        let rows = vec![
            ("charlie".into(), 5.0),
            ("alpha".into(), 5.0),
            ("bravo".into(), 5.0),
            ("delta".into(), 1.0),
        ];
        let out = top_n(rows, 2, "other");
        assert_eq!(out[0].0, "alpha");
        assert_eq!(out[1].0, "bravo");
        assert_eq!(out[2].0, "other");
        assert!((out[2].1 - 6.0).abs() < 1e-9);
    }

    #[test]
    fn pie_drops_zero_slices() {
        let sl = [
            Slice {
                label: "a".into(),
                value: 5.0,
            },
            Slice {
                label: "b".into(),
                value: 0.0,
            },
        ];
        let out = pie("t", &sl);
        assert!(out.contains("\"a\" : 5"));
        assert!(!out.contains("\"b\""));
    }

    #[test]
    fn xychart_log_when_spiky() {
        let pts = [
            Point {
                x: "a".into(),
                y: 1.0,
            },
            Point {
                x: "b".into(),
                y: 400.0,
            },
        ];
        let out = xychart("t", &pts, "d", "$");
        assert!(out.contains("y-axis \"$\" log"));
    }

    #[test]
    fn xychart_linear_when_flat() {
        let pts = [
            Point {
                x: "a".into(),
                y: 10.0,
            },
            Point {
                x: "b".into(),
                y: 15.0,
            },
        ];
        let out = xychart("t", &pts, "d", "$");
        assert!(out.contains("y-axis \"$\"\n"));
        assert!(!out.contains("log"));
    }

    #[test]
    fn xychart_keeps_zero_points_and_labels() {
        let pts = [
            Point {
                x: "a".into(),
                y: 1.0,
            },
            Point {
                x: "b".into(),
                y: 0.0,
            },
            Point {
                x: "c".into(),
                y: 2.0,
            },
        ];
        let out = xychart("t", &pts, "d", "$");
        assert!(
            out.contains("x-axis \"d\" [a, b, c]"),
            "x labels should be emitted: {out}"
        );
        assert!(
            out.contains("line [1, 0, 2]"),
            "zero point must be plotted, not dropped: {out}"
        );
    }

    #[test]
    fn xychart_empty_on_all_zero() {
        let pts = [Point {
            x: "a".into(),
            y: 0.0,
        }];
        assert_eq!(xychart("t", &pts, "d", "$"), "");
    }

    #[test]
    fn pie_empty_on_all_zero() {
        let sl = [Slice {
            label: "a".into(),
            value: 0.0,
        }];
        assert_eq!(pie("t", &sl), "");
    }
}
