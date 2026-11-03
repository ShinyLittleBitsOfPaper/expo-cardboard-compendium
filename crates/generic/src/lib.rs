//! The schema-driven dialect of coq (S5 of docs/schema-design.md): everything a dataset needs that is *not* in its schema is
//! derived from the schema, so a new dataset is a schema file plus JSONL, with no code. It provides
//! - [`ingest`]: JSONL (one file per table) -> a `Store`, stable row ids across releases (`--prev`), tombstones, append-only
//!   dictionaries; and overlays (a columnar `Int` table whose row i follows row i of the base's primary table),
//! - [`query`]: a small query language whose fields, types and operators come from the schema (docs/generic.md),
//! - the [`Generic`] dialect: record JSON, key lookup, mask columns for `Set` columns, overlay predicates,
//! - [`demo`]: a synthetic "books" dataset used by the S5 tests and `scripts/test-generic-pipeline.sh`.
//! Nothing in this crate names Magic, Scryfall or any other dataset.
pub mod demo;
pub mod ingest;
pub mod query;

use coqc_format::dialect::Dialect;
use coqc_format::gindex::{DData, DerivedCol, GIndex, GQuery, Hooks, IndexSpec};
use coqc_format::schema::{ColType, IndexKind, Rel, Schema};
use coqc_format::search::{num_cmp, Bits, Op};
use coqc_format::table::{ColData, Store};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// Dialect id of datasets served by this crate. Any schema whose id is not a dedicated dialect's id (`mtg`) uses [`Generic`].
pub const DIALECT_ID: &str = "generic";
/// Largest dictionary a mask-indexed `Set` column may have (one bit per value in a `u32`).
pub const MAX_MASK_VALUES: usize = 32;

/// The 16 key bytes of a natural-key string: a UUID (32 hex digits, hyphens optional) is used as is, anything else is hashed
/// (first 16 bytes of its SHA-256), so any unique string can key a row.
pub fn key_bytes(s: &str) -> [u8; 16] {
    let hex: Vec<u8> = s.bytes().filter(|b| *b != b'-').collect();
    if hex.len() == 32 && hex.iter().all(u8::is_ascii_hexdigit) {
        let mut out = [0u8; 16];
        for (i, o) in out.iter_mut().enumerate() {
            let v = |b: u8| (b as char).to_digit(16).unwrap() as u8;
            *o = v(hex[2 * i]) << 4 | v(hex[2 * i + 1]);
        }
        return out;
    }
    let d = Sha256::digest(s.as_bytes());
    d[..16].try_into().unwrap()
}

/// Hyphenated lower-case form of 16 key bytes.
pub fn uuid_string(b: &[u8; 16]) -> String {
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// The table queries run over: the first with a parent, else the first table (the same rule the index and engine use).
pub fn primary_table(schema: &Schema) -> usize {
    schema.tables.iter().position(|t| matches!(t.rel, Rel::Parent(_))).unwrap_or(0)
}

pub struct Generic;

impl Dialect for Generic {
    fn id(&self) -> &'static str {
        DIALECT_ID
    }
    fn parse_gquery(&self, q: &str, st: &Store, overlays: &[(String, &Store)]) -> Result<GQuery, String> {
        query::parse(q, st, overlays)
    }
    /// Mask columns for `Set` columns indexed as `IndexKind::Set` (a bit per dictionary value; the dictionary is append-only so
    /// bits stay stable across releases). Text columns need nothing: the index folds them from the schema.
    fn index_spec(&self, st: &Store) -> Result<IndexSpec, String> {
        let mut derived = vec![];
        for (ti, def) in st.schema.tables.iter().enumerate() {
            for (ci, c) in def.columns.iter().enumerate() {
                let (ColType::Set(d), IndexKind::Set) = (&c.ty, c.index) else { continue };
                let n = st.schema.dict(d).map_or(0, |i| st.dicts[i].len());
                if n > MAX_MASK_VALUES {
                    return Err(format!("{}.{}: dictionary {d} has {n} values; a mask-indexed set holds at most {MAX_MASK_VALUES}", def.name, c.name));
                }
                let ColData::Set(v) = &st.tables[ti].cols[ci] else { continue };
                let mask = v.iter().map(|o| o.as_ref().map_or(0, |ids| ids.iter().fold(0u32, |m, &i| m | 1u32 << (i % 32)))).collect();
                derived.push(DerivedCol { table: def.name.clone(), name: c.name.clone(), data: DData::Mask(mask), tri_group: None });
            }
        }
        Ok(IndexSpec { derived, skip_auto: vec![] })
    }
    fn hooks(&self) -> &dyn Hooks {
        &GenericHooks
    }
    fn record_json(&self, st: &Store, row: u32) -> Value {
        Value::Object(record(st, primary_table(&st.schema), row as usize, 0))
    }
    fn key_map(&self, st: &Store) -> HashMap<Vec<u8>, u32> {
        let p = primary_table(&st.schema);
        let Some(k) = st.schema.tables[p].key.as_ref().and_then(|k| st.schema.tables[p].columns.iter().position(|c| &c.name == k)) else {
            return HashMap::new();
        };
        let ColData::Uuid(ids) = &st.tables[p].cols[k] else { return HashMap::new() };
        let t = &st.tables[p];
        ids.iter().enumerate().filter(|(i, _)| !t.tomb[*i]).map(|(i, id)| (id.to_vec(), i as u32)).collect()
    }
    fn parse_key(&self, key: &str) -> Option<Vec<u8>> {
        (!key.is_empty()).then(|| key_bytes(key).to_vec())
    }
}

