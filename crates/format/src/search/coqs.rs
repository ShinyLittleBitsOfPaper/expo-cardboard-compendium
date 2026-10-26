//! COQS: the search index serialized as a compile-time artifact, plus deltas between two artifacts.
//!
//! Layout (little endian): `COQS` | u16 version | dataset SHA-256 (of the .coqd the index was built from) |
//! u32 oracle rows | u32 printing rows | u32 section count | (u64 off, u64 len) per section | sections,
//! each 8-byte aligned and stored raw (fixed-width arrays), so a reader can map it.
//!
//! Delta (`CQSD`): base/target file SHA-256 + zstd body. The body holds, per row table, the changed or
//! appended rows, and per trigram table the stop-gram additions/removals and the changed postings
//! (`append` when the new list extends the old bytes, otherwise `replace`). Replaying a delta on the
//! base must hash to the target.

use super::*;
use crate::codec::{Error, Reader, Result, Writer};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const MAGIC: &[u8; 4] = b"COQS";
const DMAGIC: &[u8; 4] = b"CQSD";
const VERSION: u16 = 1;
const LEVEL: i32 = 19;
const SECTIONS: usize = 13;

pub const SECTION_NAMES: [&str; SECTIONS] = [
    "meta", "names", "names_n", "names_s", "types", "texts", "full_texts", "oracle_cols", "printing_cols", "set", "cn",
    "name_trigrams", "text_trigrams",
];

pub fn sha(b: &[u8]) -> [u8; 32] {
    Sha256::digest(b).into()
}

fn zstd_len(b: &[u8]) -> usize {
    zstd::bulk::compress(b, LEVEL).map(|v| v.len()).unwrap_or(b.len())
}

// ------------------------------------------------------------------ sections ---

fn arena_from_rows(rows: &[Vec<u8>]) -> Result<Arena> {
    let mut a = Arena::default();
    for r in rows {
        a.push(std::str::from_utf8(r).map_err(|_| Error::BadUtf8)?);
    }
    Ok(a)
}

fn w_arena(a: &Arena) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(a.off.len() as u32);
    a.off.iter().for_each(|&o| w.u32(o));
    w.bytes(a.text.as_bytes());
    w.buf
}

fn r_arena(b: &[u8]) -> Result<Arena> {
    let mut r = Reader::new(b);
    let n = r.u32()? as usize;
    let mut off = Vec::with_capacity(n);
    for _ in 0..n {
        off.push(r.u32()?);
    }
    let rest = b.len().checked_sub(4 + 4 * n).ok_or(Error::Eof)?;
    let text = String::from_utf8(r.take(rest)?.to_vec()).map_err(|_| Error::BadUtf8)?;
    let mut prev = 0u32;
    for &o in &off {
        if o < prev || o as usize > text.len() || !text.is_char_boundary(o as usize) {
            return Err(Error::Corrupt("arena offsets"));
        }
        prev = o;
    }
    if off.last().map_or(0, |&o| o as usize) != text.len() {
        return Err(Error::Corrupt("arena length"));
    }
    Ok(Arena { text, off })
}

fn w_tri(t: &Trigrams) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(t.keys.len() as u32);
    w.u32(t.stop.len() as u32);
    w.u32(t.post.len() as u32);
    t.keys.iter().for_each(|&k| w.u32(k));
    if t.start.is_empty() {
        w.u32(0);
    } else {
        t.start.iter().for_each(|&s| w.u32(s));
    }
    t.stop.iter().for_each(|&s| w.u32(s));
    w.bytes(&t.post);
    w.buf
}

fn r_tri(b: &[u8]) -> Result<Trigrams> {
    let mut r = Reader::new(b);
    let (nk, ns, np) = (r.u32()? as usize, r.u32()? as usize, r.u32()? as usize);
    let mut t = Trigrams::default();
    for _ in 0..nk {
        t.keys.push(r.u32()?);
    }
    for _ in 0..=nk {
        t.start.push(r.u32()?);
    }
    for _ in 0..ns {
        t.stop.push(r.u32()?);
    }
    t.post = r.take(np)?.to_vec();
    if t.start.windows(2).any(|w| w[0] > w[1]) || t.start.last().copied() != Some(np as u32) {
        return Err(Error::Corrupt("trigram offsets"));
    }
    if t.keys.windows(2).any(|w| w[0] >= w[1]) || t.stop.windows(2).any(|w| w[0] >= w[1]) {
        return Err(Error::Corrupt("trigram order"));
    }
    Ok(t)
}

