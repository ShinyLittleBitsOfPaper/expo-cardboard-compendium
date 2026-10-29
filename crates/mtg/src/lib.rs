//! The Magic: The Gathering dialect of coq. Everything that knows about Scryfall's card JSON, MTGJSON prices or Scryfall query
//! syntax lives here; `coqc-format` holds the container, patch, price-table and index machinery. See docs/schema-design.md (S1).

pub mod ingest;
pub mod json;
pub mod parse;
pub mod prices;
pub mod quick;
pub mod synth;

pub use ingest::*;
pub use parse::ParseError;

use coqc_format::search::Query;
use coqc_format::{Dataset, Meta, OracleRecord, PrintingRecord, Ranks};

/// The MTG implementation of [`coqc_format::dialect::Dialect`].
pub struct Mtg;

impl coqc_format::dialect::Dialect for Mtg {
    fn id(&self) -> &'static str {
        "mtg"
    }
    fn parse_query(&self, q: &str) -> Result<Query, String> {
        parse::parse(q).map_err(|e| e.0)
    }
    fn record_json(&self, ds: &Dataset, row: u32) -> serde_json::Value {
        let p: &PrintingRecord = &ds.printings[row as usize];
        let o: &OracleRecord = &ds.oracle[p.oracle_row as usize];
        let r: &Ranks = &ds.ranks[p.oracle_row as usize];
        let m: &Meta = &ds.meta;
        json::card_json(m, o, r, p)
    }
    fn key_map(&self, ds: &Dataset) -> std::collections::HashMap<Vec<u8>, u32> {
        ds.printings.iter().enumerate().filter(|(_, p)| !p.tombstone).map(|(i, p)| (p.scryfall_id.to_vec(), i as u32)).collect()
    }
    fn parse_key(&self, key: &str) -> Option<Vec<u8>> {
        json::parse_uuid(key).map(|u| u.to_vec())
    }
}
