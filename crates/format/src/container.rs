//! The legacy in-memory dataset shape (oracle / printing / ranks vectors). The file format is now schema driven (`coqd.rs`); this
//! struct remains the working shape of the search index and the MTG ingestion until S2c removes it from `coqc-format`.
use crate::types::*;

#[derive(Clone, Debug)]
pub struct Dataset {
    pub meta: Meta,
    pub oracle: Vec<OracleRecord>,
    /// One entry per oracle row (same length as `oracle`).
    pub ranks: Vec<Ranks>,
    pub printings: Vec<PrintingRecord>,
}