fn w_meta(ix: &SearchIndex) -> Vec<u8> {
    let mut w = Writer::new();
    w.uvarint(ix.rarities.len() as u64);
    ix.rarities.iter().for_each(|s| w.str(s));
    w.buf
}

fn w_ocols(ix: &SearchIndex) -> Vec<u8> {
    let mut w = Writer::new();
    ix.o_live.iter().for_each(|&b| w.u8(b as u8));
    ix.o_colors.iter().for_each(|&b| w.u8(b));
    ix.o_identity.iter().for_each(|&b| w.u8(b));
    ix.o_mv.iter().for_each(|m| w.u64(m.unwrap_or(f64::NAN).to_bits()));
    w.buf
}

fn w_pcols(ix: &SearchIndex) -> Vec<u8> {
    let mut w = Writer::new();
    ix.p_oracle.iter().for_each(|&v| w.u32(v));
    ix.p_live.iter().for_each(|&b| w.u8(b as u8));
    (0..ix.p_live.len()).for_each(|i| w.u8(ix.hidden_p.get(i) as u8));
    ix.p_rarity.iter().for_each(|r| w.u32(r.unwrap_or(u32::MAX)));
    ix.p_released.iter().for_each(|&v| w.u32(v));
    w.buf
}

fn str_arena(v: &[String]) -> Arena {
    let mut a = Arena::default();
    v.iter().for_each(|s| a.push(s));
    a
}

fn sections(ix: &SearchIndex) -> Vec<Vec<u8>> {
    vec![
        w_meta(ix),
        w_arena(&ix.names),
        w_arena(&ix.names_n),
        w_arena(&ix.names_s),
        w_arena(&ix.types),
        w_arena(&ix.texts),
        w_arena(&ix.full_texts),
        w_ocols(ix),
        w_pcols(ix),
        w_arena(&str_arena(&ix.p_set)),
        w_arena(&str_arena(&ix.p_cn)),
        w_tri(&ix.name_tri),
        w_tri(&ix.text_tri),
    ]
}

/// Serialize the index. `dataset_sha` is the SHA-256 of the .coqd file it was built from.
pub fn serialize(ix: &SearchIndex, dataset_sha: &[u8; 32]) -> Vec<u8> {
    let secs = sections(ix);
    let mut head = Writer::new();
    head.bytes(MAGIC);
    head.u16(VERSION);
    head.bytes(dataset_sha);
    head.u32(ix.o_live.len() as u32);
    head.u32(ix.p_oracle.len() as u32);
    head.u32(SECTIONS as u32);
    let table_at = head.buf.len();
    let mut off = (table_at + SECTIONS * 16).next_multiple_of(8);
    let mut table = Writer::new();
    for s in &secs {
        table.u64(off as u64);
        table.u64(s.len() as u64);
        off = (off + s.len()).next_multiple_of(8);
    }
    let mut out = head.buf;
    out.extend_from_slice(&table.buf);
    for s in &secs {
        out.resize(out.len().next_multiple_of(8), 0);
        out.extend_from_slice(s);
    }
    out.resize(out.len().next_multiple_of(8), 0);
    out
}

/// One section's raw and zstd size (for the size report).
pub struct SectionSize {
    pub name: &'static str,
    pub raw: usize,
    pub zstd: usize,
}

pub fn section_sizes(ix: &SearchIndex) -> Vec<SectionSize> {
    sections(ix)
        .iter()
        .zip(SECTION_NAMES)
        .map(|(s, name)| SectionSize { name, raw: s.len(), zstd: zstd_len(s) })
        .collect()
}

pub fn compress(bytes: &[u8]) -> Vec<u8> {
    zstd::bulk::compress(bytes, LEVEL).unwrap_or_else(|_| bytes.to_vec())
}

