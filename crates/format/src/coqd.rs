//! COQD v3: the schema-driven container (S2b of docs/schema-design.md). All integers little-endian.
//!
//! ```text
//! magic "COQD" | u16 format_version=3 | u16 min_reader=3 | u32 section_count
//! section_count x { u32 kind, u32 arg, u64 offset, u64 len }        (offsets absolute)
//! kind 1 SCHEMA   Schema::encode
//! kind 2 META     u32 epoch | u32 source_date | dicts (schema order) | props | per-table u32 row count
//! kind 3 ROWS     arg = table index; row blocks (below)
//! kind 4 KEYS     arg = table index; u32 count, then count x { [u8;16] key, u32 row } sorted by key (tombstones included)
//! ```
//!
//! Row blocks: `u32 rows_per_block | u32 block_count`, then `block_count x { u64 off, u32 clen, u32 rawlen, [u8;8] hash }`, then the
//! zstd-compressed blocks (`off` relative to the data area). A raw block is a run of `varint(len) + row bytes`. Row N lives in block
//! N / rows_per_block, so appending rows only touches tail blocks and an unchanged block keeps its hash.
//!
//! Columnar tables (`TableDef::columnar`, Int columns only) replace the row blocks by one section: `u32 rows | u32 raw_len | zstd(raw)`,
//! raw = one tombstone byte per row, then every column as `rows` varints of `0 | zigzag+1` (same cell code as a row). Dense numeric
//! tables compress far better this way and a daily change touches few bytes of the column stream.
//!
//! Row bytes (schema driven): `varint flags` (bit 0 tombstone, then 2 bits per Bool column in column order: 0 absent, 1 false,
//! 2 true), then every non-Bool column in order. A tombstoned row carries only its key column (if the table has one).
//! Str/Json `opt_str`; Int `varint(0 | zigzag+1)`; Float `u8 tag [+ u64 bits]`; Uuid 16 bytes; Enum `opt_id`; Set `varint(0 | len+1)`
//! then ids; Ref `varint`; Bytes `varint(0 | len+1)` then the bytes.

use crate::codec::*;
use crate::schema::*;
use crate::table::*;
use crate::dict::Dict;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU32, Ordering};

pub const MAGIC: &[u8; 4] = b"COQD";
pub const FORMAT_VERSION: u16 = 3;
pub const MIN_READER: u16 = 3;
pub const BLOCK_ROWS: usize = 256;
/// Pinned so identical input yields identical bytes (also pinned via Cargo.lock).
pub const ZSTD_LEVEL: i32 = 15;

const K_SCHEMA: u32 = 1;
const K_META: u32 = 2;
const K_ROWS: u32 = 3;
const K_KEYS: u32 = 4;

#[derive(Clone, Debug)]
pub struct SectionStat {
    pub name: String,
    pub raw_bytes: u64,
    pub stored_bytes: u64,
    pub blocks: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockInfo {
    pub off: u64,
    pub clen: u32,
    pub rawlen: u32,
    pub hash: [u8; 8],
}

fn zstd_compress(raw: &[u8]) -> Result<Vec<u8>> {
    zstd::bulk::compress(raw, ZSTD_LEVEL).map_err(|e| Error::Zstd(e.to_string()))
}

// ------------------------------------------------------------------ rows ---

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}
fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}
fn tri(v: Option<bool>) -> u64 {
    match v {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    }
}
fn untri(v: u64) -> Option<bool> {
    match v {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    }
}

fn bool_cols(def: &TableDef) -> Vec<usize> {
    def.columns.iter().enumerate().filter(|(_, c)| c.ty == ColType::Bool).map(|(i, _)| i).collect()
}

fn key_col(def: &TableDef) -> Option<usize> {
    def.key.as_ref().and_then(|k| def.columns.iter().position(|c| &c.name == k))
}

