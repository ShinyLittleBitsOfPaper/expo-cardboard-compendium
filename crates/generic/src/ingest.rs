//! JSONL -> `Store` for any schema (no dataset-specific code). One JSONL file per table; a line is an object whose keys are the
//! table's column names. Rules (docs/generic.md):
//! - every table needs a `key` column (`Uuid`; the line holds any unique string, see [`crate::key_bytes`]);
//! - a `Ref(parent)` column holds the parent's key string; parents must come from the same run;
//! - `Enum`/`Set` values are strings (arrays of strings), appended to the dictionary in first-seen order (never reordered);
//! - `Json` takes any JSON value, `Bytes` is not accepted, unknown keys are ignored;
//! - with `prev`, rows keep their ids: a known key replaces its row (revived if it was removed), a new key is appended and a
//!   key missing from the input is tombstoned (row and key stay), so the result patches cleanly (`.coqp`) against `prev`.
use crate::key_bytes;
use coqc_format::coqd::row_bytes;
use coqc_format::dict::Dict;
use coqc_format::schema::{ColType, Rel, Schema, TableDef};
use coqc_format::table::{ColData, Store, Table};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Default, Clone, PartialEq)]
pub struct TableStats {
    pub table: String,
    pub lines: usize,
    pub live: usize,
    pub new: usize,
    pub changed: usize,
    pub revived: usize,
    pub tombstoned: usize,
}

type Key = [u8; 16];

fn col_value(cd: &mut ColData, ty: &ColType, v: Option<&Value>, schema: &Schema, dicts: &mut [Dict], live: &[HashMap<Key, usize>], name: &str) -> Result<(), String> {
    let v = v.filter(|v| !v.is_null());
    let bad = |what: &str| format!("{name}: expected {what}");
    match (cd, ty) {
        (ColData::Str(d), _) => d.push(match v {
            None => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return Err(bad("a string")),
        }),
        (ColData::Int(d), _) => d.push(match v {
            None => None,
            Some(Value::Number(n)) => Some(n.as_i64().or_else(|| n.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 9e15).map(|f| f as i64)).ok_or_else(|| bad("an integer"))?),
            Some(_) => return Err(bad("an integer")),
        }),
        (ColData::Float(d), _) => d.push(match v {
            None => None,
            Some(Value::Number(n)) => n.as_f64(),
            Some(_) => return Err(bad("a number")),
        }),
        (ColData::Bool(d), _) => d.push(match v {
            None => None,
            Some(Value::Bool(b)) => Some(*b),
            Some(_) => return Err(bad("true or false")),
        }),
        (ColData::Uuid(d), _) => match v {
            Some(Value::String(s)) if !s.is_empty() => d.push(key_bytes(s)),
            _ => return Err(bad("a non-empty string (key)")),
        },
        (ColData::Enum(d), ColType::Enum(dn)) => {
            let di = schema.dict(dn).ok_or_else(|| bad("a known dictionary"))?;
            d.push(match v {
                None => None,
                Some(Value::String(s)) => Some(dicts[di].push(s)),
                Some(_) => return Err(bad("a string")),
            })
        }
        (ColData::Set(d), ColType::Set(dn)) => {
            let di = schema.dict(dn).ok_or_else(|| bad("a known dictionary"))?;
            d.push(match v {
                None => None,
                Some(Value::Array(a)) => {
                    let mut ids = Vec::new();
                    for x in a {
                        ids.push(dicts[di].push(x.as_str().ok_or_else(|| bad("an array of strings"))?));
                    }
                    ids.sort_unstable();
                    ids.dedup();
                    Some(ids)
                }
                Some(_) => return Err(bad("an array of strings")),
            })
        }
        (ColData::Ref(d), ColType::Ref(p)) => {
            let pi = schema.table(p).ok_or_else(|| bad("a known parent table"))?;
            let Some(Value::String(s)) = v else { return Err(bad("the parent's key (a string)")) };
            let row = live[pi].get(&key_bytes(s)).ok_or_else(|| format!("{name}: no live {p} with key {s:?}"))?;
            d.push(*row as u32)
        }
        (ColData::Bytes(d), _) => match v {
            None => d.push(None),
            Some(_) => return Err(bad("nothing (Bytes columns cannot be ingested)")),
        },
        (ColData::Json(d), _) => d.push(v.map(|x| x.to_string())),
        _ => return Err(format!("{name}: column data does not match its type")),
    }
    Ok(())
}

