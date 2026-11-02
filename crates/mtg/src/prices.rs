//! MTGJSON prices -> a `.coqd` overlay (S4a: no separate price format any more). v1 scope: retail only, TCGplayer (USD) and Cardmarket (EUR), paper.
//! MTGJSON price entries are keyed by MTGJSON uuid; `AllIdentifiers` maps uuid -> `identifiers.scryfallId`.
//! The id map is cached (COQM file) and rebuilt only when the prices file has uuids it does not know.

use coqc_format::codec::{Error, Reader, Writer};
use coqc_format::coqd::{read_store, write_store};
use coqc_format::schema::{ColType, ColumnDef, IndexKind, Rel, Schema, TableDef};
use coqc_format::table::{ColData, Store, Table};
use crate::json::parse_uuid;
use crate::types::Dataset;
use serde::Deserialize;
use std::collections::HashMap;

type Id = [u8; 16];


// ----------------------------------------------------------------- overlay ---

pub const COLUMNS: [&str; 6] = ["tcg_normal", "tcg_foil", "tcg_etched", "cm_normal", "cm_foil", "cm_etched"];
/// Schema id of the price overlay.
pub const SCHEMA_ID: &str = "mtg-prices";
/// Store prop naming the table of the base dataset whose rows the overlay's rows follow (row i = row i).
pub const PROP_OVERLAY_OF: &str = "overlay_of";

/// The price overlay's schema: one table `price`, one nullable Int column per provider x finish holding cents (None = no price).
pub fn price_schema() -> Schema {
    Schema {
        id: SCHEMA_ID.into(),
        version: 1,
        dicts: vec![],
        tables: vec![TableDef {
            name: "price".into(),
            rel: Rel::None,
            key: None,
            columnar: true,
            columns: COLUMNS.iter().map(|c| ColumnDef::new(c, ColType::Int, IndexKind::Range, &[])).collect(),
        }],
    }
}

/// In-memory price table: rows are PRINTING rows of the card file of the same epoch; a value is 0 = absent, else cents + 1.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PriceTable {
    pub cards_epoch: u32,
    pub date: u32,
    pub cols: [Vec<u32>; 6],
}

impl PriceTable {
    pub fn new(cards_epoch: u32, date: u32, rows: usize) -> Self {
        Self { cards_epoch, date, cols: std::array::from_fn(|_| vec![0; rows]) }
    }
    pub fn rows(&self) -> usize {
        self.cols[0].len()
    }
    /// cents for (row, column), None = absent
    pub fn cents(&self, row: usize, col: usize) -> Option<u32> {
        self.cols[col].get(row).copied().filter(|v| *v != 0).map(|v| v - 1)
    }
    pub fn to_store(&self) -> Store {
        let mut st = Store::new(price_schema());
        st.epoch = self.cards_epoch;
        st.source_date = self.date;
        st.props = vec![(PROP_OVERLAY_OF.into(), vec!["printing".into()])];
        let t: &mut Table = &mut st.tables[0];
        t.tomb = vec![false; self.rows()];
        t.cols = self.cols.iter().map(|c| ColData::Int(c.iter().map(|&v| (v != 0).then(|| v as i64 - 1)).collect())).collect();
        st
    }
    pub fn from_store(st: &Store) -> Result<Self, Error> {
        if st.schema != price_schema() {
            return Err(Error::Corrupt("not a price overlay"));
        }
        let t = &st.tables[0];
        let mut p = PriceTable::new(st.epoch, st.source_date, t.rows());
        for (c, d) in t.cols.iter().enumerate() {
            let ColData::Int(v) = d else { return Err(Error::Corrupt("bad price column")) };
            for (i, x) in v.iter().enumerate() {
                p.cols[c][i] = x.map_or(0, |x| x as u32 + 1);
            }
        }
        Ok(p)
    }
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        Ok(write_store(&self.to_store())?.0)
    }
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        Self::from_store(&read_store(b)?)
    }
}

// ------------------------------------------------------------------ id map ---

#[derive(Deserialize)]
struct Ids {
    #[serde(rename = "scryfallId", default)]
    scryfall_id: Option<String>,
}
#[derive(Deserialize)]
struct IdEntry {
    #[serde(default)]
    identifiers: Option<Ids>,
}
#[derive(Deserialize)]
struct IdFile {
    data: HashMap<String, IdEntry>,
}