/// Encode row `i` of `t` (schema `def`).
pub fn encode_row(def: &TableDef, t: &Table, i: usize, w: &mut Writer) {
    let mut flags = t.tomb[i] as u64;
    for (k, &ci) in bool_cols(def).iter().enumerate() {
        if let ColData::Bool(v) = &t.cols[ci] {
            flags |= tri(v[i]) << (1 + 2 * k);
        }
    }
    w.uvarint(flags);
    if t.tomb[i] {
        if let Some(ci) = key_col(def) {
            if let ColData::Uuid(v) = &t.cols[ci] {
                w.bytes(&v[i]);
            }
        }
        return;
    }
    for col in &t.cols {
        match col {
            ColData::Bool(_) => {}
            ColData::Str(v) | ColData::Json(v) => w.opt_str(v[i].as_deref()),
            ColData::Int(v) => w.uvarint(v[i].map_or(0, |x| zigzag(x).wrapping_add(1))),
            ColData::Float(v) => match v[i] {
                None => w.u8(0),
                Some(x) => {
                    w.u8(1);
                    w.u64(x.to_bits());
                }
            },
            ColData::Uuid(v) => w.bytes(&v[i]),
            ColData::Enum(v) => w.opt_id(v[i]),
            ColData::Set(v) => match &v[i] {
                None => w.uvarint(0),
                Some(s) => {
                    w.uvarint(s.len() as u64 + 1);
                    for id in s {
                        w.uvarint(*id as u64);
                    }
                }
            },
            ColData::Ref(v) => w.uvarint(v[i] as u64),
            ColData::Bytes(v) => match &v[i] {
                None => w.uvarint(0),
                Some(b) => {
                    w.uvarint(b.len() as u64 + 1);
                    w.bytes(b);
                }
            },
        }
    }
}

/// Decode one row from `r` and append it to `t`.
pub fn decode_row(def: &TableDef, t: &mut Table, r: &mut Reader) -> Result<()> {
    let flags = r.uvarint()?;
    let tomb = flags & 1 != 0;
    t.tomb.push(tomb);
    let bools = bool_cols(def);
    let kc = key_col(def);
    for (ci, col) in t.cols.iter_mut().enumerate() {
        if tomb {
            match (col, kc == Some(ci)) {
                (ColData::Uuid(v), true) => v.push(r.uuid()?),
                (c, _) => c.push_default(),
            }
            continue;
        }
        match col {
            ColData::Bool(v) => {
                let k = bools.iter().position(|&b| b == ci).unwrap();
                v.push(untri(flags >> (1 + 2 * k) & 3));
            }
            ColData::Str(v) | ColData::Json(v) => v.push(r.opt_string()?),
            ColData::Int(v) => v.push(match r.uvarint()? {
                0 => None,
                x => Some(unzigzag(x.wrapping_sub(1))),
            }),
            ColData::Float(v) => v.push(match r.u8()? {
                0 => None,
                _ => Some(f64::from_bits(r.u64()?)),
            }),
            ColData::Uuid(v) => v.push(r.uuid()?),
            ColData::Enum(v) => v.push(r.opt_id()?),
            ColData::Set(v) => {
                let n = r.uvarint()? as usize;
                v.push(if n == 0 {
                    None
                } else {
                    let mut s = Vec::with_capacity(r.cap(n - 1));
                    for _ in 0..n - 1 {
                        s.push(r.uvarint32()?);
                    }
                    Some(s)
                });
            }
            ColData::Ref(v) => v.push(r.uvarint32()?),
            ColData::Bytes(v) => {
                let n = r.uvarint()? as usize;
                v.push(if n == 0 { None } else { Some(r.take(n - 1)?.to_vec()) });
            }
        }
    }
    Ok(())
}

pub fn row_bytes(def: &TableDef, t: &Table, i: usize) -> Vec<u8> {
    let mut w = Writer::new();
    encode_row(def, t, i, &mut w);
    w.buf
}

// ------------------------------------------------------------------ meta ---

/// Dataset-wide numbers, dictionaries, props and per-table row counts.
pub struct MetaBlock {
    pub epoch: u32,
    pub source_date: u32,
    pub dicts: Vec<Dict>,
    pub props: Vec<(String, Vec<String>)>,
    pub counts: Vec<u32>,
}

pub fn encode_meta(st: &Store, w: &mut Writer) {
    w.u32(st.epoch);
    w.u32(st.source_date);
    for d in &st.dicts {
        d.encode(w);
    }
    w.uvarint(st.props.len() as u64);
    for (k, v) in &st.props {
        w.str(k);
        w.uvarint(v.len() as u64);
        for s in v {
            w.str(s);
        }
    }
    for t in &st.tables {
        w.u32(t.rows() as u32);
    }
}