/// Parse a COQS file back into a [`SearchIndex`]; returns the dataset hash it was built from.
pub fn deserialize(b: &[u8]) -> Result<(SearchIndex, [u8; 32])> {
    let mut r = Reader::new(b);
    if r.take(4)? != MAGIC {
        return Err(Error::BadMagic);
    }
    let v = r.u16()?;
    if v != VERSION {
        return Err(Error::Unsupported(v));
    }
    let dsha: [u8; 32] = r.take(32)?.try_into().unwrap();
    let (no, np, ns) = (r.u32()? as usize, r.u32()? as usize, r.u32()? as usize);
    if ns != SECTIONS {
        return Err(Error::Corrupt("section count"));
    }
    let mut secs: Vec<&[u8]> = Vec::new();
    for _ in 0..ns {
        let (off, len) = (r.u64()? as usize, r.u64()? as usize);
        secs.push(b.get(off..off.checked_add(len).ok_or(Error::Eof)?).ok_or(Error::Eof)?);
    }
    let mut m = Reader::new(secs[0]);
    let rarities = (0..m.uvarint()?).map(|_| m.string()).collect::<Result<Vec<_>>>()?;
    let names = r_arena(secs[1])?;
    let names_n = r_arena(secs[2])?;
    let names_s = r_arena(secs[3])?;
    let types = r_arena(secs[4])?;
    let texts = r_arena(secs[5])?;
    let full_texts = r_arena(secs[6])?;
    for a in [&names, &names_n, &names_s, &types, &texts, &full_texts] {
        if a.len() != no {
            return Err(Error::Corrupt("oracle row count"));
        }
    }
    let mut c = Reader::new(secs[7]);
    let o_live: Vec<bool> = (0..no).map(|_| c.u8().map(|x| x != 0)).collect::<Result<_>>()?;
    let o_colors: Vec<u8> = (0..no).map(|_| c.u8()).collect::<Result<_>>()?;
    let o_identity: Vec<u8> = (0..no).map(|_| c.u8()).collect::<Result<_>>()?;
    let o_mv: Vec<Option<f64>> = (0..no)
        .map(|_| c.u64().map(|x| Some(f64::from_bits(x)).filter(|f| !f.is_nan())))
        .collect::<Result<_>>()?;
    let mut p = Reader::new(secs[8]);
    let p_oracle: Vec<u32> = (0..np).map(|_| p.u32()).collect::<Result<_>>()?;
    let p_live: Vec<bool> = (0..np).map(|_| p.u8().map(|x| x != 0)).collect::<Result<_>>()?;
    let hidden: Vec<bool> = (0..np).map(|_| p.u8().map(|x| x != 0)).collect::<Result<_>>()?;
    let p_rarity: Vec<Option<u32>> = (0..np).map(|_| p.u32().map(|x| Some(x).filter(|&x| x != u32::MAX))).collect::<Result<_>>()?;
    let p_released: Vec<u32> = (0..np).map(|_| p.u32()).collect::<Result<_>>()?;
    let strs = |a: Arena| -> Result<Vec<String>> {
        if a.len() != np {
            return Err(Error::Corrupt("printing row count"));
        }
        Ok((0..np).map(|i| a.get(i).to_string()).collect())
    };
    let p_set = strs(r_arena(secs[9])?)?;
    let p_cn = strs(r_arena(secs[10])?)?;
    if p_oracle.iter().any(|&o| o as usize >= no) {
        return Err(Error::Corrupt("printing oracle row"));
    }
    let mut live_p = Bits::new(np);
    let mut hidden_p = Bits::new(np);
    for i in 0..np {
        if p_live[i] {
            live_p.set(i);
        }
        if hidden[i] {
            hidden_p.set(i);
        }
    }
    let ix = SearchIndex {
        o_live, names, names_n, names_s, types, texts, full_texts,
        name_tri: r_tri(secs[11])?,
        text_tri: r_tri(secs[12])?,
        o_colors, o_identity, o_mv, p_oracle, p_live, p_set, p_cn, p_rarity, p_released, rarities, live_p, hidden_p,
    };
    Ok((ix, dsha))
}

// --------------------------------------------------------------------- delta ---

const ROW_TABLES: [&str; 10] = ["names", "names_n", "names_s", "types", "texts", "full_texts", "oracle_row", "printing_row", "set", "cn"];

fn arena_rows(a: &Arena) -> Vec<Vec<u8>> {
    (0..a.len()).map(|i| a.get(i).as_bytes().to_vec()).collect()
}

