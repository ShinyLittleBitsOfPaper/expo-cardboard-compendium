//! Record-level patches between two releases of the same epoch.
//!
//! A patch carries: the hashes of the base and target files, the target's meta and
//! ranks table, and every oracle/printing row that is new or whose encoded record changed.
//! Applying it rebuilds the target dataset and writes it with the same deterministic
//! writer, so the result must hash to the target's SHA-256; if it does not (for example a
//! different zstd build), the client should fall back to the full file.

use coqc_format::codec::{Reader, Writer};
use coqc_format::{decode_ranks, encode_ranks, read, write, Dataset, Meta, OracleRecord, PrintingRecord};
use sha2::{Digest, Sha256};

const MAGIC: &[u8; 4] = b"COQP";
const VERSION: u16 = 1;
const LEVEL: i32 = 19;

fn sha(b: &[u8]) -> [u8; 32] {
    Sha256::digest(b).into()
}

#[derive(Clone, Debug, Default)]
pub struct PatchInfo {
    pub oracle_changed: u64,
    pub oracle_appended: u64,
    pub printing_changed: u64,
    pub printing_appended: u64,
    pub patch_bytes: u64,
    pub raw_bytes: u64,
}

fn enc<T>(r: &T, f: impl Fn(&T, &mut Writer)) -> Vec<u8> {
    let mut w = Writer::new();
    f(r, &mut w);
    w.buf
}

fn changes<T>(old: &[T], new: &[T], f: impl Fn(&T, &mut Writer)) -> (Vec<(u32, Vec<u8>)>, u64, u64) {
    let (mut out, mut changed, mut appended) = (Vec::new(), 0, 0);
    for (i, n) in new.iter().enumerate() {
        let nb = enc(n, &f);
        match old.get(i) {
            None => appended += 1,
            Some(o) if enc(o, &f) == nb => continue,
            Some(_) => changed += 1,
        }
        out.push((i as u32, nb));
    }
    (out, changed, appended)
}

pub fn make_patch(old_bytes: &[u8], new_bytes: &[u8]) -> Result<(Vec<u8>, PatchInfo), String> {
    let old = read(old_bytes).map_err(|e| format!("old: {e}"))?;
    let new = read(new_bytes).map_err(|e| format!("new: {e}"))?;
    if old.meta.cards_epoch != new.meta.cards_epoch {
        return Err("different epochs: clients need the full file".into());
    }
    if new.oracle.len() < old.oracle.len() || new.printings.len() < old.printings.len() {
        return Err("rows shrank: clients need the full file".into());
    }
    let mut w = Writer::new();
    w.bytes(&sha(old_bytes));
    w.bytes(&sha(new_bytes));
    w.u32(new.oracle.len() as u32);
    w.u32(new.printings.len() as u32);
    let mut m = Writer::new();
    new.meta.encode(&mut m);
    w.uvarint(m.buf.len() as u64);
    w.bytes(&m.buf);
    let ranks = encode_ranks(&new.ranks);
    w.uvarint(ranks.len() as u64);
    w.bytes(&ranks);
    let (oc, oc_n, oa_n) = changes(&old.oracle, &new.oracle, |r: &OracleRecord, w| r.encode(w));
    let (pc, pc_n, pa_n) = changes(&old.printings, &new.printings, |r: &PrintingRecord, w| r.encode(w));
    for list in [&oc, &pc] {
        w.uvarint(list.len() as u64);
        for (row, bytes) in list.iter() {
            w.u32(*row);
            w.uvarint(bytes.len() as u64);
            w.bytes(bytes);
        }
    }
    let comp = zstd::bulk::compress(&w.buf, LEVEL).map_err(|e| e.to_string())?;
    let mut out = Writer::new();
    out.bytes(MAGIC);
    out.u16(VERSION);
    out.u32(w.buf.len() as u32);
    out.bytes(&comp);
    let info = PatchInfo {
        oracle_changed: oc_n,
        oracle_appended: oa_n,
        printing_changed: pc_n,
        printing_appended: pa_n,
        patch_bytes: out.buf.len() as u64,
        raw_bytes: w.buf.len() as u64,
    };
    Ok((out.buf, info))
}

pub fn apply_patch(old_bytes: &[u8], patch: &[u8]) -> Result<Vec<u8>, String> {
    let e = |x: coqc_format::Error| x.to_string();
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
    let (n_oracle, n_printing) = (r.u32().map_err(e)? as usize, r.u32().map_err(e)? as usize);
    let mlen = r.uvarint().map_err(e)? as usize;
    let meta = Meta::decode(&mut Reader::new(r.take(mlen).map_err(e)?)).map_err(e)?;
    let rlen = r.uvarint().map_err(e)? as usize;
    let ranks = decode_ranks(r.take(rlen).map_err(e)?).map_err(e)?;

    let mut ds: Dataset = read(old_bytes).map_err(e)?;
    // rows not supplied by the patch stay unchanged; new rows start as zero-id placeholders
    ds.oracle.resize_with(n_oracle, || OracleRecord::tombstone([0; 16]));
    ds.printings.resize_with(n_printing, || PrintingRecord::tombstone([0; 16]));
    let n = r.uvarint().map_err(e)?;
    for _ in 0..n {
        let row = r.u32().map_err(e)? as usize;
        let len = r.uvarint().map_err(e)? as usize;
        let rec = OracleRecord::decode(&mut Reader::new(r.take(len).map_err(e)?)).map_err(e)?;
        *ds.oracle.get_mut(row).ok_or("oracle row out of range")? = rec;
    }
    let n = r.uvarint().map_err(e)?;
    for _ in 0..n {
        let row = r.u32().map_err(e)? as usize;
        let len = r.uvarint().map_err(e)? as usize;
        let rec = PrintingRecord::decode(&mut Reader::new(r.take(len).map_err(e)?)).map_err(e)?;
        *ds.printings.get_mut(row).ok_or("printing row out of range")? = rec;
    }
    if ds.oracle.iter().any(|o| o.oracle_id == [0; 16]) || ds.printings.iter().any(|p| p.scryfall_id == [0; 16]) {
        return Err("patch left rows unfilled".into());
    }
    ds.meta = meta;
    ds.ranks = ranks;
    let (out, _) = write(&ds).map_err(e)?;
    if sha(&out) != new_sha {
        return Err("result does not match the expected hash; download the full file".into());
    }
    Ok(out)
}
