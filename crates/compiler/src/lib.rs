//! `coqc` library: the CLI's generic parts (manifest) plus re-exports of the MTG dialect so the old `coqc::compile` paths keep working.
pub use coqc_format::patch;
pub mod manifest;
pub use coqc_mtg::{prices, quick, synth};
pub use coqc_mtg::ingest::*;

/// Build the serialised search index (`.coqi` bytes) for the bytes of an MTG `.coqd`.
pub fn build_index_file(coqd_bytes: &[u8]) -> Result<Vec<u8>, String> {
    use coqc_format::dialect::Dialect;
    let st = coqc_format::read_store(coqd_bytes).map_err(|e| e.to_string())?;
    let spec = coqc_mtg::Mtg.index_spec(&st)?;
    let parts = coqc_format::gindex::GIndex::build_parts(&st, spec, coqc_format::search::STOP_FRACTION, coqc_format::search::STOP_MIN)?;
    Ok(coqc_format::coqi::serialize(&parts, &st.schema, &coqc_format::coqi::sha(coqd_bytes)))
}