pub fn decode_meta(schema: &Schema, r: &mut Reader) -> Result<MetaBlock> {
    let epoch = r.u32()?;
    let source_date = r.u32()?;
    let mut dicts = Vec::with_capacity(schema.dicts.len());
    for _ in &schema.dicts {
        dicts.push(Dict::decode(r)?);
    }
    let n = r.uvarint()? as usize;
    let mut props = Vec::with_capacity(r.cap(n));
    for _ in 0..n {
        let k = r.string()?;
        let m = r.uvarint()? as usize;
        let mut v = Vec::with_capacity(r.cap(m));
        for _ in 0..m {
            v.push(r.string()?);
        }
        props.push((k, v));
    }
    let mut counts = Vec::with_capacity(schema.tables.len());
    for _ in &schema.tables {
        counts.push(r.u32()?);
    }
    Ok(MetaBlock { epoch, source_date, dicts, props, counts })
}

// ----------------------------------------------------------------- write ---

static REUSED_BLOCKS: AtomicU32 = AtomicU32::new(0);
static ENCODED_BLOCKS: AtomicU32 = AtomicU32::new(0);

/// Diagnostics (0.0.9-1): zero the row-block counters, then read (reused, recompressed) after a `write_store*` call. Process-wide, so
/// only meaningful when one write runs at a time (the sync's `patchFile` steps do).
pub fn block_counts_reset() {
    REUSED_BLOCKS.store(0, Ordering::Relaxed);
    ENCODED_BLOCKS.store(0, Ordering::Relaxed);
}
pub fn block_counts() -> (u32, u32) {
    (REUSED_BLOCKS.load(Ordering::Relaxed), ENCODED_BLOCKS.load(Ordering::Relaxed))
}

/// The block table + data of one table's rows section in an OLD file, so an unchanged block can be copied instead of recompressed.
struct OldRows<'a> {
    nblocks: usize,
    table: &'a [u8],
    data: &'a [u8],
}

impl<'a> OldRows<'a> {
    fn new(sec: &'a [u8]) -> Option<Self> {
        let rows = u32::from_le_bytes(sec.get(0..4)?.try_into().ok()?) as usize;
        let nblocks = u32::from_le_bytes(sec.get(4..8)?.try_into().ok()?) as usize;
        if rows != BLOCK_ROWS {
            return None;
        }
        let end = 8usize.checked_add(nblocks.checked_mul(24)?)?;
        Some(OldRows { nblocks, table: sec.get(8..end)?, data: sec.get(end..)? })
    }

    /// (compressed bytes, raw length, hash) of old block `b`.
    fn block(&self, b: usize) -> Option<(&'a [u8], usize, [u8; 8])> {
        if b >= self.nblocks {
            return None;
        }
        let e = self.table.get(b * 24..b * 24 + 24)?;
        let off = u64::from_le_bytes(e[0..8].try_into().ok()?) as usize;
        let clen = u32::from_le_bytes(e[8..12].try_into().ok()?) as usize;
        let rlen = u32::from_le_bytes(e[12..16].try_into().ok()?) as usize;
        let hash: [u8; 8] = e[16..24].try_into().ok()?;
        Some((self.data.get(off..off.checked_add(clen)?)?, rlen, hash))
    }
}

/// One block: rows `start..end` as length-prefixed row bytes, zstd-compressed. Independent of every other block, so blocks can be
/// built on any thread in any order and the output bytes are the same (0.0.8-2). With `old`, a block whose raw bytes equal the old
/// file's block `b` reuses the old compressed bytes (0.0.8-3): compression is deterministic, so the result is identical, and the
/// check is on the decompressed CONTENT, so a stale or mismatched old file can only cost time, never produce different bytes.
fn encode_one_block(def: &TableDef, t: &Table, b: usize, start: usize, end: usize, old: Option<&OldRows>) -> Result<(Vec<u8>, usize, [u8; 8])> {
    let mut raw = Writer::new();
    for i in start..end {
        let one = row_bytes(def, t, i);
        raw.uvarint(one.len() as u64);
        raw.bytes(&one);
    }
    if let Some((comp, rlen, hash)) = old.and_then(|o| o.block(b)) {
        if rlen == raw.buf.len() {
            if let Ok(prev) = zstd_exact(comp, rlen) {
                if prev == raw.buf {
                    REUSED_BLOCKS.fetch_add(1, Ordering::Relaxed);
                    return Ok((comp.to_vec(), rlen, hash));
                }
            }
        }
    }
    ENCODED_BLOCKS.fetch_add(1, Ordering::Relaxed);
    let comp = zstd_compress(&raw.buf)?;
    let hash: [u8; 8] = Sha256::digest(&comp)[..8].try_into().unwrap();
    Ok((comp, raw.buf.len(), hash))
}

