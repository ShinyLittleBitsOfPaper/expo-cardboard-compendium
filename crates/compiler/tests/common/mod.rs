//! Shared test helper: a built generic index over an MTG `Dataset`, searched with the legacy Scryfall query tree.
#![allow(dead_code)]
use coqc_format::gindex::{GIndex, GResult, IndexParts};
use coqc_format::search::{STOP_FRACTION, STOP_MIN};
use coqc_format::table::Store;
use coqc_mtg::gindex::{index_spec, translate, MtgHooks};
use coqc_mtg::{Dataset, Query};

pub struct Ix {
    pub store: Store,
    pub parts: IndexParts,
}

impl Ix {
    pub fn build(ds: &Dataset) -> Ix {
        Ix::build_with(ds, STOP_FRACTION, STOP_MIN)
    }

    pub fn build_with(ds: &Dataset, stop_fraction: f64, stop_min: usize) -> Ix {
        let store = coqc_mtg::tables::to_store(ds);
        let parts = GIndex::build_parts(&store, index_spec(&store).unwrap(), stop_fraction, stop_min).unwrap();
        Ix { store, parts }
    }

    pub fn index(&self) -> GIndex<'_> {
        GIndex::with_parts(&self.store, Some(&MtgHooks), &self.parts).unwrap()
    }

    pub fn search(&self, q: &Query) -> GResult {
        self.index().search(&translate(q)).unwrap()
    }
}
