//! `coqc` library: the CLI's generic parts (manifest) plus re-exports of the MTG dialect so the old `coqc::compile` paths keep working.
pub use coqc_format::patch;
pub mod arthash;
pub mod manifest;
pub use coqc_mtg::{prices, quick, synth};
pub use coqc_mtg::ingest::*;

/// The dialect serving a dataset: `mtg` schemas use the Magic dialect, every other schema the schema-driven generic one.
pub fn dialect_for(st: &coqc_format::table::Store) -> Box<dyn coqc_format::dialect::Dialect> {
    match st.schema.id.as_str() {
        "mtg" => Box::new(coqc_mtg::Mtg),
        _ => Box::new(coqc_generic::Generic),
    }
}

/// Build the serialised search index (`.coqi` bytes) for the bytes of a `.coqd` (any dataset).
pub fn build_index_file(coqd_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let st = coqc_format::read_store(coqd_bytes).map_err(|e| e.to_string())?;
    let spec = dialect_for(&st).index_spec(&st)?;
    let parts = coqc_format::gindex::GIndex::build_parts(&st, spec, coqc_format::search::STOP_FRACTION, coqc_format::search::STOP_MIN)?;
    Ok(coqc_format::coqi::serialize(&parts, &st.schema, &coqc_format::coqi::sha(coqd_bytes)))
}