/// Compress all blocks of a table, spread over up to 8 threads (0.0.8-2: a dataset patch re-encodes the whole dataset at zstd 15 to
/// reproduce the published bytes, 91 s single-threaded on a phone). Block boundaries and per-block compression are unchanged and the
/// results are joined in block order, so the output is byte-identical to the sequential version.
fn encode_blocks(def: &TableDef, t: &Table, old: Option<&OldRows>) -> Result<(Vec<u8>, u64, u32)> {
    let rows = t.rows();
    let nblocks_total = rows.div_ceil(BLOCK_ROWS);
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 8).min(nblocks_total.max(1));
    let mut blocks: Vec<Result<(Vec<u8>, usize, [u8; 8])>> = Vec::with_capacity(nblocks_total);
    if threads <= 1 {
        for b in 0..nblocks_total {
            blocks.push(encode_one_block(def, t, b, b * BLOCK_ROWS, ((b + 1) * BLOCK_ROWS).min(rows), old));
        }
    } else {
        // Interleaved assignment (thread k takes blocks k, k+threads, ...) keeps the load even when later blocks are bigger.
        let mut parts: Vec<Vec<Result<(Vec<u8>, usize, [u8; 8])>>> = Vec::new();
        std::thread::scope(|sc| {
            let handles: Vec<_> = (0..threads)
                .map(|k| {
                    sc.spawn(move || {
                        (k..nblocks_total)
                            .step_by(threads)
                            .map(|b| encode_one_block(def, t, b, b * BLOCK_ROWS, ((b + 1) * BLOCK_ROWS).min(rows), old))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            for h in handles {
                parts.push(h.join().unwrap_or_else(|_| vec![Err(Error::Corrupt("block encoder panicked"))]));
            }
        });
        let mut iters: Vec<_> = parts.into_iter().map(|p| p.into_iter()).collect();
        for b in 0..nblocks_total {
            match iters[b % threads].next() {
                Some(r) => blocks.push(r),
                None => return Err(Error::Corrupt("block encoder panicked")),
            }
        }
    }
    let mut table = Writer::new();
    let mut data = Writer::new();
    let (mut raw_total, mut nblocks) = (0u64, 0u32);
    for r in blocks {
        let (comp, rawlen, hash) = r?;
        table.u64(data.buf.len() as u64);
        table.u32(comp.len() as u32);
        table.u32(rawlen as u32);
        table.bytes(&hash);
        data.bytes(&comp);
        raw_total += rawlen as u64;
        nblocks += 1;
    }
    let mut out = Writer::new();
    out.u32(BLOCK_ROWS as u32);
    out.u32(nblocks);
    out.bytes(&table.buf);
    out.bytes(&data.buf);
    Ok((out.buf, raw_total, nblocks))
}

fn key_section(def: &TableDef, t: &Table) -> Option<Vec<u8>> {
    let ci = key_col(def)?;
    let ColData::Uuid(keys) = &t.cols[ci] else { return None };
    let mut v: Vec<([u8; 16], u32)> = keys.iter().enumerate().map(|(i, k)| (*k, i as u32)).collect();
    v.sort();
    let mut w = Writer::new();
    w.u32(v.len() as u32);
    for (k, row) in v {
        w.bytes(&k);
        w.u32(row);
    }
    Some(w.buf)
}

fn encode_columnar(t: &Table) -> Result<Vec<u8>> {
    let mut raw = Writer::new();
    t.tomb.iter().for_each(|d| raw.u8(*d as u8));
    for c in &t.cols {
        let ColData::Int(v) = c else { return Err(Error::Corrupt("columnar table with a non-Int column")) };
        v.iter().for_each(|x| raw.uvarint(x.map_or(0, |x| zigzag(x).wrapping_add(1))));
    }
    let z = zstd_compress(&raw.buf)?;
    let mut out = Writer::new();
    out.u32(t.rows() as u32);
    out.u32(raw.buf.len() as u32);
    out.bytes(&z);
    Ok(out.buf)
}

fn decode_columnar(def: &TableDef, sec: &[u8], count: usize) -> Result<Table> {
    let mut r = Reader::new(sec);
    let (n, rawlen) = (r.u32()? as usize, r.u32()? as usize);
    if n != count {
        return Err(Error::Corrupt("row count mismatch"));
    }
    let raw = crate::codec::zstd_exact(&sec[8..], rawlen).map_err(Error::Zstd)?;
    let mut br = Reader::new(&raw);
    let mut t = Table::new(def);
    for _ in 0..n {
        t.tomb.push(br.u8()? != 0);
    }
    for c in t.cols.iter_mut() {
        let ColData::Int(v) = c else { return Err(Error::Corrupt("columnar table with a non-Int column")) };
        for _ in 0..n {
            v.push(match br.uvarint()? {
                0 => None,
                x => Some(unzigzag(x.wrapping_sub(1))),
            });
        }
    }
    Ok(t)
}

pub fn write_store(st: &Store) -> Result<(Vec<u8>, Vec<SectionStat>)> {
    write_store_reusing(st, None)
}

/// `write_store`, copying the compressed bytes of every row block that is unchanged against `old` (an earlier file of the same
/// dataset). Output is byte-identical to `write_store(st)`; only the time differs (0.0.8-3).
pub fn write_store_reusing(st: &Store, old: Option<&[u8]>) -> Result<(Vec<u8>, Vec<SectionStat>)> {
    let old_parsed = old.and_then(|b| parse(b).ok());
    st.check().map_err(|_| Error::Corrupt("inconsistent store"))?;
    let mut schema = Writer::new();
    st.schema.encode(&mut schema);
    let mut meta = Writer::new();
    encode_meta(st, &mut meta);

    let mut stats = vec![];
    let mut sections: Vec<(u32, u32, Vec<u8>)> = vec![(K_SCHEMA, 0, schema.buf), (K_META, 0, meta.buf)];
    for (i, (def, t)) in st.schema.tables.iter().zip(&st.tables).enumerate() {
        let (bytes, raw, blocks) = if def.columnar {
            let b = encode_columnar(t)?;
            let n = b.len() as u64;
            (b, n, 1)
        } else {
            let o = old_parsed.as_ref().and_then(|p| p.rows(i as u32).ok()).and_then(OldRows::new);
            encode_blocks(def, t, o.as_ref())?
        };
        stats.push(SectionStat { name: def.name.clone(), raw_bytes: raw, stored_bytes: bytes.len() as u64, blocks });
        sections.push((K_ROWS, i as u32, bytes));
        if let Some(k) = key_section(def, t) {
            stats.push(SectionStat { name: format!("{}_keys", def.name), raw_bytes: k.len() as u64, stored_bytes: k.len() as u64, blocks: 0 });
            sections.push((K_KEYS, i as u32, k));
        }
    }
    for (k, _, b) in sections.iter().take(2) {
        stats.push(SectionStat { name: if *k == K_SCHEMA { "schema" } else { "meta" }.into(), raw_bytes: b.len() as u64, stored_bytes: b.len() as u64, blocks: 0 });
    }

    let header_len = 4 + 2 + 2 + 4 + sections.len() * 24;
    let mut out = Writer::new();
    out.bytes(MAGIC);
    out.u16(FORMAT_VERSION);
    out.u16(MIN_READER);
    out.u32(sections.len() as u32);
    let mut off = header_len as u64;
    for (kind, arg, bytes) in &sections {
        out.u32(*kind);
        out.u32(*arg);
        out.u64(off);
        out.u64(bytes.len() as u64);
        off += bytes.len() as u64;
    }
    for (_, _, bytes) in &sections {
        out.bytes(bytes);
    }
    Ok((out.buf, stats))
}

// ------------------------------------------------------------------ read ---

pub struct Parsed<'a> {
    pub sections: Vec<(u32, u32, &'a [u8])>,
}

impl<'a> Parsed<'a> {
    pub fn section(&self, kind: u32, arg: u32) -> Result<&'a [u8]> {
        self.sections.iter().find(|(k, a, _)| *k == kind && *a == arg).map(|(_, _, s)| *s).ok_or(Error::Corrupt("missing section"))
    }
    pub fn schema(&self) -> Result<Schema> {
        Schema::decode(&mut Reader::new(self.section(K_SCHEMA, 0)?))
    }
    pub fn meta(&self) -> Result<MetaBlock> {
        decode_meta(&self.schema()?, &mut Reader::new(self.section(K_META, 0)?))
    }
    pub fn rows(&self, table: u32) -> Result<&'a [u8]> {
        self.section(K_ROWS, table)
    }
    pub fn keys(&self, table: u32) -> Result<&'a [u8]> {
        self.section(K_KEYS, table)
    }
}

