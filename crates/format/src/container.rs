//! File layout (all integers little-endian):
//!
//! ```text
//! magic "COQD" | u16 format_version | u16 min_reader | u32 cards_epoch
//! u32 source_date | u32 oracle_count | u32 printing_count | u32 section_count
//! section_count x { u32 id, u64 offset, u64 len }      (offsets are absolute)
//! sections...
//! ```
//!
//! Block sections (oracle, printing): `u32 records_per_block | u32 block_count`,
//! then `block_count x { u64 off, u32 clen, u32 rawlen, [u8;8] hash }`, then the
//! zstd-compressed blocks (`off` relative to the start of the data area). A raw
//! block is a run of `varint(len) + record bytes`. Row N lives in block
//! N / records_per_block, so appending rows only touches tail blocks.
//!
//! UUID sections: `u32 count`, then count x { [u8;16] uuid, u32 row } sorted by
//! uuid (tombstoned rows included, so ids are never reused).

use crate::codec::*;
use crate::types::*;
use sha2::{Digest, Sha256};

pub const MAGIC: &[u8; 4] = b"COQD";
pub const FORMAT_VERSION: u16 = 2;
pub const MIN_READER: u16 = 2;
pub const BLOCK_RECORDS: usize = 256;
/// Pinned so identical input yields identical bytes (also pinned via Cargo.lock).
pub const ZSTD_LEVEL: i32 = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum SectionId {
    Meta = 1,
    Oracle = 2,
    Printing = 3,
    OracleUuids = 4,
    PrintingUuids = 5,
    Ranks = 6,
}

impl SectionId {
    pub fn name(self) -> &'static str {
        match self {
            SectionId::Meta => "meta",
            SectionId::Oracle => "oracle",
            SectionId::Printing => "printing",
            SectionId::OracleUuids => "oracle_uuids",
            SectionId::PrintingUuids => "printing_uuids",
            SectionId::Ranks => "oracle_ranks",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Dataset {
    pub meta: Meta,
    pub oracle: Vec<OracleRecord>,
    /// One entry per oracle row (same length as `oracle`).
    pub ranks: Vec<Ranks>,
    pub printings: Vec<PrintingRecord>,
}

#[derive(Clone, Debug)]
pub struct SectionStat {
    pub name: &'static str,
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

fn encode_blocks<T>(records: &[T], enc: impl Fn(&T, &mut Writer)) -> Result<(Vec<u8>, SectionStat, &'static str)> {
    let mut table = Writer::new();
    let mut data = Writer::new();
    let mut raw_total = 0u64;
    let mut nblocks = 0u32;
    for chunk in records.chunks(BLOCK_RECORDS) {
        let mut raw = Writer::new();
        for rec in chunk {
            let mut one = Writer::new();
            enc(rec, &mut one);
            raw.uvarint(one.buf.len() as u64);
            raw.bytes(&one.buf);
        }
        let comp = zstd_compress(&raw.buf)?;
        let hash: [u8; 8] = Sha256::digest(&comp)[..8].try_into().unwrap();
        table.u64(data.buf.len() as u64);
        table.u32(comp.len() as u32);
        table.u32(raw.buf.len() as u32);
        table.bytes(&hash);
        data.bytes(&comp);
        raw_total += raw.buf.len() as u64;
        nblocks += 1;
    }
    let mut out = Writer::new();
    out.u32(BLOCK_RECORDS as u32);
    out.u32(nblocks);
    out.bytes(&table.buf);
    out.bytes(&data.buf);
    let stored = out.buf.len() as u64;
    Ok((
        out.buf,
        SectionStat { name: "", raw_bytes: raw_total, stored_bytes: stored, blocks: nblocks },
        "",
    ))
}

fn uuid_section(ids: impl Iterator<Item = ([u8; 16], u32)>) -> Vec<u8> {
    let mut v: Vec<([u8; 16], u32)> = ids.collect();
    v.sort();
    let mut w = Writer::new();
    w.u32(v.len() as u32);
    for (id, row) in v {
        w.bytes(&id);
        w.u32(row);
    }
    w.buf
}

pub fn write(ds: &Dataset) -> Result<(Vec<u8>, Vec<SectionStat>)> {
    let mut meta = Writer::new();
    ds.meta.encode(&mut meta);

    let (oracle, mut ostat, _) = encode_blocks(&ds.oracle, |r, w| r.encode(w))?;
    ostat.name = "oracle";
    let (printing, mut pstat, _) = encode_blocks(&ds.printings, |r, w| r.encode(w))?;
    pstat.name = "printing";
    let ouuid = uuid_section(ds.oracle.iter().enumerate().map(|(i, r)| (r.oracle_id, i as u32)));
    let puuid = uuid_section(ds.printings.iter().enumerate().map(|(i, r)| (r.scryfall_id, i as u32)));

    let ranks_raw = encode_ranks(&ds.ranks);
    let mut ranks_sec = Writer::new();
    ranks_sec.u32(ranks_raw.len() as u32);
    ranks_sec.bytes(&zstd_compress(&ranks_raw)?);

    let sections: Vec<(SectionId, Vec<u8>)> = vec![
        (SectionId::Meta, meta.buf),
        (SectionId::Oracle, oracle),
        (SectionId::Printing, printing),
        (SectionId::OracleUuids, ouuid),
        (SectionId::PrintingUuids, puuid),
        (SectionId::Ranks, ranks_sec.buf),
    ];

    let mut stats = vec![ostat, pstat];
    for (id, bytes) in &sections {
        if matches!(id, SectionId::Meta | SectionId::OracleUuids | SectionId::PrintingUuids | SectionId::Ranks) {
            stats.push(SectionStat {
                name: id.name(),
                raw_bytes: bytes.len() as u64,
                stored_bytes: bytes.len() as u64,
                blocks: 0,
            });
        }
    }

    let header_len = 4 + 2 + 2 + 4 * 5 + sections.len() * 20;
    let mut out = Writer::new();
    out.bytes(MAGIC);
    out.u16(FORMAT_VERSION);
    out.u16(MIN_READER);
    out.u32(ds.meta.cards_epoch);
    out.u32(ds.meta.source_date);
    out.u32(ds.oracle.len() as u32);
    out.u32(ds.printings.len() as u32);
    out.u32(sections.len() as u32);
    let mut off = header_len as u64;
    for (id, bytes) in &sections {
        out.u32(*id as u32);
        out.u64(off);
        out.u64(bytes.len() as u64);
        off += bytes.len() as u64;
    }
    for (_, bytes) in &sections {
        out.bytes(bytes);
    }
    Ok((out.buf, stats))
}

pub struct Parsed<'a> {
    pub cards_epoch: u32,
    pub source_date: u32,
    pub oracle_count: u32,
    pub printing_count: u32,
    pub sections: Vec<(u32, &'a [u8])>,
}

impl<'a> Parsed<'a> {
    pub fn section(&self, id: SectionId) -> Result<&'a [u8]> {
        self.sections
            .iter()
            .find(|(i, _)| *i == id as u32)
            .map(|(_, s)| *s)
            .ok_or(Error::Corrupt("missing section"))
    }
}

pub fn parse(bytes: &[u8]) -> Result<Parsed<'_>> {
    let mut r = Reader::new(bytes);
    if r.take(4)? != MAGIC {
        return Err(Error::BadMagic);
    }
    let version = r.u16()?;
    if version < MIN_READER {
        return Err(Error::Unsupported(version)); // older layouts lack required sections
    }
    let min_reader = r.u16()?;
    if min_reader > FORMAT_VERSION {
        return Err(Error::Unsupported(min_reader));
    }
    let cards_epoch = r.u32()?;
    let source_date = r.u32()?;
    let oracle_count = r.u32()?;
    let printing_count = r.u32()?;
    let n = r.u32()? as usize;
    let mut sections = Vec::with_capacity(n);
    for _ in 0..n {
        let id = r.u32()?;
        let off = r.u64()? as usize;
        let len = r.u64()? as usize;
        let end = off.checked_add(len).ok_or(Error::Eof)?;
        if end > bytes.len() {
            return Err(Error::Corrupt("section out of range"));
        }
        sections.push((id, &bytes[off..end]));
    }
    Ok(Parsed { cards_epoch, source_date, oracle_count, printing_count, sections })
}

/// Block table of a block section (used by `coqc diff` and range-reading engines).
pub fn block_table(bytes: &[u8], id: SectionId) -> Result<Vec<BlockInfo>> {
    let p = parse(bytes)?;
    let sec = p.section(id)?;
    let mut r = Reader::new(sec);
    let _per = r.u32()?;
    let n = r.u32()? as usize;
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        let off = r.u64()?;
        let clen = r.u32()?;
        let rawlen = r.u32()?;
        let hash: [u8; 8] = r.take(8)?.try_into().unwrap();
        v.push(BlockInfo { off, clen, rawlen, hash });
    }
    Ok(v)
}

