//! Shared test fixture: builds a synthetic `.cbor.zstd` file via the tool's own
//! `repack` command, exercising every format quirk we care about:
//!   - concatenated multi-record stream
//!   - CBOR `bytes` (raw bodies) round-tripped via `__cbor_bytes_b64`
//!   - float precision (a value that serde_json mangles without `float_roundtrip`)
//!   - empty `usage` map
//!   - null vs list tags, null vs map headers
//!   - varied model / path / status / identity for filter tests

#![allow(dead_code)]

use assert_cmd::Command;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

/// The source NDJSON used to build the fixture. Kept as the source of truth so
/// tests can assert that `extract(fixture)` reproduces it bit-for-bit
/// (semantically, via serde_json equality).
pub const SOURCE_NDJSON: &str = "\
{\"id\":1,\"ver\":2,\"timestamp\":\"2026-03-09T22:25:37.430509634Z\",\"identity\":{\"login_name\":\"alice\",\"stable_node_id\":\"nodeAAA\"},\"model\":\"alpha/one\",\"api_type\":\"oai_completions\",\"usage\":{\"input_tokens\":31,\"cached_tokens\":16,\"reasoning_tokens\":153},\"estimated_cost\":{\"dollars\":0.046167639999999996,\"cost_basis\":\"alpha/one\",\"usage\":{\"input_tokens\":31,\"output_tokens\":7}},\"duration_ms\":1892,\"capture_id\":\"cap-1\",\"session_id\":\"sess-1\",\"path\":\"/v1/chat/completions\",\"status_code\":200,\"capture\":{\"id\":\"cap-1\",\"model\":\"alpha/one\",\"startTime\":\"2026-03-09T22:25:35Z\",\"loginName\":\"alice\",\"stableNodeId\":\"nodeAAA\",\"tags\":null,\"statusCode\":200,\"path\":\"/v1/chat/completions\",\"toolUseData\":[],\"requestHeaders\":{\"Authorization\":[\"Bearer hunter2-secret\"]},\"requestBody\":\"hello world\",\"responseHeaders\":{\"Set-Cookie\":[\"session=sek$ret\"]},\"responseBody\":\"reply\",\"rawRequestBody\":{\"__cbor_bytes_b64\":\"eyJyYXciOnRydWV9\"},\"rawResponseBody\":{\"__cbor_bytes_b64\":\"dW5hdXRob3JpemVk\"},\"sessionId\":\"sess-1\",\"estimated_cost\":{\"dollars\":0.046167639999999996,\"cost_basis\":\"alpha/one\",\"usage\":{\"input_tokens\":31,\"output_tokens\":7}}}}
{\"id\":2,\"ver\":2,\"timestamp\":\"2026-03-10T10:00:00Z\",\"identity\":{\"login_name\":\"bob\",\"stable_node_id\":\"nodeBBB\"},\"model\":\"beta/two\",\"api_type\":\"anthropic\",\"usage\":{},\"duration_ms\":500,\"capture_id\":\"cap-2\",\"session_id\":\"sess-2\",\"path\":\"/v1/messages\",\"status_code\":404,\"capture\":{\"id\":\"cap-2\",\"model\":\"beta/two\",\"startTime\":\"2026-03-10T10:00:00Z\",\"loginName\":\"bob\",\"stableNodeId\":\"nodeBBB\",\"tags\":[\"retry\"],\"statusCode\":404,\"path\":\"/v1/messages\",\"toolUseData\":[],\"requestHeaders\":null,\"requestBody\":\"\",\"responseHeaders\":null,\"responseBody\":\"not found\",\"rawRequestBody\":null,\"rawResponseBody\":null,\"sessionId\":\"sess-2\"}}
{\"id\":3,\"ver\":2,\"timestamp\":\"2026-03-11T10:00:00Z\",\"identity\":{\"login_name\":\"alice\",\"stable_node_id\":\"nodeAAA\"},\"model\":\"alpha/one\",\"api_type\":\"oai_completions\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5},\"estimated_cost\":{\"dollars\":0.001,\"cost_basis\":\"alpha/one\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5}},\"duration_ms\":100,\"capture_id\":\"cap-3\",\"session_id\":\"sess-3\",\"path\":\"/v1/chat/completions\",\"status_code\":200,\"capture\":{\"id\":\"cap-3\",\"model\":\"alpha/one\",\"startTime\":\"2026-03-11T10:00:00Z\",\"loginName\":\"alice\",\"stableNodeId\":\"nodeAAA\",\"tags\":null,\"statusCode\":200,\"path\":\"/v1/chat/completions\",\"toolUseData\":[],\"requestHeaders\":{\"X-Trace\":[\"abc\"]},\"requestBody\":\"q\",\"responseHeaders\":{},\"responseBody\":\"a\",\"rawRequestBody\":null,\"rawResponseBody\":null,\"sessionId\":\"sess-3\",\"estimated_cost\":{\"dollars\":0.001,\"cost_basis\":\"alpha/one\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5}}}}
";

pub struct Fixture {
    pub _dir: TempDir, // kept alive to retain the temp files
    pub dir: PathBuf,
    pub cbor_zstd: PathBuf,
    pub source_ndjson: PathBuf,
}

impl Fixture {
    /// Build the fixture: write source NDJSON, repack to `.cbor.zstd`.
    pub fn new() -> Self {
        Self::from_ndjson(SOURCE_NDJSON)
    }

    /// Build a fixture from an arbitrary NDJSON source string.
    pub fn from_ndjson(source: &str) -> Self {
        let dir = TempDir::new().expect("tempdir");
        let dir_path = dir.path().to_path_buf();
        let source_path = dir_path.join("source.ndjson");
        let packed = dir_path.join("fixture.cbor.zstd");

        fs::write(&source_path, source).expect("write source");

        Command::cargo_bin("czsplicer")
            .expect("binary")
            .arg("repack")
            .arg(&source_path)
            .arg("-o")
            .arg(&packed)
            .assert()
            .success();

        Fixture {
            _dir: dir,
            dir: dir_path,
            cbor_zstd: packed,
            source_ndjson: source_path,
        }
    }

    /// Run `czsplicer` against the fixture.
    pub fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("czsplicer").expect("binary");
        c.env("APOLOG_FIXTURE", &self.cbor_zstd);
        c
    }
}

/// Parse an NDJSON string into a Vec of serde_json values (order-preserving).
pub fn read_ndjson(s: &str) -> Vec<serde_json::Value> {
    s.lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<Vec<_>, _>>()
        .expect("valid ndjson")
}

/// A richer 5-record dataset for merge/split/session tests:
///   - days:    2026-03-09 (id1,id4), 2026-03-10 (id2), 2026-03-11 (id3,id5)
///   - sessions: sess-1 (id1,id4), sess-2, sess-3, sess-4  (only sess-1 has >1)
///   - models:  alpha/one (id1,id3,id4), beta/two (id2,id5)
///   - paths:   /v1/chat/completions (id1,id3,id4), /v1/messages (id2,id5)
///   - id1 carries a CBOR bytes body to prove split preserves bytes.
pub const RICH_NDJSON: &str = "\
{\"id\":1,\"timestamp\":\"2026-03-09T22:25:37Z\",\"model\":\"alpha/one\",\"path\":\"/v1/chat/completions\",\"session_id\":\"sess-1\",\"status_code\":200,\"usage\":{\"input_tokens\":31},\"capture\":{\"rawRequestBody\":{\"__cbor_bytes_b64\":\"eyJyYXciOnRydWV9\"}}}
{\"id\":2,\"timestamp\":\"2026-03-10T10:00:00Z\",\"model\":\"beta/two\",\"path\":\"/v1/messages\",\"session_id\":\"sess-2\",\"status_code\":404,\"usage\":{},\"capture\":{}}
{\"id\":3,\"timestamp\":\"2026-03-11T10:00:00Z\",\"model\":\"alpha/one\",\"path\":\"/v1/chat/completions\",\"session_id\":\"sess-3\",\"status_code\":200,\"usage\":{\"input_tokens\":10},\"capture\":{}}
{\"id\":4,\"timestamp\":\"2026-03-09T23:00:00Z\",\"model\":\"alpha/one\",\"path\":\"/v1/chat/completions\",\"session_id\":\"sess-1\",\"status_code\":200,\"usage\":{\"input_tokens\":8},\"capture\":{}}
{\"id\":5,\"timestamp\":\"2026-03-11T11:00:00Z\",\"model\":\"beta/two\",\"path\":\"/v1/messages\",\"session_id\":\"sess-4\",\"status_code\":200,\"usage\":{\"input_tokens\":4},\"capture\":{}}
";

/// Build a fixture from RICH_NDJSON.
pub fn rich_fixture() -> Fixture {
    Fixture::from_ndjson(RICH_NDJSON)
}