fn row_tables(ix: &SearchIndex) -> Vec<Vec<Vec<u8>>> {
    let oracle = (0..ix.o_live.len())
        .map(|i| {
            let mut v = vec![ix.o_live[i] as u8, ix.o_colors[i], ix.o_identity[i]];
            v.extend_from_slice(&ix.o_mv[i].unwrap_or(f64::NAN).to_bits().to_le_bytes());
            v
        })
        .collect();
    let printing = (0..ix.p_oracle.len())
        .map(|i| {
            let mut v = ix.p_oracle[i].to_le_bytes().to_vec();
            v.push(ix.p_live[i] as u8);
            v.push(ix.hidden_p.get(i) as u8);
            v.extend_from_slice(&ix.p_rarity[i].unwrap_or(u32::MAX).to_le_bytes());
            v.extend_from_slice(&ix.p_released[i].to_le_bytes());
            v
        })
        .collect();
    let strs = |v: &[String]| v.iter().map(|s| s.as_bytes().to_vec()).collect();
    vec![
        arena_rows(&ix.names), arena_rows(&ix.names_n), arena_rows(&ix.names_s), arena_rows(&ix.types),
        arena_rows(&ix.texts), arena_rows(&ix.full_texts), oracle, printing, strs(&ix.p_set), strs(&ix.p_cn),
    ]
}

fn from_rows(rarities: Vec<String>, t: &[Vec<Vec<u8>>], name_tri: Trigrams, text_tri: Trigrams) -> Result<SearchIndex> {
    let bad = Error::Corrupt("delta row");
    let fixed = |rows: &Vec<Vec<u8>>, n: usize| -> Result<()> {
        if rows.iter().any(|r| r.len() != n) { Err(Error::Corrupt("delta row size")) } else { Ok(()) }
    };
    fixed(&t[6], 11)?;
    fixed(&t[7], 14)?;
    let u32at = |r: &[u8], o: usize| u32::from_le_bytes(r[o..o + 4].try_into().unwrap());
    let np = t[7].len();
    let mut live_p = Bits::new(np);
    let mut hidden_p = Bits::new(np);
    for (i, r) in t[7].iter().enumerate() {
        if r[4] != 0 { live_p.set(i); }
        if r[5] != 0 { hidden_p.set(i); }
    }
    let strs = |rows: &Vec<Vec<u8>>| -> Result<Vec<String>> {
        rows.iter().map(|r| String::from_utf8(r.clone()).map_err(|_| Error::BadUtf8)).collect()
    };
    let ix = SearchIndex {
        o_live: t[6].iter().map(|r| r[0] != 0).collect(),
        names: arena_from_rows(&t[0])?, names_n: arena_from_rows(&t[1])?, names_s: arena_from_rows(&t[2])?,
        types: arena_from_rows(&t[3])?, texts: arena_from_rows(&t[4])?, full_texts: arena_from_rows(&t[5])?,
        name_tri, text_tri,
        o_colors: t[6].iter().map(|r| r[1]).collect(),
        o_identity: t[6].iter().map(|r| r[2]).collect(),
        o_mv: t[6].iter().map(|r| Some(f64::from_bits(u64::from_le_bytes(r[3..11].try_into().unwrap()))).filter(|f| !f.is_nan())).collect(),
        p_oracle: t[7].iter().map(|r| u32at(r, 0)).collect(),
        p_live: t[7].iter().map(|r| r[4] != 0).collect(),
        p_set: strs(&t[8])?, p_cn: strs(&t[9])?,
        p_rarity: t[7].iter().map(|r| Some(u32at(r, 6)).filter(|&x| x != u32::MAX)).collect(),
        p_released: t[7].iter().map(|r| u32at(r, 10)).collect(),
        rarities, live_p, hidden_p,
    };
    let n = ix.o_live.len();
    if t[..6].iter().any(|a| a.len() != n) || t[8].len() != np || t[9].len() != np || ix.p_oracle.iter().any(|&o| o as usize >= n) {
        return Err(bad);
    }
    Ok(ix)
}

fn diff_rows(old: &[Vec<u8>], new: &[Vec<u8>]) -> (Vec<u8>, usize) {
    let mut w = Writer::new();
    w.uvarint(new.len() as u64);
    let ch: Vec<usize> = (0..new.len()).filter(|&i| old.get(i) != Some(&new[i])).collect();
    w.uvarint(ch.len() as u64);
    let mut prev = 0;
    for &i in &ch {
        w.uvarint((i - prev) as u64);
        prev = i;
        w.uvarint(new[i].len() as u64);
        w.bytes(&new[i]);
    }
    (w.buf, ch.len())
}

