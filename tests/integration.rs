mod common;

use assert_cmd::Command;
use common::{read_ndjson, rich_fixture, Fixture, RICH_NDJSON};
use predicates::prelude::*;
use std::fs;

fn fixture() -> Fixture {
    // fresh fixture per test for isolation
    Fixture::new()
}

// ===========================================================================
// info
// ===========================================================================

#[test]
fn info_table_shows_counts_and_ranges() {
    let f = fixture();
    f.cmd()
        .arg("info")
        .arg(&f.cbor_zstd)
        .assert()
        .success()
        .stdout(predicate::str::contains("records"))
        .stdout(predicate::str::contains("3")) // 3 records total
        .stdout(predicate::str::contains("1-3")); // id range
}

#[test]
fn info_json_has_expected_shape() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("info")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["totals"]["records"].as_i64(), Some(3));
    assert!(v["files"].is_array());
    let rec = &v["files"][0];
    assert_eq!(rec["records"].as_i64(), Some(3));
    assert_eq!(rec["id_min"].as_i64(), Some(1));
    assert_eq!(rec["id_max"].as_i64(), Some(3));
}

// ===========================================================================
// ls
// ===========================================================================

#[test]
fn ls_table_lists_all_records() {
    let f = fixture();
    f.cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .assert()
        .success()
        .stdout(predicate::str::contains("alpha/one"))
        .stdout(predicate::str::contains("beta/two"));
}

#[test]
fn ls_json_is_ndjson() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    assert_eq!(recs.len(), 3);
    assert_eq!(recs[0]["id"].as_i64(), Some(1));
    assert_eq!(recs[0]["model"].as_str(), Some("alpha/one"));
}

#[test]
fn ls_filter_by_model() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--model")
        .arg("beta/two")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0]["id"].as_i64(), Some(2));
}

#[test]
fn ls_filter_by_id_range() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--id")
        .arg("1-2")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    assert_eq!(recs.len(), 2);
}

#[test]
fn ls_filter_by_status() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--status")
        .arg("404")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0]["id"].as_i64(), Some(2));
}

#[test]
fn ls_filter_invert() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--status")
        .arg("200")
        .arg("--invert")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0]["status_code"].as_i64(), Some(404));
}

#[test]
fn ls_filter_by_login_name() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--login-name")
        .arg("alice")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    assert_eq!(recs.len(), 2);
}

#[test]
fn ls_filter_by_client_prefix() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--client")
        .arg("maki")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0]["id"].as_i64(), Some(1));
}

#[test]
fn ls_filter_by_client_case_insensitive() {
    // Query casing differs from the stored `Aperture-Chat/1.0` (record 3).
    let f = fixture();
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--client")
        .arg("APERTURE")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0]["id"].as_i64(), Some(3));
}

#[test]
fn ls_filter_by_client_repeatable_or() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--client")
        .arg("maki")
        .arg("--client")
        .arg("aperture")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    let mut ids: Vec<i64> = recs.iter().filter_map(|r| r["id"].as_i64()).collect();
    ids.sort();
    assert_eq!(ids, vec![1, 3]);
}

#[test]
fn ls_filter_by_client_skips_null_headers() {
    // Record 2 has requestHeaders: null — must not match any --client.
    let f = fixture();
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--client")
        .arg("zzz-no-such-client")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    assert!(recs.is_empty());
}

#[test]
fn ls_filter_by_client_invert() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--client")
        .arg("maki")
        .arg("--invert")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    let mut ids: Vec<i64> = recs.iter().filter_map(|r| r["id"].as_i64()).collect();
    ids.sort();
    // Drop the maki record (id 1); keep ids 2 and 3.
    assert_eq!(ids, vec![2, 3]);
}

// ===========================================================================
// extract
// ===========================================================================

#[test]
fn extract_ndjson_matches_source() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("extract")
        .arg(&f.cbor_zstd)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let extracted = read_ndjson(std::str::from_utf8(&out).unwrap());
    let source = read_ndjson(common::SOURCE_NDJSON);
    assert_eq!(extracted, source);
}

#[test]
fn extract_preserves_float_precision() {
    // The tricky value 0.046167639999999996 must survive unpacking unchanged.
    let f = fixture();
    let out = f
        .cmd()
        .arg("extract")
        .arg(&f.cbor_zstd)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(
        s.contains("0.046167639999999996"),
        "float precision lost: {}",
        &s[s.find("dollars").unwrap_or(0)..]
    );
}

#[test]
fn extract_preserves_bytes_as_b64() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("extract")
        .arg(&f.cbor_zstd)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    let cap = &recs[0]["capture"];
    // bytes survived as the sentinel object
    assert_eq!(
        cap["rawRequestBody"]["__cbor_bytes_b64"].as_str(),
        Some("eyJyYXciOnRydWV9")
    );
    assert_eq!(
        cap["rawResponseBody"]["__cbor_bytes_b64"].as_str(),
        Some("dW5hdXRob3JpemVk")
    );
}

#[test]
fn extract_array_emits_single_json_array() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("extract")
        .arg(&f.cbor_zstd)
        .arg("--array")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert!(v.is_array(), "expected a JSON array");
    assert_eq!(v.as_array().unwrap().len(), 3);
}

#[test]
fn extract_pretty_is_indented() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("extract")
        .arg(&f.cbor_zstd)
        .arg("--array")
        .arg("--pretty")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(s.starts_with("[\n"), "expected pretty/indented output");
}

#[test]
fn extract_fields_projects_only_requested_paths() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("extract")
        .arg(&f.cbor_zstd)
        .arg("--fields")
        .arg("id,usage.input_tokens")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    assert_eq!(recs.len(), 3);
    let r0 = &recs[0];
    assert_eq!(r0["id"].as_i64(), Some(1));
    assert_eq!(r0["usage.input_tokens"].as_i64(), Some(31));
    // only projected keys present
    let keys: Vec<&str> = r0.as_object().unwrap().keys().map(|k| k.as_str()).collect();
    assert_eq!(keys, vec!["id", "usage.input_tokens"]);
}

#[test]
fn extract_bodies_dumps_request_response_files() {
    let f = fixture();
    let bodies = f.dir.join("bodies");
    f.cmd()
        .arg("extract")
        .arg(&f.cbor_zstd)
        .arg("--bodies")
        .arg(&bodies)
        .assert()
        .success();
    // id=1 has text bodies
    assert_eq!(
        fs::read_to_string(bodies.join("1.request")).unwrap(),
        "hello world"
    );
    assert_eq!(
        fs::read_to_string(bodies.join("1.response")).unwrap(),
        "reply"
    );
    // id=2 has empty string requestBody, "" -> written as empty file
    assert_eq!(fs::read_to_string(bodies.join("2.request")).unwrap(), "");
}

// ===========================================================================
// repack round-trips (the core guarantee)
// ===========================================================================

#[test]
fn repack_ndjson_roundtrip_is_lossless() {
    // source NDJSON -> pack -> unpack -> must equal source (bytes + floats).
    let f = fixture();
    let extracted = f
        .cmd()
        .arg("extract")
        .arg(&f.cbor_zstd)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    // Re-pack the extraction into a new file, then extract THAT and compare.
    let nd2 = f.dir.join("round2.ndjson");
    fs::write(&nd2, &extracted).unwrap();
    let cz2 = f.dir.join("round2.cbor.zstd");
    Command::cargo_bin("czsplicer")
        .unwrap()
        .arg("repack")
        .arg(&nd2)
        .arg("-o")
        .arg(&cz2)
        .assert()
        .success();

    let out = f
        .cmd()
        .arg("extract")
        .arg(&cz2)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let source = read_ndjson(common::SOURCE_NDJSON);
    let twice = read_ndjson(std::str::from_utf8(&out).unwrap());
    assert_eq!(source, twice, "full pack->unpack->pack->unpack not stable");
}