/// COQM: `"COQM" | u32 count | u32 raw_len | zstd(count x {mtgjson uuid, scryfall id})`, sorted by uuid.
pub fn build_idmap(json: &[u8]) -> Result<(Vec<u8>, usize), String> {
    let f: IdFile = serde_json::from_slice(json).map_err(|e| format!("AllIdentifiers: {e}"))?;
    let mut v: Vec<(Id, Id)> = f
        .data
        .iter()
        .filter_map(|(k, e)| Some((parse_uuid(k)?, parse_uuid(e.identifiers.as_ref()?.scryfall_id.as_deref()?)?)))
        .collect();
    v.sort_unstable();
    let mut w = Writer::new();
    v.iter().for_each(|(a, b)| {
        w.bytes(a);
        w.bytes(b);
    });
    let z = zstd::bulk::compress(&w.buf, 15).map_err(|e| e.to_string())?;
    let mut o = Writer::new();
    o.bytes(b"COQM");
    o.u32(v.len() as u32);
    o.u32(w.buf.len() as u32);
    o.bytes(&z);
    Ok((o.buf, v.len()))
}

pub fn load_idmap(b: &[u8]) -> Result<HashMap<Id, Id>, Error> {
    let mut r = Reader::new(b);
    if r.take(4)? != b"COQM" {
        return Err(Error::BadMagic);
    }
    let (n, raw_len) = (r.u32()? as usize, r.u32()? as usize);
    let raw = coqc_format::codec::zstd_exact(r.take(b.len() - 12)?, raw_len).map_err(Error::Zstd)?;
    let mut rr = Reader::new(&raw);
    let mut m = HashMap::with_capacity(rr.cap(n));
    for _ in 0..n {
        let a: Id = rr.take(16)?.try_into().unwrap();
        let b: Id = rr.take(16)?.try_into().unwrap();
        m.insert(a, b);
    }
    Ok(m)
}

// ------------------------------------------------------------------ prices ---

#[derive(Deserialize)]
struct PMeta {
    date: String,
}
#[derive(Deserialize, Default)]
struct Prov {
    #[serde(default)]
    retail: HashMap<String, HashMap<String, f64>>,
}
#[derive(Deserialize, Default)]
struct PEntry {
    #[serde(default)]
    paper: HashMap<String, Prov>,
}
#[derive(Deserialize)]
struct PFile {
    meta: PMeta,
    data: HashMap<String, PEntry>,
}

#[derive(Debug, Default)]
pub struct PriceReport {
    pub date: u32,
    pub entries: usize,
    pub unmapped: usize,
    pub not_in_cards: usize,
    pub conflicts: usize,
    pub rows_priced: usize,
    pub per_column: [usize; 6],
}

fn cents(x: f64) -> Option<u32> {
    (x.is_finite() && (0.0..40_000_000.0).contains(&x)).then(|| (x * 100.0).round() as u32 + 1)
}

pub fn compile_prices(ds: &Dataset, idmap: &HashMap<Id, Id>, json: &[u8]) -> Result<(PriceTable, PriceReport), String> {
    let f: PFile = serde_json::from_slice(json).map_err(|e| format!("prices: {e}"))?;
    let date: u32 = f.meta.date.replace('-', "").parse().map_err(|_| "bad meta.date".to_string())?;
    let rows: HashMap<Id, u32> =
        ds.printings.iter().enumerate().filter(|(_, p)| !p.tombstone).map(|(i, p)| (p.scryfall_id, i as u32)).collect();
    let mut t = PriceTable::new(ds.meta.cards_epoch, date, ds.printings.len());
    let mut rep = PriceReport { date, entries: f.data.len(), ..Default::default() };
    // sorted by uuid so conflicts resolve deterministically
    let mut keys: Vec<&String> = f.data.keys().collect();
    keys.sort();
    for k in keys {
        let Some(sid) = parse_uuid(k).and_then(|u| idmap.get(&u)) else {
            rep.unmapped += 1;
            continue;
        };
        let Some(&row) = rows.get(sid) else {
            rep.not_in_cards += 1;
            continue;
        };
        for (provider, base) in [("tcgplayer", 0usize), ("cardmarket", 3)] {
            let Some(p) = f.data[k].paper.get(provider) else { continue };
            for (fi, finish) in ["normal", "foil", "etched"].iter().enumerate() {
                // latest date key in the entry (the Today file has exactly one)
                let Some(v) = p.retail.get(*finish).and_then(|m| m.iter().max_by(|a, b| a.0.cmp(b.0))).and_then(|(_, v)| cents(*v)) else {
                    continue;
                };
                let slot = &mut t.cols[base + fi][row as usize];
                if *slot != 0 && *slot != v {
                    rep.conflicts += 1;
                } else {
                    *slot = v;
                }
            }
        }
    }
    for c in 0..6 {
        rep.per_column[c] = t.cols[c].iter().filter(|v| **v != 0).count();
    }
    rep.rows_priced = (0..t.rows()).filter(|&i| (0..6).any(|c| t.cols[c][i] != 0)).count();
    Ok((t, rep))
}
