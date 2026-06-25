use crate::format;
use anyhow::{anyhow, Result};

/// An inclusive integer range [lo, hi].
#[derive(Clone, Copy, Debug)]
pub struct IdRange {
    pub lo: i64,
    pub hi: i64,
}

/// Record selection predicate. A record "matches" when every non-empty list
/// contains it. `invert` flips keep <-> drop semantics.
#[derive(Clone, Default)]
pub struct Filter {
    pub ids: Vec<IdRange>,
    pub models: Vec<String>,
    pub providers: Vec<String>,
    pub paths: Vec<String>,
    pub statuses: Vec<i64>,
    pub api_types: Vec<String>,
    pub login_names: Vec<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub date: Option<String>,
    pub invert: bool,
}

impl Filter {
    pub fn matches(&self, rec: &ciborium::Value) -> bool {
        let m = self.matches_pos(rec);
        m ^ self.invert
    }

    fn matches_pos(&self, rec: &ciborium::Value) -> bool {
        if !self.ids.is_empty() {
            let Some(id) = format::rec_int(rec, "id") else {
                return false;
            };
            if !self.ids.iter().any(|r| id >= r.lo && id <= r.hi) {
                return false;
            }
        }
        if !self.models.is_empty() {
            let Some(m) = format::rec_str(rec, "model") else {
                return false;
            };
            if !self.models.iter().any(|x| x == &m) {
                return false;
            }
        }
        if !self.providers.is_empty() {
            let Some(m) = format::rec_str(rec, "model") else {
                return false;
            };
            // Provider = model prefix before first '/'. Models without '/' have
            // no provider and don't match.
            let Some(prov) = m.split_once('/').map(|(p, _)| p) else {
                return false;
            };
            if !self.providers.iter().any(|x| x == prov) {
                return false;
            }
        }
        if !self.paths.is_empty() {
            let Some(p) = format::rec_str(rec, "path") else {
                return false;
            };
            if !self.paths.iter().any(|x| x == &p) {
                return false;
            }
        }
        if !self.statuses.is_empty() {
            let Some(s) = format::rec_int(rec, "status_code") else {
                return false;
            };
            if !self.statuses.contains(&s) {
                return false;
            }
        }
        if !self.api_types.is_empty() {
            let Some(a) = format::rec_str(rec, "api_type") else {
                return false;
            };
            if !self.api_types.iter().any(|x| x == &a) {
                return false;
            }
        }
        if !self.login_names.is_empty() {
            let Some(id) = format::field(rec, "identity") else {
                return false;
            };
            let Some(ln) = format::field(id, "login_name").and_then(format::as_str) else {
                return false;
            };
            if !self.login_names.iter().any(|x| x == &ln) {
                return false;
            }
        }
        if self.since.is_some() || self.until.is_some() || self.date.is_some() {
            let Some(ts) = format::rec_str(rec, "timestamp") else {
                return false;
            };
            if !ts_passes(&ts, &self.since, &self.until, &self.date) {
                return false;
            }
        }
        true
    }
}

/// Timestamp gate. Timestamps are ISO-8601 UTC (e.g. `2026-05-30T03:27:21Z`),
/// so fixed-format comparison is a plain lexicographic string compare.
///
/// - `--since` / `--until` accept ISO-8601 prefixes; a bare date (`YYYY-MM-DD`)
///   for `--until` is treated as end-of-day (inclusive).
/// - `--date` matches a full calendar day exactly.
fn ts_passes(
    ts: &str,
    since: &Option<String>,
    until: &Option<String>,
    date: &Option<String>,
) -> bool {
    if let Some(d) = date {
        // exact calendar-day match
        if ts.get(..d.len()) != Some(d.as_str()) {
            return false;
        }
    }
    if let Some(s) = since {
        if ts < s.as_str() {
            return false;
        }
    }
    if let Some(u) = until {
        // bare date -> inclusive end of day (23:59:59 sorts before nanosecond ts)
        let bound = if u.len() == 10 {
            format!("{u}T23:59:59Z")
        } else {
            u.clone()
        };
        if ts > bound.as_str() {
            return false;
        }
    }
    true
}

/// clap-friendly filter arguments shared across subcommands.
#[derive(clap::Args, Clone, Default)]
pub struct FilterArgs {
    /// Record id or inclusive range (e.g. 5 or 5-10). Repeatable.
    #[arg(long, value_name = "ID|RANGE")]
    pub id: Vec<String>,
    /// Keep/drop records whose model matches exactly. Repeatable.
    #[arg(long, value_name = "MODEL")]
    pub model: Vec<String>,
    /// Keep/drop records whose provider (model prefix before '/') matches
    /// exactly. Repeatable.
    #[arg(long, value_name = "PROVIDER")]
    pub provider: Vec<String>,
    /// Keep/drop records whose path matches exactly. Repeatable.
    #[arg(long, value_name = "PATH")]
    pub path: Vec<String>,
    /// Keep/drop records by HTTP status code. Repeatable.
    #[arg(long, value_name = "CODE")]
    pub status: Vec<i64>,
    /// Keep/drop records by api_type. Repeatable.
    #[arg(long = "api-type", value_name = "TYPE")]
    pub api_type: Vec<String>,
    /// Keep/drop records by identity.login_name. Repeatable.
    #[arg(long = "login-name", value_name = "NAME")]
    pub login_name: Vec<String>,
    /// Keep records at or after this ISO-8601 time (e.g. `2026-05-30` or
    /// `2026-05-30T12:00:00Z`). Prefix comparison.
    #[arg(long, value_name = "TIME")]
    pub since: Option<String>,
    /// Keep records at or before this time. A bare date is inclusive of the
    /// whole day.
    #[arg(long, value_name = "TIME")]
    pub until: Option<String>,
    /// Keep records on this exact calendar day (`YYYY-MM-DD`).
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub date: Option<String>,
    /// Invert the selection: drop matching records instead of keeping them.
    #[arg(long)]
    pub invert: bool,
}

impl FilterArgs {
    pub fn build(&self) -> Result<Filter> {
        let mut ids = Vec::new();
        for spec in &self.id {
            ids.push(parse_id_range(spec)?);
        }
        Ok(Filter {
            ids,
            models: self.model.clone(),
            providers: self.provider.clone(),
            paths: self.path.clone(),
            statuses: self.status.clone(),
            api_types: self.api_type.clone(),
            login_names: self.login_name.clone(),
            since: self.since.clone(),
            until: self.until.clone(),
            date: self.date.clone(),
            invert: self.invert,
        })
    }
}

fn parse_id_range(s: &str) -> Result<IdRange> {
    if let Some((a, b)) = s.split_once('-') {
        let lo: i64 = a
            .parse()
            .map_err(|e| anyhow!("invalid id range lower bound `{a}`: {e}"))?;
        let hi: i64 = b
            .parse()
            .map_err(|e| anyhow!("invalid id range upper bound `{b}`: {e}"))?;
        if hi < lo {
            return Err(anyhow!("id range `{s}` has hi < lo"));
        }
        Ok(IdRange { lo, hi })
    } else {
        let n: i64 = s.parse().map_err(|e| anyhow!("invalid id `{s}`: {e}"))?;
        Ok(IdRange { lo: n, hi: n })
    }
}