#[test]
fn repack_array_roundtrip_is_lossless() {
    let f = fixture();
    let arr_json = f.dir.join("arr.json");
    f.cmd()
        .arg("extract")
        .arg(&f.cbor_zstd)
        .arg("--array")
        .arg("-o")
        .arg(&arr_json)
        .assert()
        .success();

    let cz = f.dir.join("from_arr.cbor.zstd");
    Command::cargo_bin("czsplicer")
        .unwrap()
        .arg("repack")
        .arg(&arr_json)
        .arg("-o")
        .arg(&cz)
        .assert()
        .success();

    let out = f
        .cmd()
        .arg("extract")
        .arg(&cz)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let twice = read_ndjson(std::str::from_utf8(&out).unwrap());
    let source = read_ndjson(common::SOURCE_NDJSON);
    assert_eq!(source, twice, "array round-trip not lossless");
}

#[test]
fn repack_raw_emits_uncompressed_cbor() {
    let f = fixture();
    let raw_cbor = f.dir.join("out.cbor");
    Command::cargo_bin("czsplicer")
        .unwrap()
        .arg("repack")
        .arg(&f.source_ndjson)
        .arg("-o")
        .arg(&raw_cbor)
        .arg("--raw")
        .assert()
        .success();
    // CBOR map starts with a major-type-5 (map) byte. For a 14-key map that's 0xae.
    let head = fs::read(&raw_cbor).unwrap();
    assert!(!head.is_empty());
    assert_eq!(
        head[0] & 0xe0,
        0xa0,
        "expected CBOR map major type (0xa0-0xbf)"
    );
}

// ===========================================================================
// edit: redact / strip / drop
// ===========================================================================

#[test]
fn edit_redact_scrubs_strings() {
    let f = fixture();
    let out_cz = f.dir.join("redacted.cbor.zstd");
    f.cmd()
        .arg("edit")
        .arg(&f.cbor_zstd)
        .arg("-o")
        .arg(&out_cz)
        .arg("--redact")
        .arg("hunter2-secret")
        .arg("--all-strings")
        .assert()
        .success();

    let out = f
        .cmd()
        .arg("extract")
        .arg(&out_cz)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(!s.contains("hunter2-secret"), "secret leaked: {s}");
    assert!(s.contains("[REDACTED]"));
}

#[test]
fn edit_redact_scrubs_byte_bodies() {
    // Regression for the headline bug: capture.rawRequestBody is a CBOR `bytes`
    // value (the raw HTTP body). `redact_strings` used to skip Bytes entirely,
    // so a secret in a raw body survived `edit --redact` even though `grep`
    // could find it via lossy UTF-8 decode. The fix scrubs UTF-8 byte bodies.
    //
    // We assert on the *decoded* body: the secret is never plain text in the
    // extracted JSON (it rides inside `__cbor_bytes_b64`), so a naive
    // `!out.contains(secret)` would pass even on the unfixed code — exactly the
    // blind spot that let the bug ship.
    use base64::Engine;
    let body_b64 = "dG9rZW49c2stbGl2ZS1BYkNkMTIzNC1YWVomdXNlcj1hZG1pbkBleGFtcGxlLmNvbQ==";
    let secret = "sk-live-AbCd1234-XYZ";
    let ndjson =
        "{\"id\":1,\"model\":\"alpha/one\",\"capture\":{\"rawRequestBody\":{\"__cbor_bytes_b64\":\""
            .to_string()
            + body_b64 + "\"}}}\n";
    let f = Fixture::from_ndjson(&ndjson);
    let out_cz = f.dir.join("redacted.cbor.zstd");
    f.cmd()
        .arg("edit")
        .arg(&f.cbor_zstd)
        .arg("-o")
        .arg(&out_cz)
        .arg("--redact")
        .arg(secret)
        .assert()
        .success();

    let out = f
        .cmd()
        .arg("extract")
        .arg(&out_cz)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs = read_ndjson(std::str::from_utf8(&out).unwrap());
    let b64_out = recs[0]["capture"]["rawRequestBody"]["__cbor_bytes_b64"]
        .as_str()
        .expect("rawRequestBody bytes present");
    let decoded = String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(b64_out)
            .unwrap(),
    )
    .unwrap();
    assert!(
        !decoded.contains(secret),
        "secret leaked through bytes body: {decoded}"
    );
    assert!(
        decoded.contains("[REDACTED]"),
        "redaction marker missing in decoded body: {decoded}"
    );
}

#[test]
fn edit_redact_leaves_binary_bytes_untouched() {
    // Counterpart to edit_redact_scrubs_byte_bodies: a byte body that is NOT
    // valid UTF-8 (a genuinely binary payload) must pass through byte-for-byte
    // unchanged. redact_strings gates Bytes scrubbing on valid UTF-8 precisely
    // so it never corrupts binary content. This pins that contract — a future
    // change to lossy-decode-and-rewrite would mangle binary bodies and fail
    // here.
    use base64::Engine;
    // 0x80/0x81/0x82 are UTF-8 continuation bytes with no lead byte -> invalid.
    let expected: Vec<u8> = vec![0x80, 0x81, 0x82];
    let bin_b64 = "gIGC";
    // Sanity: the constant round-trips and really is invalid UTF-8 (guards
    // against a malformed literal above).
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(bin_b64)
            .unwrap(),
        expected,
    );
    assert!(std::str::from_utf8(&expected).is_err());

    let ndjson =
        "{\"id\":1,\"model\":\"alpha/one\",\"capture\":{\"rawRequestBody\":{\"__cbor_bytes_b64\":\""
            .to_string()
            + bin_b64 + "\"}}}\n";
    let f = Fixture::from_ndjson(&ndjson);
    let out_cz = f.dir.join("redacted.cbor.zstd");
    f.cmd()
        .arg("edit")
        .arg(&f.cbor_zstd)
        .arg("-o")
        .arg(&out_cz)
        .arg("--redact")
        .arg(".") // would match every byte if the body were decoded as text
        .assert()
        .success();

    let out = f
        .cmd()
        .arg("extract")
        .arg(&out_cz)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs = read_ndjson(std::str::from_utf8(&out).unwrap());
    let b64_out = recs[0]["capture"]["rawRequestBody"]["__cbor_bytes_b64"]
        .as_str()
        .expect("rawRequestBody bytes present");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(b64_out)
        .unwrap();
    assert_eq!(
        decoded, expected,
        "binary byte body was modified by redaction"
    );
}

#[test]
fn edit_strip_headers_nulls_headers() {
    let f = fixture();
    let out_cz = f.dir.join("stripped.cbor.zstd");
    f.cmd()
        .arg("edit")
        .arg(&f.cbor_zstd)
        .arg("-o")
        .arg(&out_cz)
        .arg("--strip-headers")
        .assert()
        .success();

    let out = f
        .cmd()
        .arg("extract")
        .arg(&out_cz)
        .arg("--fields")
        .arg("id,capture.requestHeaders,capture.responseHeaders")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    for r in &recs {
        assert!(
            r["capture.requestHeaders"].is_null(),
            "requestHeaders not nulled"
        );
        assert!(
            r["capture.responseHeaders"].is_null(),
            "responseHeaders not nulled"
        );
    }
}

#[test]
fn edit_strip_path_nulls_arbitrary_field() {
    let f = fixture();
    let out_cz = f.dir.join("noresp.cbor.zstd");
    f.cmd()
        .arg("edit")
        .arg(&f.cbor_zstd)
        .arg("-o")
        .arg(&out_cz)
        .arg("--strip")
        .arg("capture.responseBody")
        .assert()
        .success();
    let out = f
        .cmd()
        .arg("extract")
        .arg(&out_cz)
        .arg("--fields")
        .arg("id,capture.responseBody")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    for r in &recs {
        assert!(
            r["capture.responseBody"].is_null(),
            "responseBody not nulled"
        );
    }
}

