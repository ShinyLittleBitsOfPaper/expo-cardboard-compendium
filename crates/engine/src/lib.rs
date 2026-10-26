//! On-device engine over a COQD dataset + COQS search index.
//!
//! API (see docs/engine.md): `open`, `search`, `getCards`, `getByScryfallId`, `registerSet`, `applyPatch`,
//! `applyIndexDelta`, `info`. `Engine::call` is the JSON dispatcher the C ABI (`ffi`) and the Expo module use.
//! Phase 1: the dataset is decoded into memory (owned); the index is loaded from the `.coqs` when its
//! dataset hash matches, otherwise rebuilt (and the `.coqs` rewritten).

pub mod ffi;

use coqc_format::json::{card_json, parse_uuid};
use coqc_format::search::{coqs, SearchIndex};
use coqc_format::{patch, read, Dataset};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Engine {
    coqd_path: PathBuf,
    coqs_path: Option<PathBuf>,
    ds: Dataset,
    dsha: [u8; 32],
    /// None = stale (dataset changed since the index was made); rebuilt on the next search
    ix: Option<SearchIndex>,
    by_id: HashMap<[u8; 16], u32>,
    sets: HashMap<String, Vec<u32>>,
    index_source: &'static str,
}

pub struct Page {
    pub total: usize,
    pub rows: Vec<u32>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

fn load_dataset(path: &Path) -> Result<(Dataset, [u8; 32], HashMap<[u8; 16], u32>), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let ds = read(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let by_id = ds.printings.iter().enumerate().filter(|(_, p)| !p.tombstone).map(|(i, p)| (p.scryfall_id, i as u32)).collect();
    Ok((ds, Sha256::digest(&bytes).into(), by_id))
}

impl Engine {
    /// Open a dataset and (optionally) its `.coqs`. A missing, unreadable or mismatching `.coqs` is rebuilt.
    pub fn open(coqd: &Path, coqs_path: Option<&Path>) -> Result<Engine, String> {
        let (ds, dsha, by_id) = load_dataset(coqd)?;
        let mut e = Engine {
            coqd_path: coqd.to_path_buf(), coqs_path: coqs_path.map(Path::to_path_buf),
            ds, dsha, ix: None, by_id, sets: HashMap::new(), index_source: "none",
        };
        if let Some(p) = &e.coqs_path {
            if let Ok(b) = std::fs::read(p) {
                if let Ok((ix, sha)) = coqs::deserialize(&b) {
                    if sha == e.dsha {
                        e.ix = Some(ix);
                        e.index_source = "coqs";
                    }
                }
            }
        }
        e.ensure_index()?;
        Ok(e)
    }

    fn ensure_index(&mut self) -> Result<(), String> {
        if self.ix.is_none() {
            let ix = SearchIndex::build(&self.ds);
            if let Some(p) = &self.coqs_path {
                // best effort: a failed write only costs a rebuild next launch
                let _ = write_atomic(p, &coqs::serialize(&ix, &self.dsha));
            }
            self.ix = Some(ix);
            self.index_source = "built";
        }
        Ok(())
    }

    pub fn search(&mut self, query: &str, limit: Option<usize>, offset: usize, scope: Option<&str>) -> Result<Page, String> {
        let mut q = coqc_format::parse::parse(query).map_err(|e| e.0)?;
        q.limit = limit;
        q.offset = offset;
        if let Some(name) = scope {
            q.scope = Some(self.sets.get(name).cloned().ok_or_else(|| format!("unknown set '{name}'"))?);
        }
        self.ensure_index()?;
        let r = self.ix.as_ref().unwrap().search(&q);
        Ok(Page { total: r.total, rows: r.rows })
    }

    pub fn card(&self, row: u32) -> Option<Value> {
        let p = self.ds.printings.get(row as usize).filter(|p| !p.tombstone)?;
        let o = self.ds.oracle.get(p.oracle_row as usize)?;
        Some(card_json(&self.ds.meta, o, &self.ds.ranks[p.oracle_row as usize], p))
    }

    pub fn row_of(&self, scryfall_id: &str) -> Option<u32> {
        self.by_id.get(&parse_uuid(scryfall_id)?).copied()
    }

