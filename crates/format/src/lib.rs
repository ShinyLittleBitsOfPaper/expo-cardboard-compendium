//! COQD: a compact, block-compressed, lossless card dataset format.
pub mod codec;
pub mod container;
pub mod json;
pub mod search;
pub mod types;

pub use codec::{Error, Result};
pub use container::{read, write, BlockInfo, Dataset, SectionId, SectionStat};
pub use types::*;
