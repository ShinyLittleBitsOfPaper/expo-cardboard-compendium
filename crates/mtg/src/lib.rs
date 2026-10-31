//! The Magic: The Gathering dialect of coq. Everything that knows about Scryfall's card JSON, MTGJSON prices or Scryfall query
//! syntax lives here; `coqc-format` holds the container, patch, price-table and index machinery. See docs/schema-design.md (S1).

pub mod gindex;
pub mod ingest;
pub mod json;
pub mod parse;
pub mod prices;
pub mod quick;
pub mod synth;
pub mod tables;

pub use ingest::*;
pub use parse::ParseError;
pub use tables::{read, write};

use coqc_format::search::Query;

/// The MTG implementation of [`coqc_format::dialect::Dialect`].
pub struct Mtg;

impl coqc_format::dialect::Dialect for Mtg {
    fn id(&self) -> &'static str {
        "mtg"
    }
    fn parse_query(&self, q: &str) -> Result<Query, String> {
        parse::parse(q).map_err(|e| e.0)
    }
    fn parse_gquery(&self, q: &str) -> Result<coqc_format::gindex::GQuery, String> {
        Ok(gindex::translate(&parse::parse(q).map_err(|e| e.0)?))
    }
    fn index_spec(&self, st: &coqc_format::table::Store) -> Result<coqc_format::gindex::IndexSpec, String> {
        gindex::index_spec(st)
    }
    fn hooks(&self) -> &dyn coqc_format::gindex::Hooks {
        &gindex::MtgHooks
    }
    fn record_json(&self, st: &coqc_format::table::Store, row: u32) -> serde_json::Value {
        let build = || -> coqc_format::Result<serde_json::Value> {
            let p = tables::printing_at(st, row as usize)?;
            let o = tables::oracle_at(st, p.oracle_row as usize)?;
            let r = tables::ranks_at(st, p.oracle_row as usize)?;
            Ok(json::card_json(&tables::meta_of(st), &o, &r, &p))
        };
        build().unwrap_or(serde_json::Value::Null)
    }
    fn key_map(&self, st: &coqc_format::table::Store) -> std::collections::HashMap<Vec<u8>, u32> {
        let (Some(t), Some(coqc_format::table::ColData::Uuid(ids))) = (st.tables.get(2), st.tables.get(2).and_then(|t| t.cols.first())) else {
            return Default::default();
        };
        ids.iter().enumerate().filter(|(i, _)| !t.tomb[*i]).map(|(i, id)| (id.to_vec(), i as u32)).collect()
    }
    fn parse_key(&self, key: &str) -> Option<Vec<u8>> {
        json::parse_uuid(key).map(|u| u.to_vec())
    }
}