#[test]
fn edit_drop_invert_removes_matching_records() {
    // keep everything NOT (status 200), i.e. drop the two 200 records, keep id=2.
    let f = fixture();
    let out_cz = f.dir.join("dropped.cbor.zstd");
    f.cmd()
        .arg("edit")
        .arg(&f.cbor_zstd)
        .arg("-o")
        .arg(&out_cz)
        .arg("--status")
        .arg("200")
        .arg("--invert")
        .assert()
        .success();

    let out = f
        .cmd()
        .arg("ls")
        .arg(&out_cz)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0]["id"].as_i64(), Some(2));
}

#[test]
fn edit_keep_only_matching_records() {
    // without --invert: keep only alpha/one records (ids 1 and 3).
    let f = fixture();
    let out_cz = f.dir.join("kept.cbor.zstd");
    f.cmd()
        .arg("edit")
        .arg(&f.cbor_zstd)
        .arg("-o")
        .arg(&out_cz)
        .arg("--model")
        .arg("alpha/one")
        .assert()
        .success();

    let out = f
        .cmd()
        .arg("ls")
        .arg(&out_cz)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    assert_eq!(recs.len(), 2);
    assert!(recs
        .iter()
        .all(|r| r["model"].as_str() == Some("alpha/one")));
}

#[test]
fn edit_json_output_is_ndjson() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("edit")
        .arg(&f.cbor_zstd)
        .arg("-o")
        .arg("-")
        .arg("--json")
        .arg("--strip-headers")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    assert_eq!(recs.len(), 3);
    for r in &recs {
        assert!(r["capture"]["requestHeaders"].is_null());
    }
}

#[test]
fn edit_preserves_non_edited_values() {
    // redaction must not corrupt floats or bytes elsewhere.
    let f = fixture();
    let out_cz = f.dir.join("redact2.cbor.zstd");
    f.cmd()
        .arg("edit")
        .arg(&f.cbor_zstd)
        .arg("-o")
        .arg(&out_cz)
        .arg("--redact")
        .arg("hunter2-secret")
        .assert()
        .success();
    let out = f
        .cmd()
        .arg("extract")
        .arg(&out_cz)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(
        s.contains("0.046167639999999996"),
        "float corrupted by edit"
    );
    assert!(
        s.contains("\"__cbor_bytes_b64\":\"eyJyYXciOnRydWV9\""),
        "bytes corrupted by edit"
    );
}

// ===========================================================================
// stats
// ===========================================================================

#[test]
fn stats_table_reports_totals() {
    let f = fixture();
    f.cmd()
        .arg("stats")
        .arg(&f.cbor_zstd)
        .assert()
        .success()
        .stdout(predicate::str::contains("records:"))
        .stdout(predicate::str::contains("alpha/one"))
        .stdout(predicate::str::contains("beta/two"));
}

#[test]
fn stats_json_totals() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("stats")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["records"].as_i64(), Some(3));
    // input_tokens: 31 (id1) + 0 (id2 empty usage) + 10 (id3) = 41
    assert_eq!(v["input_tokens"].as_i64(), Some(41));
    // cost: 0.046167639999999996 + 0 + 0.001 = 0.047167639999999996
    let cost = v["cost_usd"].as_f64().unwrap();
    assert!((cost - 0.047167639999999996).abs() < 1e-15, "cost {cost}");
}

#[test]
fn stats_by_path_dimension() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("stats")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--by")
        .arg("path")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let by_path = v["by_path"].as_array().unwrap();
    assert_eq!(by_path.len(), 2); // /v1/chat/completions and /v1/messages
}

#[test]
fn stats_by_provider_dimension() {
    let f = rich_fixture();
    let out = f
        .cmd()
        .arg("stats")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--by")
        .arg("provider")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let by_provider = v["by_provider"].as_array().unwrap();
    // rich fixture: alpha/one (3), beta/two (2) -> providers: alpha, beta
    assert_eq!(by_provider.len(), 2);
    let alpha = by_provider
        .iter()
        .find(|b| b["key"].as_str() == Some("alpha"))
        .unwrap();
    assert_eq!(alpha["count"].as_i64(), Some(3));
    let beta = by_provider
        .iter()
        .find(|b| b["key"].as_str() == Some("beta"))
        .unwrap();
    assert_eq!(beta["count"].as_i64(), Some(2));
}

#[test]
fn stats_by_status_dimension() {
    let f = fixture();
    // base fixture: ids 1,3 = 200; id 2 = 404 -> statuses 200 (2 recs), 404 (1 rec)
    let out = f
        .cmd()
        .arg("stats")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--by")
        .arg("status")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let by_status = v["by_status"].as_array().unwrap();
    assert_eq!(by_status.len(), 2); // 200 and 404
    let ok = by_status
        .iter()
        .find(|b| b["key"].as_str() == Some("200"))
        .unwrap();
    assert_eq!(ok["count"].as_i64(), Some(2));
    let nf = by_status
        .iter()
        .find(|b| b["key"].as_str() == Some("404"))
        .unwrap();
    assert_eq!(nf["count"].as_i64(), Some(1));
}

#[test]
fn stats_status_table_shows_codes() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("stats")
        .arg(&f.cbor_zstd)
        .arg("--by")
        .arg("status")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(s.contains("=== by status ==="), "missing status header");
    assert!(s.contains("200"));
    assert!(s.contains("404"));
}

#[test]
fn stats_provider_table_groups_by_prefix() {
    let f = rich_fixture();
    let out = f
        .cmd()
        .arg("stats")
        .arg(&f.cbor_zstd)
        .arg("--by")
        .arg("provider")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(s.contains("=== by provider ==="), "missing provider header");
    assert!(s.contains("alpha"));
    assert!(s.contains("beta"));
    // should NOT list full model names
    assert!(!s.contains("alpha/one"));
}

#[test]
fn stats_invalid_by_errors() {
    let f = fixture();
    f.cmd()
        .arg("stats")
        .arg(&f.cbor_zstd)
        .arg("--by")
        .arg("bogus")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown --by `bogus`"))
        .stderr(predicate::str::contains("model|provider|path|status"));
}

// ===========================================================================
// directory expansion
// ===========================================================================

#[test]
fn directory_argument_expands_to_files() {
    let f = fixture();
    // Passing the temp dir should pick up fixture.cbor.zstd but not source.ndjson.
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.dir)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    assert_eq!(recs.len(), 3);
}

// ===========================================================================
// merge
// ===========================================================================

#[test]
fn merge_two_files_into_one() {
    let a = Fixture::new();
    let b = rich_fixture();
    let out = a.dir.join("merged.cbor.zstd");
    a.cmd()
        .arg("merge")
        .arg(&a.cbor_zstd)
        .arg(&b.cbor_zstd)
        .arg("-o")
        .arg(&out)
        .assert()
        .success();
    // base fixture = 3 records, rich = 5 records -> 8 total
    let info = a
        .cmd()
        .arg("info")
        .arg(&out)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&info).unwrap();
    assert_eq!(v["totals"]["records"].as_i64(), Some(8));
}

#[test]
fn merge_respects_filter() {
    let f = rich_fixture();
    let out = f.dir.join("merged.cbor.zstd");
    f.cmd()
        .arg("merge")
        .arg(&f.cbor_zstd)
        .arg("-o")
        .arg(&out)
        .arg("--model")
        .arg("alpha/one")
        .assert()
        .success();
    let ls = f
        .cmd()
        .arg("ls")
        .arg(&out)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&ls).unwrap());
    // alpha/one = id1, id3, id4
    assert_eq!(recs.len(), 3);
    assert!(recs
        .iter()
        .all(|r| r["model"].as_str() == Some("alpha/one")));
}

#[test]
fn merge_is_lossless() {
    let f = rich_fixture();
    let out = f.dir.join("merged.cbor.zstd");
    f.cmd()
        .arg("merge")
        .arg(&f.cbor_zstd)
        .arg("-o")
        .arg(&out)
        .assert()
        .success();
    let extracted = f
        .cmd()
        .arg("extract")
        .arg(&out)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let got = read_ndjson(std::str::from_utf8(&extracted).unwrap());
    let want = read_ndjson(RICH_NDJSON);
    assert_eq!(got, want, "merge altered record contents");
}

