//! The seam between the generic machinery and a dataset's own semantics. A dialect owns the query syntax, the derived index
//! columns and custom predicates, the JSON shape of a record, and the natural-key lookup; the container, patch, index and sync
//! code never name a dataset. `row` is a row of the primary table (MTG: a printing).
use crate::gindex::{GQuery, Hooks, IndexSpec};
use crate::search::Query;
use crate::table::Store;

pub trait Dialect: Send + Sync {
    /// Stable id stored in schemas, e.g. `"mtg"`.
    fn id(&self) -> &'static str;
    /// Parse the dataset's query language into the legacy query tree (kept for the S3a cross-check tests; removed in S3c).
    fn parse_query(&self, q: &str) -> Result<Query, String>;
    /// Parse the dataset's query language into the generic query tree (sort/unique/prefer/hide directives included).
    fn parse_gquery(&self, q: &str) -> Result<GQuery, String>;
    /// Derived columns and the automatic text columns to skip, computed from the store at index-build time.
    fn index_spec(&self, st: &Store) -> Result<IndexSpec, String>;
    /// Evaluator for `Custom` predicates.
    fn hooks(&self) -> &dyn Hooks;
    /// The public JSON for one record.
    fn record_json(&self, st: &Store, row: u32) -> serde_json::Value;
    /// Natural key (MTG: Scryfall id bytes) -> live row, for every live row.
    fn key_map(&self, st: &Store) -> std::collections::HashMap<Vec<u8>, u32>;
    /// Parse a key as given by a caller (MTG: a Scryfall id string) into the bytes used by `key_map`.
    fn parse_key(&self, key: &str) -> Option<Vec<u8>>;
}
