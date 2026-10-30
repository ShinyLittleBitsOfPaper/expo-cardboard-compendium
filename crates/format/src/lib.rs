//! COQD: a compact, block-compressed, lossless, schema-driven dataset format.
pub mod codec;
pub mod container;
pub mod coqd;
pub mod dialect;
pub mod patch;
pub mod prices;
pub mod schema;
pub mod search;
pub mod table;
pub mod types;

pub use codec::{Error, Result};
pub use container::Dataset;
pub use coqd::{read_store, write_store, BlockInfo, SectionStat};
pub use types::*;
