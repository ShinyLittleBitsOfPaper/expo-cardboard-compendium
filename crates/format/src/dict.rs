//! Append-only string dictionary used by schema `Enum`/`Set` columns.

use crate::codec::*;
use std::collections::HashMap;

/// Append-only string dictionary. IDs are positions and must never be reordered
/// between releases, otherwise unchanged records would re-encode differently.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct Dict {
    values: Vec<String>,
    index: HashMap<String, u32>,
}

impl Dict {
    pub fn from_values(values: Vec<String>) -> Self {
        let index = values
            .iter()
            .enumerate()
            .map(|(i, s)| (s.clone(), i as u32))
            .collect();
        Self { values, index }
    }
    pub fn id(&self, s: &str) -> Option<u32> {
        self.index.get(s).copied()
    }
    pub fn push(&mut self, s: &str) -> u32 {
        if let Some(i) = self.index.get(s) {
            return *i;
        }
        let i = self.values.len() as u32;
        self.values.push(s.to_string());
        self.index.insert(s.to_string(), i);
        i
    }
    pub fn get(&self, id: u32) -> Option<&str> {
        self.values.get(id as usize).map(|s| s.as_str())
    }
    pub fn len(&self) -> usize {
        self.values.len()
    }
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
    pub fn values(&self) -> &[String] {
        &self.values
    }
    pub fn encode(&self, w: &mut Writer) {
        w.uvarint(self.values.len() as u64);
        for v in &self.values {
            w.str(v);
        }
    }
    pub fn decode(r: &mut Reader) -> Result<Self> {
        let n = r.uvarint()? as usize;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(r.string()?);
        }
        Ok(Self::from_values(v))
    }
}

