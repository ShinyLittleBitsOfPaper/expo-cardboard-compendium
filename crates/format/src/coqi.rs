//! COQI v2: the compile-time search index serialised for any `Store` (S3b of docs/s3-design.md), plus deltas between two index files.
//!
//! Layout (little endian): `COQI` | u16 version | dataset SHA-256 (of the `.coqd` the index was built from) | u32 section count |
//! (u64 off, u64 len) per section | sections, each 8-byte aligned and stored raw (fixed-width arrays) so a reader can map it.
//! Section 0 is the meta table (every column's table, name, kind and trigram table); then one section per column, then one per
//! trigram table. The row sections change only in their tail between two daily builds, which is what the delta exploits.
//!
//! Delta (`CQID`): base/target file SHA-256 + zstd body: per column section the changed or appended rows, per trigram table the
//! stop-gram additions/removals and the changed postings (`append` when the new list extends the old bytes, else `replace`).
//! A delta only applies between two files with the same layout (same meta section); otherwise ship the full index.
use crate::codec::{Error, Reader, Result, Writer};
use crate::gindex::{DKind, Dcol, IndexParts};
use crate::schema::Schema;
use crate::search::{Arena, Trigrams};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const MAGIC: &[u8; 4] = b"COQI";
const DMAGIC: &[u8; 4] = b"CQID";
pub const VERSION: u16 = 2;
const LEVEL: i32 = 19;

pub fn sha(b: &[u8]) -> [u8; 32] {
    Sha256::digest(b).into()
}

fn zstd_len(b: &[u8]) -> usize {
    zstd::bulk::compress(b, LEVEL).map(|v| v.len()).unwrap_or(b.len())
}

pub fn compress(bytes: &[u8]) -> Vec<u8> {
    zstd::bulk::compress(bytes, LEVEL).unwrap_or_else(|_| bytes.to_vec())
}

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


// ------------------------------------------------------------------ columns ---

const T_TEXT: u8 = 0;
const T_MASK: u8 = 1;
const T_NUM: u8 = 2;
const T_FLAG: u8 = 3;

fn tag_of(k: &DKind) -> u8 {
    match k {
        DKind::Text(_) => T_TEXT,
        DKind::Mask(_) => T_MASK,
        DKind::Num(_) => T_NUM,
        DKind::Flag(_) => T_FLAG,
    }
}