pub fn parse(bytes: &[u8]) -> Result<Parsed<'_>> {
    let mut r = Reader::new(bytes);
    if r.take(4)? != MAGIC {
        return Err(Error::BadMagic);
    }
    let version = r.u16()?;
    if version < MIN_READER {
        return Err(Error::Unsupported(version));
    }
    let min_reader = r.u16()?;
    if min_reader > FORMAT_VERSION {
        return Err(Error::Unsupported(min_reader));
    }
    let n = r.u32()? as usize;
    let mut sections = Vec::with_capacity(r.cap(n));
    for _ in 0..n {
        let (kind, arg) = (r.u32()?, r.u32()?);
        let off = r.u64()? as usize;
        let len = r.u64()? as usize;
        let end = off.checked_add(len).ok_or(Error::Eof)?;
        if end > bytes.len() {
            return Err(Error::Corrupt("section out of range"));
        }
        sections.push((kind, arg, &bytes[off..end]));
    }
    Ok(Parsed { sections })
}

/// Block table of one table's row section (used by `coqc diff` and range-reading engines).
pub fn block_table(bytes: &[u8], table: &str) -> Result<Vec<BlockInfo>> {
    let p = parse(bytes)?;
    let schema = p.schema()?;
    let ti = schema.table(table).ok_or(Error::Corrupt("unknown table"))?;
    if schema.tables[ti].columnar {
        return Err(Error::Corrupt("columnar table has no row blocks"));
    }
    let mut r = Reader::new(p.rows(ti as u32)?);
    let _per = r.u32()?;
    let n = r.u32()? as usize;
    let mut v = Vec::with_capacity(r.cap(n));
    for _ in 0..n {
        let off = r.u64()?;
        let clen = r.u32()?;
        let rawlen = r.u32()?;
        let hash: [u8; 8] = r.take(8)?.try_into().unwrap();
        v.push(BlockInfo { off, clen, rawlen, hash });
    }
    Ok(v)
}

