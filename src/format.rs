use anyhow::{anyhow, Result};
use serde_json::{Map as JsonMap, Number, Value as Json};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;

type ZDecoder = zstd::Decoder<'static, BufReader<File>>;

/// Marker used to losslessly carry CBOR `bytes` through JSON.
/// A bytes value is encoded as `{"__cbor_bytes_b64": "<base64>"}`.
pub const BYTES_KEY: &str = "__cbor_bytes_b64";
/// Marker for CBOR tags: `{"__cbor_tag": [<u64>, <value>]}`.
pub const TAG_KEY: &str = "__cbor_tag";

// ---------------------------------------------------------------------------
// CBOR <-> JSON bridge (lossless: preserves int-vs-float and bytes-vs-text)
// ---------------------------------------------------------------------------

pub fn cbor_to_json(v: &ciborium::Value) -> Json {
    use ciborium::Value::*;
    match v {
        Integer(i) => {
            let n: i128 = (*i).into();
            if let Ok(x) = i64::try_from(n) {
                Json::Number(x.into())
            } else if let Ok(x) = u64::try_from(n) {
                Json::Number(x.into())
            } else {
                Number::from_f64(n as f64)
                    .map(Json::Number)
                    .unwrap_or(Json::Null)
            }
        }
        Float(f) => Number::from_f64(*f).map(Json::Number).unwrap_or(Json::Null),
        Text(s) => Json::String(s.clone()),
        Bytes(b) => {
            let mut m = JsonMap::new();
            m.insert(BYTES_KEY.into(), Json::String(b64_encode(b)));
            Json::Object(m)
        }
        Bool(b) => Json::Bool(*b),
        Null => Json::Null,
        Array(a) => Json::Array(a.iter().map(cbor_to_json).collect()),
        Map(m) => {
            let mut obj = JsonMap::new();
            for (k, val) in m {
                let key = key_to_string(k);
                obj.insert(key, cbor_to_json(val));
            }
            Json::Object(obj)
        }
        Tag(t, inner) => {
            let mut m = JsonMap::new();
            m.insert(
                TAG_KEY.into(),
                Json::Array(vec![Json::Number((*t as u64).into()), cbor_to_json(inner)]),
            );
            Json::Object(m)
        }
        _ => Json::Null,
    }
}

fn key_to_string(k: &ciborium::Value) -> String {
    use ciborium::Value::*;
    match k {
        Text(s) => s.clone(),
        Integer(i) => {
            let n: i128 = (*i).into();
            n.to_string()
        }
        Bool(b) => b.to_string(),
        Null => "null".to_string(),
        _ => serde_json::to_string(&cbor_to_json(k)).unwrap_or_else(|_| "\"?\"".into()),
    }
}

pub fn json_to_cbor(v: &Json) -> ciborium::Value {
    use ciborium::Value::*;
    match v {
        Json::Null => Null,
        Json::Bool(b) => Bool(*b),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                Integer(i.into())
            } else if let Some(u) = n.as_u64() {
                Integer(u.into())
            } else {
                Float(n.as_f64().unwrap_or(0.0))
            }
        }
        Json::String(s) => Text(s.clone()),
        Json::Array(a) => Array(a.iter().map(json_to_cbor).collect()),
        Json::Object(m) => {
            if m.len() == 1 {
                if let Some(Json::String(b64)) = m.get(BYTES_KEY) {
                    if let Ok(bytes) = b64_decode(b64) {
                        return Bytes(bytes);
                    }
                }
            }
            if m.len() == 1 {
                if let Some(Json::Array(arr)) = m.get(TAG_KEY) {
                    if arr.len() == 2 {
                        if let Some(Json::Number(t)) = arr.get(0) {
                            if let Some(tag) = t.as_u64() {
                                return Tag(tag, Box::new(json_to_cbor(&arr[1])));
                            }
                        }
                    }
                }
            }
            let mut out = Vec::with_capacity(m.len());
            for (k, val) in m {
                out.push((Text(k.clone()), json_to_cbor(val)));
            }
            Map(out)
        }
    }
}

fn b64_encode(b: &[u8]) -> String {
    use base64::{engine::general_purpose, Engine as _};
    general_purpose::STANDARD.encode(b)
}
fn b64_decode(s: &str) -> Result<Vec<u8>> {
    use base64::{engine::general_purpose, Engine as _};
    general_purpose::STANDARD
        .decode(s)
        .map_err(|e| anyhow!("invalid base64 in {BYTES_KEY}: {e}"))
}