// ===========================================================================
// split
// ===========================================================================

#[test]
fn split_by_day_produces_per_day_files() {
    let f = rich_fixture();
    let out_dir = f.dir.join("days");
    f.cmd()
        .arg("split")
        .arg(&f.cbor_zstd)
        .arg("--by")
        .arg("day")
        .arg("--out-dir")
        .arg(&out_dir)
        .assert()
        .success();
    let mut days: Vec<String> = fs::read_dir(&out_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_str().unwrap().to_string())
        .collect();
    days.sort();
    assert_eq!(
        days,
        vec![
            "2026-03-09.cbor.zstd",
            "2026-03-10.cbor.zstd",
            "2026-03-11.cbor.zstd",
        ]
    );
}

#[test]
fn split_by_day_is_lossless() {
    let f = rich_fixture();
    let out_dir = f.dir.join("days");
    f.cmd()
        .arg("split")
        .arg(&f.cbor_zstd)
        .arg("--by")
        .arg("day")
        .arg("--out-dir")
        .arg(&out_dir)
        .assert()
        .success();
    // re-merge the per-day files and compare to the original.
    let remerged = f.dir.join("remerged.cbor.zstd");
    f.cmd()
        .arg("merge")
        .arg(&out_dir)
        .arg("-o")
        .arg(&remerged)
        .assert()
        .success();
    let got = f
        .cmd()
        .arg("extract")
        .arg(&remerged)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let got_recs = read_ndjson(std::str::from_utf8(&got).unwrap());
    let want = read_ndjson(RICH_NDJSON);
    // records arrive grouped by day; compare as id-sorted sets.
    let mut g = got_recs.clone();
    let mut w = want.clone();
    g.sort_by_key(|r| r["id"].as_i64().unwrap());
    w.sort_by_key(|r| r["id"].as_i64().unwrap());
    assert_eq!(g, w, "split->merge not lossless");
}

#[test]
fn split_by_session_default_skips_singletons() {
    let f = rich_fixture();
    let out_dir = f.dir.join("sessions");
    f.cmd()
        .arg("split")
        .arg(&f.cbor_zstd)
        .arg("--by")
        .arg("session")
        .arg("--out-dir")
        .arg(&out_dir)
        .assert()
        .success();
    // only sess-1 has >=2 records; the rest are singletons (skipped by default)
    let files: Vec<String> = fs::read_dir(&out_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_str().unwrap().to_string())
        .collect();
    assert_eq!(files, vec!["sess-1.cbor.zstd"]);
    let ls = f
        .cmd()
        .arg("ls")
        .arg(out_dir.join("sess-1.cbor.zstd"))
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&ls).unwrap());
    let mut ids: Vec<i64> = recs.iter().map(|r| r["id"].as_i64().unwrap()).collect();
    ids.sort();
    assert_eq!(ids, vec![1, 4]);
}

#[test]
fn split_by_session_min_records_1_emits_all() {
    let f = rich_fixture();
    let out_dir = f.dir.join("sessions_all");
    f.cmd()
        .arg("split")
        .arg(&f.cbor_zstd)
        .arg("--by")
        .arg("session")
        .arg("--min-records")
        .arg("1")
        .arg("--out-dir")
        .arg(&out_dir)
        .assert()
        .success();
    let count = fs::read_dir(&out_dir).unwrap().count();
    assert_eq!(count, 4); // sess-1..sess-4
}

#[test]
fn split_by_session_groups_by_first_user_message_not_session_id() {
    // Aperture gives every request a unique session_id, so --by session must
    // group by the conversation root (first user message), not the raw field.
    // Here four records share NO session_ids but form two conversations:
    //   - "how do I bake bread" asked twice (a retry), same system prompt
    //   - "how do I cook rice" asked twice, same system prompt
    // They must split into 2 files (not 4), titled by the first user message.
    let body_bread_a = serde_json::json!({"messages":[
        {"role":"system","content":"You are a chef."},
        {"role":"user","content":"how do I bake bread"}
    ]})
    .to_string();
    let body_bread_b = serde_json::json!({"messages":[
        {"role":"system","content":"You are a chef."},
        {"role":"user","content":"how do I bake bread"},
        {"role":"assistant","content":"knead it"},
        {"role":"user","content":"how long"}
    ]})
    .to_string();
    let body_rice_a = serde_json::json!({"messages":[
        {"role":"system","content":"You are a chef."},
        {"role":"user","content":"how do I cook rice"}
    ]})
    .to_string();
    let body_rice_b = serde_json::json!({"messages":[
        {"role":"system","content":"You are a chef."},
        {"role":"user","content":"how do I cook rice"},
        {"role":"assistant","content":"rinse it"},
        {"role":"user","content":"then what"}
    ]})
    .to_string();
    let nd = format!(
        "{}\n{}\n{}\n{}\n",
        serde_json::json!({"id":1,"model":"m","path":"/p","session_id":"UNIQUE-1","status_code":200,"capture":{"requestBody":body_bread_a}}).to_string(),
        serde_json::json!({"id":2,"model":"m","path":"/p","session_id":"UNIQUE-2","status_code":200,"capture":{"requestBody":body_rice_a}}).to_string(),
        serde_json::json!({"id":3,"model":"m","path":"/p","session_id":"UNIQUE-3","status_code":200,"capture":{"requestBody":body_bread_b}}).to_string(),
        serde_json::json!({"id":4,"model":"m","path":"/p","session_id":"UNIQUE-4","status_code":200,"capture":{"requestBody":body_rice_b}}).to_string(),
    );
    let f = Fixture::from_ndjson(&nd);
    let out_dir = f.dir.join("sessions");
    f.cmd()
        .arg("split")
        .arg(&f.cbor_zstd)
        .arg("--by")
        .arg("session")
        .arg("--out-dir")
        .arg(&out_dir)
        .assert()
        .success();
    let files: Vec<String> = fs::read_dir(&out_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_str().unwrap().to_string())
        .collect();
    // Two conversations, each with 2 records (>= default min_records=2).
    assert_eq!(
        files.len(),
        2,
        "grouped by first user message, not session_id: {files:?}"
    );
    // Filenames are titled by the first user message (not the system prompt).
    let joined = files.join(" ");
    assert!(
        joined.contains("how_do_I_bake_bread"),
        "bread conversation titled by user message: {joined}"
    );
    assert!(
        joined.contains("how_do_I_cook_rice"),
        "rice conversation titled by user message: {joined}"
    );
    assert!(
        !joined.contains("You_are_a_chef"),
        "system prompt must NOT be the title: {joined}"
    );
}

#[test]
fn split_by_model() {
    let f = rich_fixture();
    let out_dir = f.dir.join("models");
    f.cmd()
        .arg("split")
        .arg(&f.cbor_zstd)
        .arg("--by")
        .arg("model")
        .arg("--out-dir")
        .arg(&out_dir)
        .assert()
        .success();
    let mut files: Vec<String> = fs::read_dir(&out_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_str().unwrap().to_string())
        .collect();
    files.sort();
    // `/` in model names is sanitized to `_`
    assert_eq!(files, vec!["alpha_one.cbor.zstd", "beta_two.cbor.zstd"]);
}

#[test]
fn split_by_provider() {
    let f = rich_fixture();
    let out_dir = f.dir.join("providers");
    f.cmd()
        .arg("split")
        .arg(&f.cbor_zstd)
        .arg("--by")
        .arg("provider")
        .arg("--out-dir")
        .arg(&out_dir)
        .assert()
        .success();
    let mut files: Vec<String> = fs::read_dir(&out_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_str().unwrap().to_string())
        .collect();
    files.sort();
    // rich fixture: alpha/one and beta/two -> providers alpha, beta
    assert_eq!(files, vec!["alpha.cbor.zstd", "beta.cbor.zstd"]);

    // the alpha provider file should contain id1, id3, id4 (all alpha/one)
    let ls = f
        .cmd()
        .arg("ls")
        .arg(out_dir.join("alpha.cbor.zstd"))
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&ls).unwrap());
    let mut ids: Vec<i64> = recs.iter().map(|r| r["id"].as_i64().unwrap()).collect();
    ids.sort();
    assert_eq!(ids, vec![1, 3, 4]);
}