fn decode_blocks(def: &TableDef, sec: &[u8], count: usize) -> Result<Table> {
    let mut r = Reader::new(sec);
    let _per = r.u32()?;
    let n = r.u32()? as usize;
    let mut blocks = Vec::with_capacity(r.cap(n));
    for _ in 0..n {
        let off = r.u64()? as usize;
        let clen = r.u32()? as usize;
        let rawlen = r.u32()? as usize;
        r.take(8)?;
        blocks.push((off, clen, rawlen));
    }
    // Checked arithmetic: offsets and lengths come from untrusted input (overflowed on fuzzed files; also wraps on 32-bit targets).
    let data_start = n.checked_mul(24).and_then(|x| x.checked_add(8)).ok_or(Error::Corrupt("block table too large"))?;
    let mut t = Table::new(def);
    for (off, clen, rawlen) in blocks {
        let comp = data_start
            .checked_add(off)
            .and_then(|start| start.checked_add(clen).map(|end| (start, end)))
            .and_then(|(start, end)| sec.get(start..end))
            .ok_or(Error::Corrupt("block out of range"))?;
        let raw = crate::codec::zstd_exact(comp, rawlen).map_err(Error::Zstd)?;
        let mut br = Reader::new(&raw);
        while !br.is_empty() {
            let len = br.uvarint()? as usize;
            decode_row(def, &mut t, &mut Reader::new(br.take(len)?))?;
        }
    }
    if t.rows() != count {
        return Err(Error::Corrupt("row count mismatch"));
    }
    Ok(t)
}

pub fn read_store(bytes: &[u8]) -> Result<Store> {
    let p = parse(bytes)?;
    let schema = p.schema()?;
    schema.validate().map_err(|_| Error::Corrupt("invalid schema"))?;
    let m = decode_meta(&schema, &mut Reader::new(p.section(K_META, 0)?))?;
    let mut tables = Vec::with_capacity(schema.tables.len());
    for (i, def) in schema.tables.iter().enumerate() {
        let sec = p.rows(i as u32)?;
        let count = m.counts[i] as usize;
        tables.push(if def.columnar { decode_columnar(def, sec, count)? } else { decode_blocks(def, sec, count)? });
    }
    let st = Store { schema, epoch: m.epoch, source_date: m.source_date, dicts: m.dicts, props: m.props, tables };
    st.check().map_err(|_| Error::Corrupt("inconsistent store"))?;
    Ok(st)
}