// ---------------------------------------------------------------------------
// Streaming record IO over .cbor.zstd files
// ---------------------------------------------------------------------------

/// A reader that counts how many decompressed bytes have passed through it.
pub struct Counting<R> {
    inner: R,
    pub bytes: u64,
}
impl<R> Counting<R> {
    pub fn new(inner: R) -> Self {
        Self { inner, bytes: 0 }
    }
}
impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes += n as u64;
        Ok(n)
    }
}
impl<R: BufRead> BufRead for Counting<R> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.inner.fill_buf()
    }
    fn consume(&mut self, amt: usize) {
        self.inner.consume(amt)
    }
}

/// Streaming iterator over the concatenated top-level CBOR records in a file.
///
/// Generic over the buffered reader so byte counting is an opt-in wrapper
/// rather than a second stream type: `open` is the plain path, `open_counting`
/// wraps the decoder in `Counting` and exposes `decompressed_bytes`.
pub struct RecordStream<R: BufRead> {
    reader: R,
    done: bool,
}

impl<R: BufRead> RecordStream<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            done: false,
        }
    }
}

impl RecordStream<BufReader<ZDecoder>> {
    /// Open a `.cbor.zstd` file as a stream of CBOR records.
    pub fn open(path: &Path) -> Result<Self> {
        let f = File::open(path)?;
        let dec: ZDecoder = zstd::Decoder::new(f)?;
        Ok(Self::new(BufReader::new(dec)))
    }
}

impl RecordStream<BufReader<Counting<ZDecoder>>> {
    /// Like `open`, but also counts decompressed bytes consumed.
    pub fn open_counting(path: &Path) -> Result<Self> {
        let f = File::open(path)?;
        let dec: ZDecoder = zstd::Decoder::new(f)?;
        Ok(Self::new(BufReader::new(Counting::new(dec))))
    }
    /// Total decompressed bytes read so far.
    pub fn decompressed_bytes(&self) -> u64 {
        self.reader.get_ref().bytes
    }
}

impl<R: BufRead> Iterator for RecordStream<R> {
    type Item = Result<ciborium::Value>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        // Peek to distinguish "no more records" from a truncated record.
        match self.reader.fill_buf() {
            Ok(buf) if buf.is_empty() => {
                self.done = true;
                None
            }
            Ok(_) => {
                Some(ciborium::from_reader(&mut self.reader).map_err(|e| anyhow!(format!("{e}"))))
            }
            Err(e) => {
                self.done = true;
                Some(Err(e.into()))
            }
        }
    }
}

/// Writes records as a new `.cbor.zstd` stream at the given compression level.
pub struct ZstdPacker {
    enc: zstd::Encoder<'static, BufWriter<File>>,
}
impl ZstdPacker {
    pub fn create(path: &Path, level: i32) -> Result<Self> {
        let f = File::create(path)?;
        let bw = BufWriter::new(f);
        let enc = zstd::Encoder::new(bw, level)?;
        Ok(Self { enc })
    }
    pub fn write_record(&mut self, v: &ciborium::Value) -> Result<()> {
        ciborium::into_writer(v, &mut self.enc).map_err(|e| anyhow!(format!("{e}")))
    }
    pub fn finish(self) -> Result<()> {
        self.enc.finish().map_err(|e| anyhow!(format!("{e}")))?;
        Ok(())
    }
}

/// Writes a single record to any writer as raw CBOR (no compression).
pub fn write_cbor_record<W: Write + ?Sized>(v: &ciborium::Value, w: &mut W) -> Result<()> {
    ciborium::into_writer(v, w).map_err(|e| anyhow!(format!("{e}")))
}

// ---------------------------------------------------------------------------
// Record field accessors (treat top-level value as a CBOR map)
// ---------------------------------------------------------------------------

pub fn field<'a>(rec: &'a ciborium::Value, key: &str) -> Option<&'a ciborium::Value> {
    if let ciborium::Value::Map(m) = rec {
        for (k, v) in m.iter() {
            if let ciborium::Value::Text(t) = k {
                if t == key {
                    return Some(v);
                }
            }
        }
    }
    None
}

