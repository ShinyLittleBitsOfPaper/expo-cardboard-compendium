//! COQR price table + COQX delta. Separate from the card dataset: rows are PRINTING rows of the card
//! file (same epoch), so card patches never carry prices and price deltas never touch cards.
//!
//! Six columns of retail prices (TCGplayer USD, Cardmarket EUR) x (normal, foil, etched). A value is
//! 0 = absent, else cents + 1. Rows past the table's length are absent.
//!
//! File: `"COQR" | u16 version | u16 min_reader | u32 cards_epoch | u32 date YYYYMMDD | u32 rows
//!        | u32 raw_len | zstd(6 columns x rows varints)`.
//! Delta: `"COQX" | u16 version | sha256(base file) | sha256(target file) | u32 raw_len | zstd(body)`,
//!        body = `u32 epoch | u32 date | u32 rows | 6 columns x rows zigzag varint (new - old)`.

use crate::codec::{Error, Reader, Result, Writer};
use sha2::{Digest, Sha256};

pub const COLUMNS: [&str; 6] = ["tcg_normal", "tcg_foil", "tcg_etched", "cm_normal", "cm_foil", "cm_etched"];
const MAGIC: &[u8; 4] = b"COQR";
const DMAGIC: &[u8; 4] = b"COQX";
const VERSION: u16 = 1;
const LEVEL: i32 = 15;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PriceTable {
    pub cards_epoch: u32,
    pub date: u32,
    pub cols: [Vec<u32>; 6],
}

pub fn sha(b: &[u8]) -> [u8; 32] {
    Sha256::digest(b).into()
}

fn zstd_err(e: std::io::Error) -> Error {
    Error::Zstd(e.to_string())
}

fn zz(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}
fn unzz(v: u64) -> i64 {
    (v >> 1) as i64 ^ -((v & 1) as i64)
}

impl PriceTable {
    pub fn rows(&self) -> usize {
        self.cols[0].len()
    }

    pub fn new(cards_epoch: u32, date: u32, rows: usize) -> Self {
        Self { cards_epoch, date, cols: std::array::from_fn(|_| vec![0; rows]) }
    }

    /// cents for (row, column), None = absent
    pub fn cents(&self, row: usize, col: usize) -> Option<u32> {
        self.cols[col].get(row).copied().filter(|v| *v != 0).map(|v| v - 1)
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut body = Writer::new();
        for c in &self.cols {
            c.iter().for_each(|v| body.uvarint(*v as u64));
        }
        let z = zstd::bulk::compress(&body.buf, LEVEL).map_err(zstd_err)?;
        let mut w = Writer::new();
        w.bytes(MAGIC);
        w.u16(VERSION);
        w.u16(VERSION);
        w.u32(self.cards_epoch);
        w.u32(self.date);
        w.u32(self.rows() as u32);
        w.u32(body.buf.len() as u32);
        w.bytes(&z);
        Ok(w.buf)
    }

    pub fn decode(b: &[u8]) -> Result<Self> {
        let mut r = Reader::new(b);
        if r.take(4)? != MAGIC {
            return Err(Error::BadMagic);
        }
        let (_ver, min) = (r.u16()?, r.u16()?);
        if min > VERSION {
            return Err(Error::Unsupported(min));
        }
        let (cards_epoch, date, rows, raw_len) = (r.u32()?, r.u32()?, r.u32()? as usize, r.u32()? as usize);
        let raw = zstd::bulk::decompress(r.take(b.len() - 24)?, raw_len).map_err(zstd_err)?;
        let mut rr = Reader::new(&raw);
        let mut t = PriceTable::new(cards_epoch, date, rows);
        for c in 0..6 {
            for i in 0..rows {
                t.cols[c][i] = rr.uvarint32()?;
            }
        }
        Ok(t)
    }
}

/// Delta from `old` file bytes to `new` file bytes (same epoch, rows must not shrink).
pub fn make_delta(old_bytes: &[u8], new_bytes: &[u8]) -> Result<Vec<u8>> {
    let (old, new) = (PriceTable::decode(old_bytes)?, PriceTable::decode(new_bytes)?);
    if old.cards_epoch != new.cards_epoch || new.rows() < old.rows() {
        return Err(Error::Corrupt("epoch changed or rows shrank: clients need the full file"));
    }
    let mut body = Writer::new();
    body.u32(new.cards_epoch);
    body.u32(new.date);
    body.u32(new.rows() as u32);
    for c in 0..6 {
        for i in 0..new.rows() {
            let o = old.cols[c].get(i).copied().unwrap_or(0) as i64;
            body.uvarint(zz(new.cols[c][i] as i64 - o));
        }
    }
    let z = zstd::bulk::compress(&body.buf, 19).map_err(zstd_err)?;
    let mut w = Writer::new();
    w.bytes(DMAGIC);
    w.u16(VERSION);
    w.bytes(&sha(old_bytes));
    w.bytes(&sha(new_bytes));
    w.u32(body.buf.len() as u32);
    w.bytes(&z);
    Ok(w.buf)
}

pub fn apply_delta(old_bytes: &[u8], delta: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader::new(delta);
    if r.take(4)? != DMAGIC {
        return Err(Error::BadMagic);
    }
    let ver = r.u16()?;
    if ver > VERSION {
        return Err(Error::Unsupported(ver));
    }
    let (base, target) = (r.take(32)?, r.take(32)?);
    if base != sha(old_bytes) {
        return Err(Error::Corrupt("delta does not match this base file"));
    }
    let raw_len = r.u32()? as usize;
    let raw = zstd::bulk::decompress(r.take(delta.len() - 74)?, raw_len).map_err(zstd_err)?;
    let old = PriceTable::decode(old_bytes)?;
    let mut rr = Reader::new(&raw);
    let (epoch, date, rows) = (rr.u32()?, rr.u32()?, rr.u32()? as usize);
    let mut t = PriceTable::new(epoch, date, rows);
    for c in 0..6 {
        for i in 0..rows {
            let o = old.cols[c].get(i).copied().unwrap_or(0) as i64;
            t.cols[c][i] = u32::try_from(o + unzz(rr.uvarint()?)).map_err(|_| Error::Corrupt("bad price delta"))?;
        }
    }
    let out = t.encode()?;
    if sha(&out) != target {
        return Err(Error::Corrupt("result hash mismatch: download the full file"));
    }
    Ok(out)
}