#[test]
fn split_preserves_bytes() {
    let f = rich_fixture();
    let out_dir = f.dir.join("days");
    f.cmd()
        .arg("split")
        .arg(&f.cbor_zstd)
        .arg("--by")
        .arg("day")
        .arg("--out-dir")
        .arg(&out_dir)
        .assert()
        .success();
    // id1 is on 2026-03-09 and carries a bytes body; it must survive the split.
    let out = f
        .cmd()
        .arg("extract")
        .arg(out_dir.join("2026-03-09.cbor.zstd"))
        .arg("--fields")
        .arg("id,capture.rawRequestBody")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    let id1 = recs.iter().find(|r| r["id"].as_i64() == Some(1)).unwrap();
    assert_eq!(
        id1["capture.rawRequestBody"]["__cbor_bytes_b64"].as_str(),
        Some("eyJyYXciOnRydWV9"),
        "bytes body lost across split"
    );
}

#[test]
fn split_json_manifest() {
    let f = rich_fixture();
    let out_dir = f.dir.join("days");
    let manifest = f
        .cmd()
        .arg("split")
        .arg(&f.cbor_zstd)
        .arg("--by")
        .arg("day")
        .arg("--out-dir")
        .arg(&out_dir)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
    assert_eq!(v["scanned"].as_i64(), Some(5));
    assert_eq!(v["written"].as_i64(), Some(3));
    assert_eq!(v["distinct_groups"].as_i64(), Some(3));
    let groups = v["groups"].as_array().unwrap();
    let d09 = groups
        .iter()
        .find(|g| g["key"].as_str() == Some("2026-03-09"))
        .unwrap();
    assert_eq!(d09["records"].as_i64(), Some(2));
    assert_eq!(d09["file"].as_str(), Some("2026-03-09.cbor.zstd"));
    assert!(out_dir.join("2026-03-09.cbor.zstd").exists());
}

#[test]
fn split_no_qualifying_groups_reports_zero() {
    let f = rich_fixture();
    let out_dir = f.dir.join("empty");
    // min-records huge -> nothing qualifies, exit success, no files written
    f.cmd()
        .arg("split")
        .arg(&f.cbor_zstd)
        .arg("--by")
        .arg("session")
        .arg("--min-records")
        .arg("9999")
        .arg("--out-dir")
        .arg(&out_dir)
        .assert()
        .success();
    if out_dir.exists() {
        assert_eq!(fs::read_dir(&out_dir).unwrap().count(), 0);
    }
}

// ===========================================================================
// provider filter (--provider)
// ===========================================================================

#[test]
fn filter_provider_matches_prefix() {
    let f = rich_fixture();
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--provider")
        .arg("alpha")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    // alpha/one = id1, id3, id4
    let mut ids: Vec<i64> = recs.iter().map(|r| r["id"].as_i64().unwrap()).collect();
    ids.sort();
    assert_eq!(ids, vec![1, 3, 4]);
}

#[test]
fn filter_provider_repeatable() {
    let f = rich_fixture();
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--provider")
        .arg("alpha")
        .arg("--provider")
        .arg("beta")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    assert_eq!(recs.len(), 5); // all records are alpha or beta
}

#[test]
fn filter_provider_invert() {
    let f = rich_fixture();
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--provider")
        .arg("alpha")
        .arg("--invert")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    let mut ids: Vec<i64> = recs.iter().map(|r| r["id"].as_i64().unwrap()).collect();
    ids.sort();
    // drop alpha records -> keep beta/two = id2, id5
    assert_eq!(ids, vec![2, 5]);
}

#[test]
fn filter_provider_no_match_for_bare_model() {
    // A model without '/' has no provider; --provider should never match it.
    let ndjson = r#"{"id":1,"model":"claude-haiku-4-5","path":"/p","session_id":"s","status_code":200,"usage":{},"capture":{}}"#;
    let f = Fixture::from_ndjson(ndjson);
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--provider")
        .arg("claude-haiku-4-5")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    assert!(recs.is_empty(), "bare model should not match --provider");
}

#[test]
fn filter_provider_partial_does_not_match() {
    // --provider ol should NOT match ollama/... (exact match, not prefix).
    let f = rich_fixture();
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--provider")
        .arg("alph")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    assert!(recs.is_empty(), "partial provider should not match");
}

#[test]
fn filter_provider_combines_with_model() {
    // --provider alpha AND --model alpha/one are consistent -> same as just --model.
    let f = rich_fixture();
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--provider")
        .arg("alpha")
        .arg("--model")
        .arg("alpha/one")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    assert_eq!(recs.len(), 3); // all alpha/one
}

// ===========================================================================
// timestamp filters (--since / --until / --date)
// ===========================================================================

#[test]
fn filter_date_selects_one_day() {
    let f = rich_fixture();
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--date")
        .arg("2026-03-09")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    let mut ids: Vec<i64> = recs.iter().map(|r| r["id"].as_i64().unwrap()).collect();
    ids.sort();
    assert_eq!(ids, vec![1, 4]);
}

#[test]
fn filter_since_inclusive_prefix() {
    let f = rich_fixture();
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--since")
        .arg("2026-03-11")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    let mut ids: Vec<i64> = recs.iter().map(|r| r["id"].as_i64().unwrap()).collect();
    ids.sort();
    assert_eq!(ids, vec![3, 5]); // only 2026-03-11 records
}

#[test]
fn filter_until_bare_date_is_inclusive() {
    let f = rich_fixture();
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--until")
        .arg("2026-03-09")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    let mut ids: Vec<i64> = recs.iter().map(|r| r["id"].as_i64().unwrap()).collect();
    ids.sort();
    // bare date --until includes the WHOLE day -> id1 and id4
    assert_eq!(ids, vec![1, 4]);
}

#[test]
fn filter_since_until_range() {
    let f = rich_fixture();
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--since")
        .arg("2026-03-10")
        .arg("--until")
        .arg("2026-03-11")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    let mut ids: Vec<i64> = recs.iter().map(|r| r["id"].as_i64().unwrap()).collect();
    ids.sort();
    // 2026-03-10 (id2) and 2026-03-11 (id3, id5)
    assert_eq!(ids, vec![2, 3, 5]);
}

#[test]
fn filter_date_combines_with_other_filters() {
    let f = rich_fixture();
    let out = f
        .cmd()
        .arg("ls")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .arg("--date")
        .arg("2026-03-09")
        .arg("--model")
        .arg("alpha/one")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    // both id1 and id4 are alpha/one on 2026-03-09
    assert_eq!(recs.len(), 2);
}

// ===========================================================================
// grep
// ===========================================================================

#[test]
fn grep_finds_matching_records() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("grep")
        .arg("hello")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    // "hello world" is in id=1's requestBody
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0]["id"].as_i64(), Some(1));
}

#[test]
fn grep_case_insensitive() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("grep")
        .arg("-i")
        .arg("HELLO")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0]["id"].as_i64(), Some(1));
}

#[test]
fn grep_case_sensitive_no_match() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("grep")
        .arg("HELLO")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let recs: Vec<serde_json::Value> = read_ndjson(std::str::from_utf8(&out).unwrap());
    assert!(recs.is_empty());
}

#[test]
fn grep_count_mode() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("grep")
        .arg("hello")
        .arg(&f.cbor_zstd)
        .arg("--count")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(std::str::from_utf8(&out).unwrap().trim(), "1");
}

#[test]
fn grep_show_matches_shows_snippet() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("grep")
        .arg("hello")
        .arg(&f.cbor_zstd)
        .arg("--show-matches")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(s.contains("1:"), "should show record id");
    assert!(s.contains("hello"), "should show match snippet");
}