pub fn field_mut<'a>(rec: &'a mut ciborium::Value, key: &str) -> Option<&'a mut ciborium::Value> {
    if let ciborium::Value::Map(m) = rec {
        for (k, v) in m.iter_mut() {
            if let ciborium::Value::Text(t) = k {
                if t == key {
                    return Some(v);
                }
            }
        }
    }
    None
}

pub fn as_str(v: &ciborium::Value) -> Option<String> {
    match v {
        ciborium::Value::Text(s) => Some(s.clone()),
        _ => None,
    }
}

pub fn as_int(v: &ciborium::Value) -> Option<i64> {
    if let ciborium::Value::Integer(i) = v {
        let n: i128 = (*i).into();
        return i64::try_from(n).ok();
    }
    None
}

pub fn as_f64(v: &ciborium::Value) -> Option<f64> {
    match v {
        ciborium::Value::Float(f) => Some(*f),
        ciborium::Value::Integer(i) => {
            let n: i128 = (*i).into();
            Some(n as f64)
        }
        _ => None,
    }
}

pub fn rec_str(rec: &ciborium::Value, key: &str) -> Option<String> {
    field(rec, key).and_then(as_str)
}
pub fn rec_int(rec: &ciborium::Value, key: &str) -> Option<i64> {
    field(rec, key).and_then(as_int)
}

/// Walks a dotted path (e.g. `capture.requestBody`) returning the value.
pub fn path_get<'a>(root: &'a ciborium::Value, path: &str) -> Option<&'a ciborium::Value> {
    let mut cur = root;
    for seg in path.split('.') {
        cur = field(cur, seg)?;
    }
    Some(cur)
}

/// Sets the value at a dotted path to Null. Creates no new keys.
pub fn path_null(root: &mut ciborium::Value, path: &str) -> bool {
    let mut segs = path.split('.');
    let Some(last) = segs.next_back() else {
        return false;
    };
    let mut cur = root;
    for seg in segs {
        let Some(next) = field_mut(cur, seg) else {
            return false;
        };
        cur = next;
    }
    if let Some(slot) = field_mut(cur, last) {
        *slot = ciborium::Value::Null;
        return true;
    }
    false
}

/// Applies a regex substitution to every Text value reachable from `root`.
///
/// Also scrubs `Bytes` whose contents are valid UTF-8 — this is where raw HTTP
/// bodies live (e.g. `capture.rawRequestBody` / `rawResponseBody`), i.e. the
/// most likely place for live secrets. Bytes that are NOT valid UTF-8 are left
/// untouched so binary payloads are never corrupted.
///
/// Note this is slightly narrower than `search_value_strings` (used by `grep`),
/// which decodes Bytes *lossily*: grep can therefore surface an ASCII secret
/// embedded in a partially-invalid body that redaction will not scrub. That gap
/// is deliberate — scrubbing a lossy decode would rewrite the original bytes
/// and corrupt binary formats. For well-formed text bodies (the realistic
/// case) the two agree exactly.
pub fn redact_strings<F: Fn(&str) -> String>(root: &mut ciborium::Value, sub: &F) {
    match root {
        ciborium::Value::Text(s) => {
            *s = sub(s);
        }
        ciborium::Value::Bytes(b) => {
            if let Ok(s) = std::str::from_utf8(b) {
                *b = sub(s).into_bytes();
            }
        }
        ciborium::Value::Array(a) => {
            for v in a.iter_mut() {
                redact_strings(v, sub);
            }
        }
        ciborium::Value::Map(m) => {
            for (_k, v) in m.iter_mut() {
                redact_strings(v, sub);
            }
        }
        _ => {}
    }
}

/// Visits every Text value (and Bytes decoded as lossy UTF-8) reachable from
/// `root`, calling `f` on each. Map keys are skipped (they're field names).
/// Used by `grep` to search record contents.
pub fn search_value_strings<F: FnMut(&str)>(root: &ciborium::Value, f: &mut F) {
    match root {
        ciborium::Value::Text(s) => f(s),
        ciborium::Value::Bytes(b) => f(&String::from_utf8_lossy(b)),
        ciborium::Value::Array(a) => {
            for v in a.iter() {
                search_value_strings(v, f);
            }
        }
        ciborium::Value::Map(m) => {
            for (_k, v) in m.iter() {
                search_value_strings(v, f);
            }
        }
        _ => {}
    }
}