fn apply_rows(old: &[Vec<u8>], r: &mut Reader) -> Result<Vec<Vec<u8>>> {
    let n = r.uvarint()? as usize;
    if n < old.len() {
        return Err(Error::Corrupt("delta shrinks rows"));
    }
    let mut rows: Vec<Vec<u8>> = old.to_vec();
    rows.resize(n, Vec::new());
    let mut prev = 0usize;
    for _ in 0..r.uvarint()? {
        prev += r.uvarint()? as usize;
        let len = r.uvarint()? as usize;
        *rows.get_mut(prev).ok_or(Error::Corrupt("delta row index"))? = r.take(len)?.to_vec();
    }
    Ok(rows)
}

fn tri_map(t: &Trigrams) -> BTreeMap<u32, Vec<u8>> {
    t.keys.iter().enumerate().map(|(k, &g)| (g, t.post[t.start[k] as usize..t.start[k + 1] as usize].to_vec())).collect()
}

fn tri_from(m: BTreeMap<u32, Vec<u8>>, stop: Vec<u32>) -> Trigrams {
    let mut t = Trigrams { stop, ..Default::default() };
    for (g, p) in m {
        t.keys.push(g);
        t.start.push(t.post.len() as u32);
        t.post.extend_from_slice(&p);
    }
    t.start.push(t.post.len() as u32);
    t
}

fn put_set(w: &mut Writer, v: &[u32]) {
    w.uvarint(v.len() as u64);
    let mut prev = 0;
    for &x in v {
        w.uvarint((x - prev) as u64);
        prev = x;
    }
}

fn get_set(r: &mut Reader) -> Result<Vec<u32>> {
    let (mut prev, mut v) = (0u32, Vec::new());
    for _ in 0..r.uvarint()? {
        prev += r.uvarint32()?;
        v.push(prev);
    }
    Ok(v)
}

/// Returns (bytes, stop-gram changes, [appended, replaced, removed, new grams]).
fn diff_tri(old: &Trigrams, new: &Trigrams) -> (Vec<u8>, usize, [usize; 4]) {
    let mut w = Writer::new();
    let add: Vec<u32> = new.stop.iter().copied().filter(|g| old.stop.binary_search(g).is_err()).collect();
    let rem: Vec<u32> = old.stop.iter().copied().filter(|g| new.stop.binary_search(g).is_err()).collect();
    put_set(&mut w, &add);
    put_set(&mut w, &rem);
    let (om, nm) = (tri_map(old), tri_map(new));
    let mut ch: Vec<(u32, u8, &[u8])> = Vec::new();
    let mut cnt = [0usize; 4];
    for (g, np) in &nm {
        match om.get(g) {
            Some(op) if op == np => {}
            Some(op) if np.len() > op.len() && np.starts_with(op) => { ch.push((*g, 1, &np[op.len()..])); cnt[0] += 1; }
            Some(_) => { ch.push((*g, 2, np)); cnt[1] += 1; }
            None => { ch.push((*g, 2, np)); cnt[3] += 1; }
        }
    }
    for g in om.keys().filter(|g| !nm.contains_key(g)) {
        ch.push((*g, 0, &[]));
        cnt[2] += 1;
    }
    ch.sort_by_key(|c| c.0);
    w.uvarint(ch.len() as u64);
    let mut prev = 0;
    for (g, kind, p) in &ch {
        w.uvarint((g - prev) as u64);
        prev = *g;
        w.u8(*kind);
        w.uvarint(p.len() as u64);
        w.bytes(p);
    }
    (w.buf, add.len() + rem.len(), cnt)
}

fn apply_tri(old: &Trigrams, r: &mut Reader) -> Result<Trigrams> {
    let add = get_set(r)?;
    let rem = get_set(r)?;
    let mut stop: Vec<u32> = old.stop.iter().copied().filter(|g| rem.binary_search(g).is_err()).collect();
    stop.extend(add);
    stop.sort_unstable();
    let mut m = tri_map(old);
    let mut g = 0u32;
    for _ in 0..r.uvarint()? {
        g += r.uvarint32()?;
        let kind = r.u8()?;
        let len = r.uvarint()? as usize;
        let p = r.take(len)?;
        match kind {
            0 => { m.remove(&g); }
            1 => m.get_mut(&g).ok_or(Error::Corrupt("delta append"))?.extend_from_slice(p),
            2 => { m.insert(g, p.to_vec()); }
            _ => return Err(Error::Corrupt("delta kind")),
        }
    }
    Ok(tri_from(m, stop))
}