#[test]
fn grep_field_narrows_scope() {
    let f = fixture();
    // "hello world" is in capture.requestBody but NOT capture.responseBody.
    let out = f
        .cmd()
        .arg("grep")
        .arg("hello")
        .arg(&f.cbor_zstd)
        .arg("--field")
        .arg("capture.responseBody")
        .arg("--count")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(std::str::from_utf8(&out).unwrap().trim(), "0");

    let out = f
        .cmd()
        .arg("grep")
        .arg("hello")
        .arg(&f.cbor_zstd)
        .arg("--field")
        .arg("capture.requestBody")
        .arg("--count")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(std::str::from_utf8(&out).unwrap().trim(), "1");
}

#[test]
fn grep_searches_bytes_bodies() {
    // rawRequestBody is CBOR bytes; grep should still find content in it.
    let f = fixture();
    let out = f
        .cmd()
        .arg("grep")
        .arg("raw")
        .arg(&f.cbor_zstd)
        .arg("--count")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    // id=1 has rawRequestBody = {"raw":true}
    assert_eq!(std::str::from_utf8(&out).unwrap().trim(), "1");
}

#[test]
fn grep_respects_filter() {
    let f = fixture();
    // "hello" is in id=1 (alpha/one); searching with --model beta/two finds nothing.
    let out = f
        .cmd()
        .arg("grep")
        .arg("hello")
        .arg(&f.cbor_zstd)
        .arg("--model")
        .arg("beta/two")
        .arg("--count")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(std::str::from_utf8(&out).unwrap().trim(), "0");
}

#[test]
fn grep_no_matches_exit_zero() {
    let f = fixture();
    f.cmd()
        .arg("grep")
        .arg("zzzznotfound")
        .arg(&f.cbor_zstd)
        .arg("--count")
        .assert()
        .success()
        .stdout("0\n");
}

#[test]
fn grep_table_shows_count_footer_on_stdout() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("grep")
        .arg("hello")
        .arg(&f.cbor_zstd)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(
        s.contains("1 matching record(s)"),
        "count footer missing from stdout"
    );
}

#[test]
fn grep_show_matches_shows_count_footer_on_stdout() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("grep")
        .arg("hello")
        .arg(&f.cbor_zstd)
        .arg("--show-matches")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(
        s.contains("1 matching record(s)"),
        "count footer missing from stdout"
    );
}

#[test]
fn grep_json_keeps_count_on_stderr() {
    let f = fixture();
    let output = f
        .cmd()
        .arg("grep")
        .arg("hello")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .clone();
    // stdout: only NDJSON records, no count line
    let stdout_recs = read_ndjson(std::str::from_utf8(&output.stdout).unwrap());
    assert_eq!(stdout_recs.len(), 1);
    assert!(
        !std::str::from_utf8(&output.stdout)
            .unwrap()
            .contains("matching record(s)"),
        "count leaked into stdout in json mode"
    );
    // stderr: the count footer
    assert!(
        std::str::from_utf8(&output.stderr)
            .unwrap()
            .contains("1 matching record(s)"),
        "count footer missing from stderr in json mode"
    );
}

// ===========================================================================
// verify
// ===========================================================================

#[test]
fn verify_good_file_passes() {
    let f = fixture();
    f.cmd()
        .arg("verify")
        .arg(&f.cbor_zstd)
        .assert()
        .success()
        .stdout(predicate::str::contains("ok"))
        .stdout(predicate::str::contains("3 records"));
}

#[test]
fn verify_corrupted_file_fails() {
    let f = fixture();
    // Truncate the file mid-stream.
    let data = fs::read(&f.cbor_zstd).unwrap();
    let corrupt = f.dir.join("corrupt.cbor.zstd");
    fs::write(&corrupt, &data[..data.len() / 2]).unwrap();

    f.cmd()
        .arg("verify")
        .arg(&corrupt)
        .assert()
        .failure()
        .stdout(predicate::str::contains("FAIL"));
}

#[test]
fn verify_json_reports_ok() {
    let f = fixture();
    let out = f
        .cmd()
        .arg("verify")
        .arg(&f.cbor_zstd)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["all_ok"].as_bool(), Some(true));
    assert_eq!(v["results"][0]["ok"].as_bool(), Some(true));
    assert_eq!(v["results"][0]["records"].as_i64(), Some(3));
    assert!(v["results"][0]["error"].is_null());
}

#[test]
fn verify_json_reports_failure() {
    let f = fixture();
    let data = fs::read(&f.cbor_zstd).unwrap();
    let corrupt = f.dir.join("corrupt2.cbor.zstd");
    fs::write(&corrupt, &data[..data.len() / 2]).unwrap();

    let out = f
        .cmd()
        .arg("verify")
        .arg(&corrupt)
        .arg("--json")
        .assert()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["all_ok"].as_bool(), Some(false));
    assert_eq!(v["results"][0]["ok"].as_bool(), Some(false));
    assert!(v["results"][0]["error"].as_str().is_some());
}

// ===========================================================================
// redaction presets
// ===========================================================================

#[test]
fn preset_bearer_redacts_tokens() {
    let f = fixture();
    let out_cz = f.dir.join("redacted.cbor.zstd");
    f.cmd()
        .arg("edit")
        .arg(&f.cbor_zstd)
        .arg("-o")
        .arg(&out_cz)
        .arg("--redact-preset")
        .arg("bearer")
        .assert()
        .success();
    let out = f
        .cmd()
        .arg("extract")
        .arg(&out_cz)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    // id=1 has "Bearer hunter2-secret" in requestHeaders
    assert!(!s.contains("hunter2-secret"), "bearer token not redacted");
    assert!(s.contains("[REDACTED]"));
}

#[test]
fn preset_email_redacts_addresses() {
    let ndjson = r#"{"id":1,"model":"m","path":"/p","session_id":"s","status_code":200,"usage":{},"capture":{"requestBody":"contact me at alice@example.com please","responseBody":"sent to bob@test.org"}}"#;
    let f = Fixture::from_ndjson(ndjson);
    let out_cz = f.dir.join("redacted.cbor.zstd");
    f.cmd()
        .arg("edit")
        .arg(&f.cbor_zstd)
        .arg("-o")
        .arg(&out_cz)
        .arg("--redact-preset")
        .arg("email")
        .assert()
        .success();
    let out = f
        .cmd()
        .arg("extract")
        .arg(&out_cz)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(!s.contains("alice@example.com"), "email not redacted");
    assert!(!s.contains("bob@test.org"), "email not redacted");
    assert!(s.contains("[REDACTED]"));
}

#[test]
fn preset_jwt_redacts_tokens() {
    let jwt =
        "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTYifQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
    let ndjson = format!(
        r#"{{"id":1,"model":"m","path":"/p","session_id":"s","status_code":200,"usage":{{}},"capture":{{"requestBody":"token={jwt}"}}}}"#
    );
    let f = Fixture::from_ndjson(&ndjson);
    let out_cz = f.dir.join("redacted.cbor.zstd");
    f.cmd()
        .arg("edit")
        .arg(&f.cbor_zstd)
        .arg("-o")
        .arg(&out_cz)
        .arg("--redact-preset")
        .arg("jwt")
        .assert()
        .success();
    let out = f
        .cmd()
        .arg("extract")
        .arg(&out_cz)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(!s.contains(jwt), "JWT not redacted");
}

#[test]
fn preset_all_combines_everything() {
    let ndjson = r#"{"id":1,"model":"m","path":"/p","session_id":"s","status_code":200,"usage":{},"capture":{"requestBody":"key=sk-projabcdefghijklmnopqrstuvwxyz and alice@test.com"}}"#;
    let f = Fixture::from_ndjson(ndjson);
    let out_cz = f.dir.join("redacted.cbor.zstd");
    f.cmd()
        .arg("edit")
        .arg(&f.cbor_zstd)
        .arg("-o")
        .arg(&out_cz)
        .arg("--redact-preset")
        .arg("all")
        .assert()
        .success();
    let out = f
        .cmd()
        .arg("extract")
        .arg(&out_cz)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(
        !s.contains("sk-projabcdefghijklmnopqrstuvwxyz"),
        "apikey not redacted"
    );
    assert!(!s.contains("alice@test.com"), "email not redacted");
}