    /// Register (or replace) a named scope from Scryfall ids. Returns (resolved, unknown).
    pub fn register_set(&mut self, name: &str, ids: &[String]) -> (usize, usize) {
        let mut rows: Vec<u32> = ids.iter().filter_map(|s| self.row_of(s)).collect();
        rows.sort_unstable();
        rows.dedup();
        let resolved = rows.len();
        self.sets.insert(name.to_string(), rows);
        (resolved, ids.len() - resolved)
    }

    pub fn apply_patch(&mut self, patch_path: &Path) -> Result<(), String> {
        let old = std::fs::read(&self.coqd_path).map_err(|e| e.to_string())?;
        let p = std::fs::read(patch_path).map_err(|e| format!("{}: {e}", patch_path.display()))?;
        let new = patch::apply_patch(&old, &p)?;
        write_atomic(&self.coqd_path, &new)?;
        let (ds, dsha, by_id) = load_dataset(&self.coqd_path)?;
        (self.ds, self.dsha, self.by_id) = (ds, dsha, by_id);
        self.ix = None; // stale until an index delta is applied or the next search rebuilds
        self.index_source = "stale";
        Ok(())
    }

    /// Apply a `.coqs` delta (after `apply_patch`). On any failure the index stays stale and is rebuilt on demand.
    pub fn apply_index_delta(&mut self, delta_path: &Path) -> Result<(), String> {
        let p = self.coqs_path.clone().ok_or("no .coqs path configured")?;
        let old = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let d = std::fs::read(delta_path).map_err(|e| format!("{}: {e}", delta_path.display()))?;
        let new = coqs::apply_delta(&old, &d).map_err(|e| e.to_string())?;
        let (ix, sha) = coqs::deserialize(&new).map_err(|e| e.to_string())?;
        if sha != self.dsha {
            return Err("index delta targets a different dataset (apply the dataset patch first)".into());
        }
        write_atomic(&p, &new)?;
        self.ix = Some(ix);
        self.index_source = "delta";
        Ok(())
    }

    pub fn info(&self) -> Value {
        json!({
            "printings": self.ds.printings.iter().filter(|p| !p.tombstone).count(),
            "oracleCards": self.ds.oracle.iter().filter(|o| !o.tombstone).count(),
            "datasetSha256": hex(&self.dsha),
            "index": self.index_source,
            "sets": self.sets.iter().map(|(k, v)| (k.clone(), json!(v.len()))).collect::<serde_json::Map<_, _>>(),
        })
    }

    /// JSON dispatcher: `method` + JSON `args` -> JSON result.
    pub fn call(&mut self, method: &str, a: &Value) -> Result<Value, String> {
        let s = |k: &str| a.get(k).and_then(Value::as_str);
        match method {
            "info" => Ok(self.info()),
            "search" => {
                let query = s("query").ok_or("missing query")?;
                let limit = a.get("limit").and_then(Value::as_u64).map(|v| v as usize);
                let offset = a.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
                let page = self.search(query, limit, offset, s("scope"))?;
                let mut out = json!({"total": page.total, "rows": page.rows});
                if a.get("cards").and_then(Value::as_bool) == Some(true) {
                    out["cards"] = Value::Array(page.rows.iter().filter_map(|&r| self.card(r)).collect());
                }
                Ok(out)
            }
            "getCards" => {
                let rows = a.get("rows").and_then(Value::as_array).ok_or("missing rows")?;
                Ok(Value::Array(rows.iter().map(|r| r.as_u64().and_then(|r| self.card(r as u32)).unwrap_or(Value::Null)).collect()))
            }
            "getByScryfallId" => {
                let id = s("id").ok_or("missing id")?;
                Ok(self.row_of(id).and_then(|r| self.card(r)).unwrap_or(Value::Null))
            }
            "registerSet" => {
                let ids: Vec<String> = a.get("ids").and_then(Value::as_array).ok_or("missing ids")?.iter().filter_map(|v| v.as_str().map(String::from)).collect();
                let (resolved, unknown) = self.register_set(s("name").ok_or("missing name")?, &ids);
                Ok(json!({"resolved": resolved, "unknown": unknown}))
            }
            "applyPatch" => self.apply_patch(Path::new(s("path").ok_or("missing path")?)).map(|_| self.info()),
            "applyIndexDelta" => self.apply_index_delta(Path::new(s("path").ok_or("missing path")?)).map(|_| self.info()),
            _ => Err(format!("unknown method '{method}'")),
        }
    }
}