pub struct TableDelta {
    pub name: &'static str,
    pub changed: usize,
    pub raw: usize,
    pub zstd: usize,
}

pub struct DeltaInfo {
    pub tables: Vec<TableDelta>,
    pub tri_appended: usize,
    pub tri_new: usize,
    pub tri_replaced: usize,
    pub tri_removed: usize,
    pub raw: usize,
    pub zstd: usize,
}

/// Delta from `old_file` to `new_file` (both full COQS files). Returns the delta bytes and a breakdown.
pub fn make_delta(old_file: &[u8], new_file: &[u8]) -> Result<(Vec<u8>, DeltaInfo)> {
    let (old, _) = deserialize(old_file)?;
    let (new, new_dsha) = deserialize(new_file)?;
    let (ot, nt) = (row_tables(&old), row_tables(&new));
    let mut body = Writer::new();
    body.bytes(&new_dsha);
    body.uvarint(new.rarities.len() as u64);
    new.rarities.iter().for_each(|s| body.str(s));
    let mut info = DeltaInfo { tables: Vec::new(), tri_appended: 0, tri_new: 0, tri_replaced: 0, tri_removed: 0, raw: 0, zstd: 0 };
    for (i, name) in ROW_TABLES.iter().enumerate() {
        let (b, changed) = diff_rows(&ot[i], &nt[i]);
        info.tables.push(TableDelta { name, changed, raw: b.len(), zstd: zstd_len(&b) });
        body.bytes(&b);
    }
    for (name, o, n) in [("name_trigrams", &old.name_tri, &new.name_tri), ("text_trigrams", &old.text_tri, &new.text_tri)] {
        let (b, stop_changes, c) = diff_tri(o, n);
        info.tri_appended += c[0];
        info.tri_replaced += c[1];
        info.tri_removed += c[2];
        info.tri_new += c[3];
        info.tables.push(TableDelta { name, changed: stop_changes + c[0] + c[1] + c[2] + c[3], raw: b.len(), zstd: zstd_len(&b) });
        body.bytes(&b);
    }
    let mut out = Vec::from(&DMAGIC[..]);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&sha(old_file));
    out.extend_from_slice(&sha(new_file));
    let comp = compress(&body.buf);
    out.extend_from_slice(&comp);
    info.raw = body.buf.len();
    info.zstd = out.len();
    Ok((out, info))
}

/// Replay a delta on `old_file`; the result must hash to the delta's target hash.
pub fn apply_delta(old_file: &[u8], delta: &[u8]) -> Result<Vec<u8>> {
    if delta.get(..4) != Some(&DMAGIC[..]) {
        return Err(Error::BadMagic);
    }
    let v = u16::from_le_bytes(delta.get(4..6).ok_or(Error::Eof)?.try_into().unwrap());
    if v != VERSION {
        return Err(Error::Unsupported(v));
    }
    let (base, target) = (delta.get(6..38).ok_or(Error::Eof)?, delta.get(38..70).ok_or(Error::Eof)?);
    if base != sha(old_file) {
        return Err(Error::Corrupt("delta base hash mismatch"));
    }
    let raw = zstd::stream::decode_all(&delta[70..]).map_err(|e| Error::Zstd(e.to_string()))?;
    let (old, _) = deserialize(old_file)?;
    let ot = row_tables(&old);
    let mut r = Reader::new(&raw);
    let dsha: [u8; 32] = r.take(32)?.try_into().unwrap();
    let rarities = (0..r.uvarint()?).map(|_| r.string()).collect::<Result<Vec<_>>>()?;
    let mut tables = Vec::new();
    for t in ot.iter().take(ROW_TABLES.len()) {
        tables.push(apply_rows(t, &mut r)?);
    }
    let name_tri = apply_tri(&old.name_tri, &mut r)?;
    let text_tri = apply_tri(&old.text_tri, &mut r)?;
    let ix = from_rows(rarities, &tables, name_tri, text_tri)?;
    let out = serialize(&ix, &dsha);
    if sha(&out) != target {
        return Err(Error::Corrupt("delta result hash mismatch"));
    }
    Ok(out)
}