#[test]
fn preset_combines_with_explicit_redact() {
    let f = fixture();
    let out_cz = f.dir.join("redacted.cbor.zstd");
    f.cmd()
        .arg("edit")
        .arg(&f.cbor_zstd)
        .arg("-o")
        .arg(&out_cz)
        .arg("--redact")
        .arg("clee@github")
        .arg("--redact-preset")
        .arg("bearer")
        .assert()
        .success();
    let out = f
        .cmd()
        .arg("extract")
        .arg(&out_cz)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = std::str::from_utf8(&out).unwrap();
    assert!(!s.contains("clee@github"), "explicit redact not applied");
    assert!(!s.contains("hunter2-secret"), "preset bearer not applied");
}

#[test]
fn preset_invalid_name_errors_with_list() {
    let f = fixture();
    let err = f
        .cmd()
        .arg("edit")
        .arg(&f.cbor_zstd)
        .arg("-o")
        .arg(f.dir.join("x.cbor.zstd"))
        .arg("--redact-preset")
        .arg("nonexistent")
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let e = std::str::from_utf8(&err).unwrap();
    assert!(e.contains("unknown redact preset"));
    assert!(e.contains("email"));
    assert!(e.contains("jwt"));
    assert!(e.contains("all"));
}

// ===========================================================================
// smoke test against the real prod/ dump (opt-in, slow, needs data present)
// ===========================================================================

#[test]
#[ignore = "requires real export data in prod/; run with: cargo test -- --ignored"]
fn prod_roundtrip_is_lossless_on_small_files() {
    let prod = std::path::Path::new("prod");
    if !prod.is_dir() {
        eprintln!("no prod/ dir; skipping");
        return;
    }
    let mut files: Vec<_> = fs::read_dir(prod)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(".cbor.zstd"))
                .unwrap_or(false)
        })
        .collect();
    files.sort();

    let dir = tempfile::TempDir::new().unwrap();
    for fz in &files {
        // Skip the very large 2000-record file to keep this snappy.
        if std::fs::metadata(fz)
            .map(|m| m.len() > 5_000_000)
            .unwrap_or(false)
        {
            continue;
        }
        let nd = dir.path().join(format!(
            "{}.ndjson",
            fz.file_stem().unwrap().to_str().unwrap()
        ));
        Command::cargo_bin("czsplicer")
            .unwrap()
            .arg("extract")
            .arg(fz)
            .arg("-o")
            .arg(&nd)
            .assert()
            .success();
        let cz = dir.path().join(format!(
            "{}.rt.cbor.zstd",
            fz.file_stem().unwrap().to_str().unwrap()
        ));
        Command::cargo_bin("czsplicer")
            .unwrap()
            .arg("repack")
            .arg(&nd)
            .arg("-o")
            .arg(&cz)
            .assert()
            .success();
        // Re-extract both and compare semantically.
        let a = Command::cargo_bin("czsplicer")
            .unwrap()
            .arg("extract")
            .arg(fz)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let b = Command::cargo_bin("czsplicer")
            .unwrap()
            .arg("extract")
            .arg(&cz)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let av = read_ndjson(std::str::from_utf8(&a).unwrap());
        let bv = read_ndjson(std::str::from_utf8(&b).unwrap());
        assert_eq!(
            av.len(),
            bv.len(),
            "record count changed for {}",
            fz.display()
        );
        assert_eq!(av, bv, "round-trip mismatch for {}", fz.display());
        println!("OK: {} ({} records)", fz.display(), av.len());
    }
}

// ===========================================================================
// thread (JSON reconstruction)
// ===========================================================================

/// the parsed JSON forest.
fn thread_json(ndjson: &str) -> serde_json::Value {
    let f = Fixture::from_ndjson(ndjson);
    let out = Command::cargo_bin("czsplicer")
        .unwrap()
        .arg("thread")
        .arg(&f.cbor_zstd)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&out).expect("valid json forest")
}

/// Collect all (role, depth) nodes in tree order via iterative DFS.
fn flatten(trees: &serde_json::Value) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    let mut stack: Vec<(&serde_json::Value, usize)> = trees
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t, 0usize))
        .collect();
    while let Some((n, dp)) = stack.pop() {
        let role = n["role"].as_str().unwrap_or("").to_string();
        let nkids = n["children"].as_array().map(|a| a.len()).unwrap_or(0);
        out.push((role, dp, nkids));
        if let Some(kids) = n["children"].as_array() {
            for c in kids.iter().rev() {
                stack.push((c, dp + 1));
            }
        }
    }
    out
}

/// A request body with the given message history (system + user/assistant turns).
/// Returns the JSON string for `capture.requestBody`.
fn body_with_messages(system: &str, turns: &[(&str, &str)]) -> String {
    let mut msgs = vec![serde_json::json!({"role":"system","content":system})];
    for (role, content) in turns {
        msgs.push(serde_json::json!({"role":role,"content":content}));
    }
    serde_json::json!({"messages":msgs,"model":"test/model"}).to_string()
}

fn rec(id: i64, body: &str) -> String {
    serde_json::json!({
        "id":id,
        "model":"test/model",
        "path":"/v1/messages",
        "status_code":200,
        "capture":{"requestBody":body}
    })
    .to_string()
}

#[test]
fn thread_linear_chain_is_single_path() {
    // Three records, each extending the same conversation: a single tree,
    // no branches, deepest path = 4 messages (sys + u + a + u).
    let nd = format!(
        "{}\n{}\n{}\n",
        rec(1, &body_with_messages("S", &[("user", "hello")])),
        rec(
            2,
            &body_with_messages(
                "S",
                &[("user", "hello"), ("assistant", "hi"), ("user", "bye")]
            )
        ),
        rec(
            3,
            &body_with_messages(
                "S",
                &[
                    ("user", "hello"),
                    ("assistant", "hi"),
                    ("user", "bye"),
                    ("assistant", "bye")
                ]
            )
        ),
    );
    let j = thread_json(&nd);
    assert_eq!(j["records_total"].as_i64(), Some(3));
    assert_eq!(j["root_count"].as_i64(), Some(1), "one conversation root");
    assert_eq!(
        j["branch_count"].as_i64(),
        Some(0),
        "no branches in a chain"
    );
    // The single root's record_ids should include all three records.
    let root = &j["trees"][0];
    assert_eq!(root["record_ids"].as_array().unwrap().len(), 3);
}

#[test]
fn thread_detects_branch_divergence() {
    // Two records share a prefix [sys, u1] then diverge: one continues with
    // assistant "A", the other with assistant "B". This is a real branch.
    let nd = format!(
        "{}\n{}\n",
        rec(
            1,
            &body_with_messages("S", &[("user", "q"), ("assistant", "answer-one")])
        ),
        rec(
            2,
            &body_with_messages("S", &[("user", "q"), ("assistant", "answer-two")])
        ),
    );
    let j = thread_json(&nd);
    assert_eq!(j["records_total"].as_i64(), Some(2));
    assert_eq!(j["root_count"].as_i64(), Some(1));
    assert_eq!(
        j["branch_count"].as_i64(),
        Some(1),
        "prefix divergence is a branch"
    );
    // The branch node is the assistant turn at depth 2, with 2 children.
    let root = &j["trees"][0];
    assert_eq!(root["role"].as_str(), Some("system"));
    assert_eq!(
        root["children"].as_array().unwrap().len(),
        1,
        "single user turn under system"
    );
    let user = &root["children"][0];
    assert_eq!(
        user["children"].as_array().unwrap().len(),
        2,
        "two divergent assistant turns"
    );
}

#[test]
fn thread_separate_system_prompts_are_separate_roots() {
    // Different system prompts => different roots, not a branch.
    let nd = format!(
        "{}\n{}\n",
        rec(1, &body_with_messages("SYS-A", &[("user", "hi")])),
        rec(2, &body_with_messages("SYS-B", &[("user", "hi")])),
    );
    let j = thread_json(&nd);
    assert_eq!(
        j["root_count"].as_i64(),
        Some(2),
        "two distinct system prompts"
    );
    assert_eq!(j["branch_count"].as_i64(), Some(0));
}

