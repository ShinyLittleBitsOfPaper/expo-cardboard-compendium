//! Row-level patches between two releases of the same epoch and schema (COQP v2).
//!
//! A patch carries the hashes of the base and target files, the target's meta block (epoch, dictionaries, props, row counts) and,
//! per table, every row that is new or whose encoded bytes changed. Applying it rebuilds the target store and writes it with the
//! same deterministic writer, so the result must hash to the target's SHA-256; if it does not (for example a different zstd
//! build), the client should fall back to the full file.
//!
//! Per table one mode byte: 0 = row list (above), 1 = columnar delta for `TableDef::columnar` tables: the tombstone flags and then
//! every column as `rows` zigzag varints of (new cell code - old cell code), rows past the old table counting as absent (code 0).

use crate::codec::{Reader, Writer};
use crate::coqd::{decode_meta, decode_row, encode_meta, read_store, row_bytes, write_store};
use crate::table::{ColData, Table};
use sha2::{Digest, Sha256};

const MAGIC: &[u8; 4] = b"COQP";
const VERSION: u16 = 2;
const LEVEL: i32 = 19;

fn zz(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}
fn unzz(v: u64) -> i64 {
    (v >> 1) as i64 ^ -((v & 1) as i64)
}
/// Cell code of an Int cell: 0 = absent, else zigzag + 1 (wrapping, so every i64 round-trips through modular arithmetic).
fn code(x: Option<i64>) -> i64 {
    x.map_or(0, |x| zz(x).wrapping_add(1) as i64)
}
fn uncode(c: i64) -> Option<i64> {
    (c != 0).then(|| unzz((c as u64).wrapping_sub(1)))
}
fn int_col(t: &Table, c: usize) -> &Vec<Option<i64>> {
    match &t.cols[c] {
        ColData::Int(v) => v,
        _ => unreachable!("columnar tables only hold Int columns (Schema::validate)"),
    }
}

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
        if def.columnar {
            w.u8(1);
            let (mut changed, appended) = (0u64, (n.rows() - o.rows()) as u64);
            let stream = |old: &dyn Fn(usize) -> i64, new: &dyn Fn(usize) -> i64, w: &mut Writer, changed: &mut u64| {
                for i in 0..n.rows() {
                    let d = new(i).wrapping_sub(old(i));
                    if d != 0 && i < o.rows() {
                        *changed += 1;
                    }
                    w.uvarint(zz(d));
                }
            };
            stream(&|i| o.tomb.get(i).copied().unwrap_or(false) as i64, &|i| n.tomb[i] as i64, &mut w, &mut changed);
            for c in 0..def.columns.len() {
                let (ov, nv) = (int_col(o, c), int_col(n, c));
                stream(&|i| ov.get(i).map_or(0, |x| code(*x)), &|i| code(nv[i]), &mut w, &mut changed);
            }
            // for a columnar table `changed` counts changed cells (all columns), `appended` rows
            info.tables.push((def.name.clone(), changed, appended));
            continue;
        }
        w.u8(0);
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
    let body = crate::codec::zstd_exact(&patch[10..], rawlen)?;
    let mut r = Reader::new(&body);
    let old_sha: [u8; 32] = r.take(32).map_err(e)?.try_into().unwrap();
    let new_sha: [u8; 32] = r.take(32).map_err(e)?.try_into().unwrap();
    if sha(old_bytes) != old_sha {
        return Err("patch is for a different base file".into());
    }
    let mut st = read_store(old_bytes).map_err(e)?;
    let mlen = r.uvarint().map_err(e)? as usize;
    let meta = decode_meta(&st.schema, &mut Reader::new(r.take(mlen).map_err(e)?)).map_err(e)?;
    let old_rows: Vec<usize> = st.tables.iter().map(|t| t.rows()).collect();
    // rows not supplied by the patch stay unchanged; new rows start as zero-key tombstone placeholders
    for (t, n) in st.tables.iter_mut().zip(&meta.counts) {
        // every new row costs the patch at least a byte, so a larger count is corrupt (and must not allocate billions of rows)
        if (*n as usize).saturating_sub(t.rows()) > r.remaining() {
            return Err("patch row count exceeds its size".into());
        }
        while t.rows() < *n as usize {
            t.push_default();
        }
    }
    for ti in 0..st.tables.len() {
        let def = st.schema.tables[ti].clone();
        let mode = r.u8().map_err(e)?;
        if (mode == 1) != def.columnar || mode > 1 {
            return Err(format!("{}: bad patch mode", def.name));
        }
        if mode == 1 {
            let n = st.tables[ti].rows();
            let o = old_rows[ti];
            let mut t = Table::new(&def);
            for i in 0..n {
                let old = if i < o { st.tables[ti].tomb[i] as i64 } else { 0 };
                t.tomb.push(old.wrapping_add(unzz(r.uvarint().map_err(e)?)) != 0);
            }
            for c in 0..def.columns.len() {
                let mut v = Vec::with_capacity(r.cap(n));
                for i in 0..n {
                    let old = if i < o { code(int_col(&st.tables[ti], c)[i]) } else { 0 };
                    v.push(uncode(old.wrapping_add(unzz(r.uvarint().map_err(e)?))));
                }
                t.cols[c] = ColData::Int(v);
            }
            st.tables[ti] = t;
            continue;
        }
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