fn w_col(k: &DKind) -> Vec<u8> {
    match k {
        DKind::Text(a) => w_arena(a),
        DKind::Mask(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        DKind::Num(v) => v.iter().flat_map(|x| x.unwrap_or(f64::NAN).to_bits().to_le_bytes()).collect(),
        DKind::Flag(v) => v.iter().map(|&b| b as u8).collect(),
    }
}

fn r_col(tag: u8, b: &[u8]) -> Result<DKind> {
    let fixed = |w: usize| -> Result<()> {
        if b.len() % w != 0 {
            Err(Error::Corrupt("column width"))
        } else {
            Ok(())
        }
    };
    Ok(match tag {
        T_TEXT => DKind::Text(r_arena(b)?),
        T_MASK => {
            fixed(4)?;
            DKind::Mask(b.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect())
        }
        T_NUM => {
            fixed(8)?;
            DKind::Num(b.chunks_exact(8).map(|c| Some(f64::from_bits(u64::from_le_bytes(c.try_into().unwrap()))).filter(|f| !f.is_nan())).collect())
        }
        T_FLAG => DKind::Flag(b.iter().map(|&x| x != 0).collect()),
        _ => return Err(Error::Corrupt("column kind")),
    })
}

/// A column section as one byte string per row (the unit of the delta).
fn col_rows(tag: u8, b: &[u8]) -> Result<Vec<Vec<u8>>> {
    Ok(match r_col(tag, b)? {
        DKind::Text(a) => (0..a.len()).map(|i| a.get(i).as_bytes().to_vec()).collect(),
        DKind::Mask(_) => b.chunks_exact(4).map(|c| c.to_vec()).collect(),
        DKind::Num(_) => b.chunks_exact(8).map(|c| c.to_vec()).collect(),
        DKind::Flag(_) => b.iter().map(|&x| vec![x]).collect(),
    })
}

fn rows_col(tag: u8, rows: &[Vec<u8>]) -> Result<Vec<u8>> {
    let width = match tag {
        T_TEXT => return Ok(w_arena(&arena_from_rows(rows)?)),
        T_MASK => 4,
        T_NUM => 8,
        T_FLAG => 1,
        _ => return Err(Error::Corrupt("column kind")),
    };
    if rows.iter().any(|r| r.len() != width) {
        return Err(Error::Corrupt("delta row size"));
    }
    Ok(rows.concat())
}

// ------------------------------------------------------------------- file ---

fn meta_section(p: &IndexParts, schema: &Schema) -> Vec<u8> {
    let mut w = Writer::new();
    w.uvarint(p.dcols.len() as u64);
    for d in &p.dcols {
        w.str(&schema.tables[d.table].name);
        w.str(&d.name);
        w.u8(tag_of(&d.kind));
        w.uvarint(d.tri.map_or(0, |t| t as u64 + 1));
    }
    w.uvarint(p.tris.len() as u64);
    w.buf
}

struct Meta {
    cols: Vec<(String, String, u8, Option<usize>)>,
    ntri: usize,
}

fn parse_meta(b: &[u8]) -> Result<Meta> {
    let mut r = Reader::new(b);
    let n = r.uvarint()? as usize;
    let mut cols = Vec::new();
    for _ in 0..n {
        let (t, c, tag) = (r.string()?, r.string()?, r.u8()?);
        let tri = r.uvarint()? as usize;
        cols.push((t, c, tag, tri.checked_sub(1)));
    }
    let ntri = r.uvarint()? as usize;
    if cols.iter().any(|c| c.3.is_some_and(|t| t >= ntri)) {
        return Err(Error::Corrupt("trigram table id"));
    }
    Ok(Meta { cols, ntri })
}

fn sections(p: &IndexParts, schema: &Schema) -> Vec<(String, Vec<u8>)> {
    let mut v = vec![("meta".to_string(), meta_section(p, schema))];
    for d in &p.dcols {
        v.push((format!("{}.{}", schema.tables[d.table].name, d.name), w_col(&d.kind)));
    }
    for (i, t) in p.tris.iter().enumerate() {
        v.push((format!("trigrams.{i}"), w_tri(t)));
    }
    v
}

fn assemble(dataset_sha: &[u8; 32], secs: &[Vec<u8>]) -> Vec<u8> {
    let mut head = Writer::new();
    head.bytes(MAGIC);
    head.u16(VERSION);
    head.bytes(dataset_sha);
    head.u32(secs.len() as u32);
    let table_at = head.buf.len();
    let mut off = (table_at + secs.len() * 16).next_multiple_of(8);
    let mut table = Writer::new();
    for s in secs {
        table.u64(off as u64);
        table.u64(s.len() as u64);
        off = (off + s.len()).next_multiple_of(8);
    }
    let mut out = head.buf;
    out.extend_from_slice(&table.buf);
    for s in secs {
        out.resize(out.len().next_multiple_of(8), 0);
        out.extend_from_slice(s);
    }
    out.resize(out.len().next_multiple_of(8), 0);
    out
}

/// Serialize index parts. `dataset_sha` is the SHA-256 of the `.coqd` file they were built from.
pub fn serialize(p: &IndexParts, schema: &Schema, dataset_sha: &[u8; 32]) -> Vec<u8> {
    let secs: Vec<Vec<u8>> = sections(p, schema).into_iter().map(|s| s.1).collect();
    assemble(dataset_sha, &secs)
}

/// One section's raw and zstd size (for the size report).
pub struct SectionSize {
    pub name: String,
    pub raw: usize,
    pub zstd: usize,
}

pub fn section_sizes(p: &IndexParts, schema: &Schema) -> Vec<SectionSize> {
    sections(p, schema).into_iter().map(|(name, s)| SectionSize { name, raw: s.len(), zstd: zstd_len(&s) }).collect()
}

fn split(b: &[u8]) -> Result<([u8; 32], Vec<&[u8]>)> {
    let mut r = Reader::new(b);
    if r.take(4)? != MAGIC {
        return Err(Error::BadMagic);
    }
    let v = r.u16()?;
    if v != VERSION {
        return Err(Error::Unsupported(v));
    }
    let dsha: [u8; 32] = r.take(32)?.try_into().unwrap();
    let ns = r.u32()? as usize;
    let mut secs = Vec::new();
    for _ in 0..ns {
        let (off, len) = (r.u64()? as usize, r.u64()? as usize);
        secs.push(b.get(off..off.checked_add(len).ok_or(Error::Eof)?).ok_or(Error::Eof)?);
    }
    if secs.is_empty() {
        return Err(Error::Corrupt("no sections"));
    }
    Ok((dsha, secs))
}

/// Dataset hash recorded in an index file (cheap: reads the header only).
pub fn dataset_sha(b: &[u8]) -> Result<[u8; 32]> {
    let mut r = Reader::new(b);
    if r.take(4)? != MAGIC {
        return Err(Error::BadMagic);
    }
    let v = r.u16()?;
    if v != VERSION {
        return Err(Error::Unsupported(v));
    }
    Ok(r.take(32)?.try_into().unwrap())
}

/// Parse a COQI file back into index parts; returns the dataset hash it was built from. The schema maps table names to ids.
pub fn deserialize(b: &[u8], schema: &Schema) -> Result<(IndexParts, [u8; 32])> {
    let (dsha, secs) = split(b)?;
    let meta = parse_meta(secs[0])?;
    if secs.len() != 1 + meta.cols.len() + meta.ntri {
        return Err(Error::Corrupt("section count"));
    }
    let mut dcols = Vec::new();
    for (i, (t, c, tag, tri)) in meta.cols.iter().enumerate() {
        let table = schema.table(t).ok_or(Error::Corrupt("index table"))?;
        dcols.push(Dcol { table, name: c.clone(), kind: r_col(*tag, secs[1 + i])?, tri: *tri });
    }
    let tris = secs[1 + meta.cols.len()..].iter().map(|s| r_tri(s)).collect::<Result<Vec<_>>>()?;
    Ok((IndexParts { dcols, tris }, dsha))
}

// --------------------------------------------------------------------- delta ---
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
    pub name: String,
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

fn names(meta: &Meta) -> Vec<String> {
    meta.cols.iter().map(|c| format!("{}.{}", c.0, c.1)).collect()
}

/// Delta from `old_file` to `new_file` (both full COQI files with the same layout). Returns the delta bytes and a breakdown.
pub fn make_delta(old_file: &[u8], new_file: &[u8]) -> Result<(Vec<u8>, DeltaInfo)> {
    let ((_, os), (new_dsha, ns)) = (split(old_file)?, split(new_file)?);
    if os[0] != ns[0] {
        return Err(Error::Corrupt("index layout changed; ship the full index"));
    }
    let meta = parse_meta(ns[0])?;
    let nc = meta.cols.len();
    let mut body = Writer::new();
    body.bytes(&new_dsha);
    let mut info = DeltaInfo { tables: Vec::new(), tri_appended: 0, tri_new: 0, tri_replaced: 0, tri_removed: 0, raw: 0, zstd: 0 };
    for (i, name) in names(&meta).into_iter().enumerate() {
        let tag = meta.cols[i].2;
        let (b, changed) = diff_rows(&col_rows(tag, os[1 + i])?, &col_rows(tag, ns[1 + i])?);
        info.tables.push(TableDelta { name, changed, raw: b.len(), zstd: zstd_len(&b) });
        body.bytes(&b);
    }
    for t in 0..meta.ntri {
        let (b, stop_changes, c) = diff_tri(&r_tri(os[1 + nc + t])?, &r_tri(ns[1 + nc + t])?);
        info.tri_appended += c[0];
        info.tri_replaced += c[1];
        info.tri_removed += c[2];
        info.tri_new += c[3];
        info.tables.push(TableDelta { name: format!("trigrams.{t}"), changed: stop_changes + c[0] + c[1] + c[2] + c[3], raw: b.len(), zstd: zstd_len(&b) });
        body.bytes(&b);
    }
    let mut out = Vec::from(&DMAGIC[..]);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&sha(old_file));
    out.extend_from_slice(&sha(new_file));
    out.extend_from_slice(&compress(&body.buf));
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
    let (_, os) = split(old_file)?;
    let meta = parse_meta(os[0])?;
    let nc = meta.cols.len();
    if os.len() != 1 + nc + meta.ntri {
        return Err(Error::Corrupt("section count"));
    }
    let mut r = Reader::new(&raw);
    let dsha: [u8; 32] = r.take(32)?.try_into().unwrap();
    let mut secs: Vec<Vec<u8>> = vec![os[0].to_vec()];
    for i in 0..nc {
        let tag = meta.cols[i].2;
        let rows = apply_rows(&col_rows(tag, os[1 + i])?, &mut r)?;
        secs.push(rows_col(tag, &rows)?);
    }
    for t in 0..meta.ntri {
        let tri = apply_tri(&r_tri(os[1 + nc + t])?, &mut r)?;
        secs.push(w_tri(&tri));
    }
    let out = assemble(&dsha, &secs);
    if sha(&out) != target {
        return Err(Error::Corrupt("delta result hash mismatch"));
    }
    Ok(out)
}