/// Predicates on overlay columns: `Custom { name: "overlay", args: [overlay id, column, op, number] }`, evaluated over primary rows
/// (row i of an overlay follows row i of the primary table; a row beyond the overlay or with no value never matches).
pub struct GenericHooks;

impl Hooks for GenericHooks {
    fn custom(&self, ix: &GIndex, name: &str, args: &[String]) -> Result<Bits, String> {
        if name != "overlay" {
            return Err(format!("unknown predicate {name}"));
        }
        let [id, col, op, val] = args else { return Err("bad overlay predicate".into()) };
        let st = ix.overlay(id).ok_or_else(|| format!("overlay {id} is not attached"))?;
        let op: Op = serde_json::from_value(Value::String(op.clone())).map_err(|_| format!("bad operator {op}"))?;
        let q: f64 = val.parse().map_err(|_| format!("{val} is not a number"))?;
        let def = st.schema.tables.first().ok_or("overlay has no table")?;
        let ci = def.columns.iter().position(|c| &c.name == col).ok_or_else(|| format!("overlay {id} has no column {col}"))?;
        let get: Box<dyn Fn(usize) -> Option<f64> + '_> = match &st.tables[0].cols[ci] {
            ColData::Int(v) => Box::new(move |r| v.get(r).copied().flatten().map(|x| x as f64)),
            ColData::Float(v) => Box::new(move |r| v.get(r).copied().flatten()),
            _ => return Err(format!("overlay column {col} is not numeric")),
        };
        let primary = &ix.store().schema.tables[primary_table(&ix.store().schema)].name;
        ix.pred(primary, |r| get(r).is_some_and(|v| num_cmp(v, q, op)))
    }
}

fn cell(st: &Store, ti: usize, ci: usize, row: usize, depth: usize) -> Value {
    let def = &st.schema.tables[ti];
    match &st.tables[ti].cols[ci] {
        ColData::Str(v) => v[row].clone().map_or(Value::Null, Value::String),
        ColData::Int(v) => v[row].map_or(Value::Null, |x| json!(x)),
        ColData::Float(v) => v[row].map_or(Value::Null, |x| json!(x)),
        ColData::Bool(v) => v[row].map_or(Value::Null, Value::Bool),
        ColData::Uuid(v) => Value::String(uuid_string(&v[row])),
        ColData::Enum(v) => match (&def.columns[ci].ty, v[row]) {
            (ColType::Enum(d), Some(id)) => st.schema.dict(d).and_then(|i| st.dicts[i].get(id)).map_or(Value::Null, |s| json!(s)),
            _ => Value::Null,
        },
        ColData::Set(v) => match (&def.columns[ci].ty, &v[row]) {
            (ColType::Set(d), Some(ids)) => {
                let di = st.schema.dict(d);
                Value::Array(ids.iter().filter_map(|&id| di.and_then(|i| st.dicts[i].get(id))).map(|s| json!(s)).collect())
            }
            _ => Value::Null,
        },
        ColData::Ref(v) => match &def.columns[ci].ty {
            ColType::Ref(p) if depth < 4 => st.schema.table(p).filter(|&pi| (v[row] as usize) < st.tables[pi].rows()).map_or(Value::Null, |pi| Value::Object(record(st, pi, v[row] as usize, depth + 1))),
            _ => Value::Null,
        },
        ColData::Json(v) => v[row].as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null),
        // opaque dialect bytes have no generic JSON form
        ColData::Bytes(_) => Value::Null,
    }
}

/// One row as a JSON object: every column by name, a `Ref` column as the nested parent record, sidecar tables flattened in.
fn record(st: &Store, ti: usize, row: usize, depth: usize) -> Map<String, Value> {
    let mut m = Map::new();
    let def = &st.schema.tables[ti];
    for (ci, c) in def.columns.iter().enumerate() {
        if matches!(c.ty, ColType::Bytes) {
            continue;
        }
        m.insert(c.name.clone(), cell(st, ti, ci, row, depth));
    }
    for (si, sd) in st.schema.tables.iter().enumerate() {
        if matches!(&sd.rel, Rel::Sidecar(p) if *p == def.name) && row < st.tables[si].rows() {
            for (ci, c) in sd.columns.iter().enumerate() {
                if !matches!(c.ty, ColType::Bytes | ColType::Ref(_)) {
                    m.insert(c.name.clone(), cell(st, si, ci, row, depth));
                }
            }
        }
    }
    m
}
