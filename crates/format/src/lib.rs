//! COQD: a compact, block-compressed, lossless, schema-driven dataset format, plus the generic patch, price-table and search-index
//! machinery that works on it. Everything dataset specific (MTG) lives in a dialect crate (`coqc-mtg`).
pub mod codec;
pub mod coqd;
pub mod coqi;
pub mod dialect;
pub mod dict;
pub mod gindex;
pub mod patch;
pub mod prices;
pub mod schema;
pub mod search;
pub mod table;

pub use codec::{Error, Result};
pub use coqd::{read_store, write_store, BlockInfo, SectionStat};
pub use dict::Dict;
