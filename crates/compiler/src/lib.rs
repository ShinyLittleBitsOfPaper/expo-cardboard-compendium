//! `coqc` library: the CLI's generic parts (manifest) plus re-exports of the MTG dialect so the old `coqc::compile` paths keep working.
pub use coqc_format::patch;
pub mod manifest;
pub use coqc_mtg::{prices, quick, synth};
pub use coqc_mtg::ingest::*;
