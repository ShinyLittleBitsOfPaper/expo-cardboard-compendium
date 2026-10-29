//! The seam between the generic machinery and a dataset's own semantics (S1 of docs/schema-design.md). A dialect owns the query
//! syntax, the JSON shape of a record, and the natural-key lookup; the container, patch, index and sync code never name a
//! dataset. Until the table model lands (S2) records are still the oracle/printing pair, so `row` means a printing row.
use crate::search::Query;
use crate::Dataset;

pub trait Dialect: Send + Sync {
    /// Stable id stored in (future) schemas, e.g. `"mtg"`.
    fn id(&self) -> &'static str;
    /// Parse the dataset's query language into the generic query tree (including sort/unique/prefer directives).
    fn parse_query(&self, q: &str) -> Result<Query, String>;
    /// The public JSON for one record.
    fn record_json(&self, ds: &Dataset, row: u32) -> serde_json::Value;
    /// Natural key (MTG: Scryfall id bytes) -> live row, for every live row.
    fn key_map(&self, ds: &Dataset) -> std::collections::HashMap<Vec<u8>, u32>;
    /// Parse a key as given by a caller (MTG: a Scryfall id string) into the bytes used by `key_map`.
    fn parse_key(&self, key: &str) -> Option<Vec<u8>>;
}
