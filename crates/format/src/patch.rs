//! Row-level patches between two releases of the same epoch and schema (COQP v2).
//!
//! A patch carries the hashes of the base and target files, the target's meta block (epoch, dictionaries, props, row counts) and,
//! per table, every row that is new or whose encoded bytes changed. Applying it rebuilds the target store and writes it with the
//! same deterministic writer, so the result must hash to the target's SHA-256; if it does not (for example a different zstd
//! build), the client should fall back to the full file.

use crate::codec::{Reader, Writer};
use crate::coqd::{decode_meta, decode_row, encode_meta, read_store, row_bytes, write_store};
use crate::table::{ColData, Table};
use sha2::{Digest, Sha256};

const MAGIC: &[u8; 4] = b"COQP";
const VERSION: u16 = 2;
const LEVEL: i32 = 19;

fn sha(b: &[u8]) -> [u8; 32] {
    Sha256::digest(b).into()
}

#[derive(Clone, Debug, Default)]
pub struct PatchInfo {
    /// (table, rows changed, rows appended)
    pub tables: Vec<(String, u64, u64)>,
    pub patch_bytes: u64,
    pub raw_bytes: u64,
}

impl PatchInfo {
    pub fn changed(&self, table: &str) -> u64 {
        self.tables.iter().find(|t| t.0 == table).map_or(0, |t| t.1)
    }
    pub fn appended(&self, table: &str) -> u64 {
        self.tables.iter().find(|t| t.0 == table).map_or(0, |t| t.2)
    }
}

pub fn make_patch(old_bytes: &[u8], new_bytes: &[u8]) -> Result<(Vec<u8>, PatchInfo), String> {
    let old = read_store(old_bytes).map_err(|e| format!("old: {e}"))?;
    let new = read_store(new_bytes).map_err(|e| format!("new: {e}"))?;
    if old.epoch != new.epoch {
        return Err("different epochs: clients need the full file".into());
    }
    if old.schema != new.schema {
        return Err("different schemas: clients need the full file".into());
    }
    if old.tables.iter().zip(&new.tables).any(|(o, n)| n.rows() < o.rows()) {
        return Err("rows shrank: clients need the full file".into());
    }
    let mut w = Writer::new();
    w.bytes(&sha(old_bytes));
    w.bytes(&sha(new_bytes));
    let mut m = Writer::new();
    encode_meta(&new, &mut m);
    w.uvarint(m.buf.len() as u64);
    w.bytes(&m.buf);
    let mut info = PatchInfo::default();
    for ((def, o), n) in new.schema.tables.iter().zip(&old.tables).zip(&new.tables) {
        let (mut changed, mut appended) = (0u64, 0u64);
        let mut list: Vec<(u32, Vec<u8>)> = Vec::new();
        for i in 0..n.rows() {
            let nb = row_bytes(def, n, i);
            if i < o.rows() {
                if row_bytes(def, o, i) == nb {
                    continue;
                }
                changed += 1;
            } else {
                appended += 1;
            }
            list.push((i as u32, nb));
        }
        w.uvarint(list.len() as u64);
        for (row, bytes) in &list {
            w.u32(*row);
            w.uvarint(bytes.len() as u64);
            w.bytes(bytes);
        }
        info.tables.push((def.name.clone(), changed, appended));
    }
    let comp = zstd::bulk::compress(&w.buf, LEVEL).map_err(|e| e.to_string())?;
    let mut out = Writer::new();
    out.bytes(MAGIC);
    out.u16(VERSION);
    out.u32(w.buf.len() as u32);
    out.bytes(&comp);
    info.patch_bytes = out.buf.len() as u64;
    info.raw_bytes = w.buf.len() as u64;
    Ok((out.buf, info))
}

pub fn apply_patch(old_bytes: &[u8], patch: &[u8]) -> Result<Vec<u8>, String> {
    let e = |x: crate::Error| x.to_string();
    let mut r = Reader::new(patch);
    if r.take(4).map_err(e)? != MAGIC {
        return Err("not a COQP patch".into());
    }
    if r.u16().map_err(e)? != VERSION {
        return Err("unsupported patch version".into());
    }
    let rawlen = r.u32().map_err(e)? as usize;
    let body = zstd::bulk::decompress(&patch[10..], rawlen).map_err(|x| x.to_string())?;
    let mut r = Reader::new(&body);
    let old_sha: [u8; 32] = r.take(32).map_err(e)?.try_into().unwrap();
    let new_sha: [u8; 32] = r.take(32).map_err(e)?.try_into().unwrap();
    if sha(old_bytes) != old_sha {
        return Err("patch is for a different base file".into());
    }
    let mut st = read_store(old_bytes).map_err(e)?;
    let mlen = r.uvarint().map_err(e)? as usize;
    let meta = decode_meta(&st.schema, &mut Reader::new(r.take(mlen).map_err(e)?)).map_err(e)?;
    // rows not supplied by the patch stay unchanged; new rows start as zero-key tombstone placeholders
    for (t, n) in st.tables.iter_mut().zip(&meta.counts) {
        while t.rows() < *n as usize {
            t.push_default();
        }
    }
    for ti in 0..st.tables.len() {
        let def = st.schema.tables[ti].clone();
        let n = r.uvarint().map_err(e)?;
        for _ in 0..n {
            let row = r.u32().map_err(e)? as usize;
            let len = r.uvarint().map_err(e)? as usize;
            let mut scratch = Table::new(&def);
            decode_row(&def, &mut scratch, &mut Reader::new(r.take(len).map_err(e)?)).map_err(e)?;
            if row >= st.tables[ti].rows() {
                return Err(format!("{} row out of range", def.name));
            }
            st.tables[ti].set_row_from(row, &scratch, 0);
        }
    }
    for (def, t) in st.schema.tables.iter().zip(&st.tables) {
        let Some(k) = def.key.as_ref().and_then(|k| def.columns.iter().position(|c| &c.name == k)) else { continue };
        if let ColData::Uuid(keys) = &t.cols[k] {
            if keys.iter().any(|x| *x == [0; 16]) {
                return Err("patch left rows unfilled".into());
            }
        }
    }
    st.epoch = meta.epoch;
    st.source_date = meta.source_date;
    st.dicts = meta.dicts;
    st.props = meta.props;
    let (out, _) = write_store(&st).map_err(e)?;
    if sha(&out) != new_sha {
        return Err("result does not match the expected hash; download the full file".into());
    }
    Ok(out)
}