#[test]
fn thread_string_and_block_content_hash_identically() {
    // A bare-string user message and the equivalent [{type:text}] block form
    // must hash to the same node, so the two records form a chain, not a branch.
    let msgs_str = serde_json::json!({
        "messages":[
            {"role":"system","content":"S"},
            {"role":"user","content":"same question"}
        ]
    })
    .to_string();
    // Record 2 extends record 1's path by one assistant turn. If the string
    // and block user messages DIDN'T hash equally, we'd get 2 roots instead.
    let nd = format!(
        "{}\n{}\n",
        rec(1, &msgs_str),
        rec(
            2,
            &serde_json::json!({
                "messages":[
                    {"role":"system","content":"S"},
                    {"role":"user","content":[{"type":"text","text":"same question"}]},
                    {"role":"assistant","content":"reply"}
                ]
            })
            .to_string()
        ),
    );
    let j = thread_json(&nd);
    assert_eq!(
        j["root_count"].as_i64(),
        Some(1),
        "string and block content share a root"
    );
    assert_eq!(j["branch_count"].as_i64(), Some(0));
}

#[test]
fn thread_records_without_messages_are_skipped() {
    // A record with no capture.requestBody should not crash and should be
    // excluded from the message count, but still counted in records_total.
    let nd = format!(
        "{}\n{}\n",
        rec(1, &body_with_messages("S", &[("user", "hi")])),
        serde_json::json!({"id":2,"model":"test/model","path":"/v1/messages","status_code":500,"capture":{}}).to_string(),
    );
    let j = thread_json(&nd);
    assert_eq!(j["records_total"].as_i64(), Some(2));
    assert_eq!(j["records_with_messages"].as_i64(), Some(1));
    assert_eq!(j["root_count"].as_i64(), Some(1));
    // Per-record metadata is captured even for the empty-body 500 record.
    assert_eq!(j["records"]["2"]["status_code"].as_i64(), Some(500));
    assert_eq!(j["records"]["1"]["status_code"].as_i64(), Some(200));
}

#[test]
fn thread_record_metadata_carries_status_tools_and_timestamp() {
    // Record 1: a 200 with an assistant response that issues a tool_call
    // (OpenAI choices[0].message.tool_calls shape). Record 2: a 429 failure
    // whose request echoes a tool_result block (Anthropic shape).
    let nd = format!(
        "{}\n{}\n",
        serde_json::json!({
            "id":1,"model":"alpha/one","path":"/v1/x","status_code":200,
            "timestamp":"2026-06-26T00:00:00Z","duration_ms":1234,"api_type":"oai_completions",
            "capture":{
                "requestBody":body_with_messages("S",&[("user","do thing")]),
                "responseBody":serde_json::json!({
                    "choices":[{"message":{"role":"assistant","content":"ok","tool_calls":[
                        {"id":"call_1","type":"function","function":{"name":"f","arguments":"{}"}}
                    ]}}]
                }).to_string()
            }
        }).to_string(),
        serde_json::json!({
            "id":2,"model":"alpha/one","path":"/v1/x","status_code":429,
            "timestamp":"2026-06-26T00:00:01Z","api_type":"ant_messages",
            "capture":{
                "requestBody":serde_json::json!({
                    "messages":[
                        {"role":"system","content":"S"},
                        {"role":"user","content":[{"type":"text","text":"q"}]},
                        {"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"f","input":{}}]},
                        {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"done"}]}
                    ]
                }).to_string()
            }
        }).to_string(),
    );
    let j = thread_json(&nd);
    let r1 = &j["records"]["1"];
    let r2 = &j["records"]["2"];
    assert_eq!(r1["status_code"].as_i64(), Some(200));
    assert_eq!(r1["duration_ms"].as_i64(), Some(1234));
    assert_eq!(
        r1["tool_calls"].as_i64(),
        Some(1),
        "OpenAI tool_call counted"
    );
    assert_eq!(
        r2["status_code"].as_i64(),
        Some(429),
        "failure status preserved"
    );
    assert_eq!(
        r2["tool_results"].as_i64(),
        Some(1),
        "Anthropic tool_result block counted"
    );
}

#[test]
fn thread_tool_results_count_only_new_turn_not_history() {
    // Regression: a request that echoes a long prior history with many
    // tool_result blocks must count only the NEW ones (after the last
    // assistant message), not the accumulated total. Otherwise every turn
    // in a long thread reports the same growing number.
    let nd = serde_json::json!({
        "id":1,"model":"m","path":"/v1/x","status_code":200,
        "capture":{"requestBody":serde_json::json!({
            "messages":[
                {"role":"system","content":"S"},
                {"role":"user","content":"start"},
                // A long echoed history of prior tool results.
                {"role":"assistant","content":[{"type":"tool_use","id":"a","name":"f","input":{}}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"a","content":"r1"}]},
                {"role":"assistant","content":[{"type":"tool_use","id":"b","name":"f","input":{}}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"b","content":"r2"}]},
                {"role":"assistant","content":"done so far"},
                // The NEW turn: exactly one fresh tool result.
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"b","content":"r3"}]}
            ]
        }).to_string()}
    })
    .to_string();
    let j = thread_json(&nd);
    assert_eq!(
        j["records"]["1"]["tool_results"].as_i64(),
        Some(1),
        "only the one tool_result after the last assistant is counted, not the 2 in history"
    );
}

#[test]
fn thread_filter_scopes_records() {
    // Using --model filter should restrict which records contribute to trees.
    let nd = format!(
        "{}\n{}\n",
        serde_json::json!({
            "id":1,"model":"alpha/one","path":"/v1/messages","status_code":200,
            "capture":{"requestBody":body_with_messages("S",&[("user","a")])}
        })
        .to_string(),
        serde_json::json!({
            "id":2,"model":"beta/two","path":"/v1/messages","status_code":200,
            "capture":{"requestBody":body_with_messages("S",&[("user","a"),("assistant","b")])}
        })
        .to_string(),
    );
    let f = Fixture::from_ndjson(&nd);
    let out = Command::cargo_bin("czsplicer")
        .unwrap()
        .arg("thread")
        .arg(&f.cbor_zstd)
        .arg("--model")
        .arg("alpha/one")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let j: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        j["records_total"].as_i64(),
        Some(1),
        "filter kept only alpha/one"
    );
    // Only one record => a shallow tree (sys + user), no branch.
    assert_eq!(j["branch_count"].as_i64(), Some(0));
}

#[test]
fn thread_redact_bad_preset_errors() {
    let nd = rec(1, &body_with_messages("S", &[("user", "x")]));
    let f = Fixture::from_ndjson(&nd);
    Command::cargo_bin("czsplicer")
        .unwrap()
        .arg("thread")
        .arg(&f.cbor_zstd)
        .arg("--redact-preset")
        .arg("nonexistent")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown redact preset"));
}

#[test]
fn thread_survives_redaction_that_breaks_json_body() {
    // If a redact regex corrupts a JSON escape sequence inside
    // capture.requestBody, the tree builder must skip that body gracefully
    // rather than failing the whole run with "invalid escape".
    let nd = serde_json::json!({
        "id":1,"model":"m","path":"/v1/x","status_code":200,
        "capture":{"requestBody":"{\"messages\":[{\"role\":\"user\",\"content\":\"a\\\\npath/secret\"}]}\""}
    })
    .to_string();
    let f = Fixture::from_ndjson(&nd);
    // A redact pattern that, if applied to the raw JSON text, leaves a dangling
    // escape. The command must still exit successfully (the record is skipped).
    Command::cargo_bin("czsplicer")
        .unwrap()
        .arg("thread")
        .arg(&f.cbor_zstd)
        .arg("--redact")
        .arg("secret")
        .assert()
        .success();
}