fn key_col(def: &TableDef) -> Result<usize, String> {
    def.key.as_ref().and_then(|k| def.columns.iter().position(|c| &c.name == k)).ok_or_else(|| format!("table {} has no key column; generic ingest needs one", def.name))
}

/// Compile `inputs` (`(table name, JSONL text)`, one entry per table) into a store; see the module docs.
pub fn compile(schema: &Schema, inputs: &[(String, String)], prev: Option<&Store>, rebase: bool, source_date: u32) -> Result<(Store, Vec<TableStats>), String> {
    schema.validate()?;
    if schema.tables.iter().any(|t| t.columnar || matches!(t.rel, Rel::Sidecar(_))) {
        return Err("columnar and sidecar tables are not supported by dataset ingest (overlays: compile_overlay)".into());
    }
    let reuse = prev.filter(|p| !rebase && p.schema == *schema);
    if prev.is_some() && !rebase && reuse.is_none() {
        return Err("the schema differs from the previous dataset; use --rebase (new epoch)".into());
    }
    let mut st = reuse.map_or_else(|| Store::new(schema.clone()), Clone::clone);
    st.epoch = match prev {
        None => 1,
        Some(p) if rebase => p.epoch + 1,
        Some(p) => p.epoch,
    };
    st.source_date = source_date;
    let mut live: Vec<HashMap<Key, usize>> = vec![HashMap::new(); schema.tables.len()];
    let mut stats = vec![];
    for (ti, def) in schema.tables.iter().enumerate() {
        let text = &inputs.iter().find(|(n, _)| *n == def.name).ok_or_else(|| format!("no input for table {}", def.name))?.1;
        let kc = key_col(def)?;
        let mut by_key: HashMap<Key, usize> = HashMap::new();
        if let ColData::Uuid(ids) = &st.tables[ti].cols[kc] {
            ids.iter().enumerate().for_each(|(i, k)| {
                by_key.insert(*k, i);
            });
        }
        let existing = st.tables[ti].rows();
        let mut seen = vec![false; existing];
        let mut dup: HashSet<Key> = HashSet::new();
        let mut s = TableStats { table: def.name.clone(), ..Default::default() };
        for (n, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
            let at = |e: String| format!("{} line {}: {e}", def.name, n + 1);
            s.lines += 1;
            let obj: Value = serde_json::from_str(line).map_err(|e| at(e.to_string()))?;
            let obj = obj.as_object().ok_or_else(|| at("not a JSON object".into()))?;
            let mut cand = Table::new(def);
            cand.tomb.push(false);
            for (c, cd) in def.columns.iter().zip(cand.cols.iter_mut()) {
                col_value(cd, &c.ty, obj.get(&c.name), schema, &mut st.dicts, &live, &format!("{}.{}", def.name, c.name)).map_err(at)?;
            }
            let ColData::Uuid(kv) = &cand.cols[kc] else { unreachable!() };
            let key = kv[0];
            if !dup.insert(key) {
                return Err(at("duplicate key".into()));
            }
            match by_key.get(&key) {
                Some(&row) => {
                    let was_dead = st.tables[ti].tomb[row];
                    if was_dead {
                        s.revived += 1;
                    } else if row_bytes(def, &cand, 0) != row_bytes(def, &st.tables[ti], row) {
                        s.changed += 1;
                    }
                    st.tables[ti].set_row_from(row, &cand, 0);
                    seen[row] = true;
                }
                None => {
                    let row = st.tables[ti].rows();
                    st.tables[ti].set_row_from(row, &cand, 0);
                    by_key.insert(key, row);
                    s.new += 1;
                }
            }
            live[ti].insert(key, by_key[&key]);
        }
        for row in 0..existing {
            if seen[row] || st.tables[ti].tomb[row] {
                continue;
            }
            let ColData::Uuid(ids) = &st.tables[ti].cols[kc] else { unreachable!() };
            let key = ids[row];
            let mut dead = Table::new(def);
            dead.push_default();
            if let ColData::Uuid(v) = &mut dead.cols[kc] {
                v[0] = key;
            }
            st.tables[ti].set_row_from(row, &dead, 0);
            s.tombstoned += 1;
        }
        s.live = st.tables[ti].tomb.iter().filter(|t| !**t).count();
        stats.push(s);
    }
    st.check()?;
    Ok((st, stats))
}