fn decode_blocks<T>(sec: &[u8], count: usize, dec: impl Fn(&mut Reader) -> Result<T>) -> Result<Vec<T>> {
    let mut r = Reader::new(sec);
    let _per = r.u32()?;
    let n = r.u32()? as usize;
    let mut table = Vec::with_capacity(n);
    for _ in 0..n {
        let off = r.u64()? as usize;
        let clen = r.u32()? as usize;
        let rawlen = r.u32()? as usize;
        r.take(8)?;
        table.push((off, clen, rawlen));
    }
    let data_start = 8 + n * 24;
    let mut out = Vec::with_capacity(count);
    for (off, clen, rawlen) in table {
        let start = data_start + off;
        let comp = sec.get(start..start + clen).ok_or(Error::Corrupt("block out of range"))?;
        let raw = zstd::bulk::decompress(comp, rawlen).map_err(|e| Error::Zstd(e.to_string()))?;
        let mut br = Reader::new(&raw);
        while !br.is_empty() {
            let len = br.uvarint()? as usize;
            let rec = br.take(len)?;
            out.push(dec(&mut Reader::new(rec))?);
        }
    }
    if out.len() != count {
        return Err(Error::Corrupt("record count mismatch"));
    }
    Ok(out)
}

pub fn read(bytes: &[u8]) -> Result<Dataset> {
    let p = parse(bytes)?;
    let meta = Meta::decode(&mut Reader::new(p.section(SectionId::Meta)?))?;
    let oracle = decode_blocks(p.section(SectionId::Oracle)?, p.oracle_count as usize, OracleRecord::decode)?;
    let printings =
        decode_blocks(p.section(SectionId::Printing)?, p.printing_count as usize, PrintingRecord::decode)?;
    let rsec = p.section(SectionId::Ranks)?;
    let mut rr = Reader::new(rsec);
    let rawlen = rr.u32()? as usize;
    let comp = rr.take(rsec.len() - 4)?;
    let raw = zstd::bulk::decompress(comp, rawlen).map_err(|e| Error::Zstd(e.to_string()))?;
    let ranks = decode_ranks(&raw)?;
    if ranks.len() != oracle.len() {
        return Err(Error::Corrupt("ranks length mismatch"));
    }
    Ok(Dataset { meta, oracle, ranks, printings })
}