#[derive(Debug, Default, PartialEq)]
pub struct OverlayStats {
    pub lines: usize,
    pub rows: usize,
    pub unknown_keys: usize,
    pub values: usize,
}

/// Compile an overlay: `schema` has exactly one columnar table of `Int` columns; row i follows row i of `base`'s primary table
/// (props `overlay_of`). Each JSONL line is `{"key": <primary key string>, <column>: <integer>, ...}`; a key the base does not
/// hold alive is counted and skipped, a missing column stays empty. The overlay carries the base's epoch.
pub fn compile_overlay(schema: &Schema, base: &Store, jsonl: &str, source_date: u32) -> Result<(Store, OverlayStats), String> {
    schema.validate()?;
    let [def] = schema.tables.as_slice() else { return Err("an overlay schema has exactly one table".into()) };
    if !def.columnar {
        return Err("the overlay table must be columnar (Int columns only, no key)".into());
    }
    let p = crate::primary_table(&base.schema);
    let pdef = &base.schema.tables[p];
    let ColData::Uuid(keys) = &base.tables[p].cols[key_col(pdef)?] else { return Err("the primary key column is not a Uuid".into()) };
    let rows = base.tables[p].rows();
    let by_key: HashMap<Key, usize> = keys.iter().enumerate().filter(|(i, _)| !base.tables[p].tomb[*i]).map(|(i, k)| (*k, i)).collect();
    let mut st = Store::new(schema.clone());
    st.epoch = base.epoch;
    st.source_date = source_date;
    st.props = vec![("overlay_of".into(), vec![pdef.name.clone()])];
    let mut cols: Vec<Vec<Option<i64>>> = vec![vec![None; rows]; def.columns.len()];
    let mut s = OverlayStats { rows, ..Default::default() };
    let mut seen: HashSet<usize> = HashSet::new();
    for (n, line) in jsonl.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
        let at = |e: String| format!("overlay line {}: {e}", n + 1);
        s.lines += 1;
        let obj: Value = serde_json::from_str(line).map_err(|e| at(e.to_string()))?;
        let key = obj.get("key").and_then(Value::as_str).ok_or_else(|| at("missing \"key\"".into()))?;
        let Some(&row) = by_key.get(&key_bytes(key)) else {
            s.unknown_keys += 1;
            continue;
        };
        if !seen.insert(row) {
            return Err(at(format!("duplicate key {key:?}")));
        }
        for (ci, c) in def.columns.iter().enumerate() {
            match obj.get(&c.name) {
                None | Some(Value::Null) => {}
                Some(Value::Number(x)) => {
                    cols[ci][row] = Some(x.as_i64().ok_or_else(|| at(format!("{}: expected an integer", c.name)))?);
                    s.values += 1;
                }
                Some(_) => return Err(at(format!("{}: expected an integer", c.name))),
            }
        }
    }
    st.tables[0].tomb = vec![false; rows];
    st.tables[0].cols = cols.into_iter().map(ColData::Int).collect();
    st.check()?;
    Ok((st, s))
}
