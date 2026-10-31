//! On-device engine over a COQD dataset + COQI search index.
//!
//! API (see docs/engine.md): `open`, `search`, `getCards`, `getByScryfallId`, `registerSet`, `applyPatch`,
//! `applyIndexDelta`, `info`. `Engine::call` is the JSON dispatcher the C ABI (`ffi`) and the Expo module use.
//! The dataset is held as a columnar `Store` (no per-record structs); the index is loaded from the `.coqi` when its
//! dataset hash matches and its columns fit the tables, otherwise rebuilt (and the `.coqi` rewritten).

pub mod ffi;
pub mod files;

use coqc_format::dialect::Dialect;
use coqc_format::gindex::{GIndex, IndexParts};
use coqc_format::schema::Rel;
use coqc_format::search::{STOP_FRACTION, STOP_MIN};
use coqc_format::table::Store;
use coqc_format::{coqd, coqi, patch};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Engine {
    coqd_path: PathBuf,
    coqi_path: Option<PathBuf>,
    store: Store,
    dsha: [u8; 32],
    /// None = stale (dataset changed since the index was made); rebuilt on the next search
    parts: Option<IndexParts>,
    by_id: HashMap<Vec<u8>, u32>,
    dialect: Box<dyn Dialect>,
    sets: HashMap<String, Vec<u32>>,
    index_source: &'static str,
    /// the primary table (the one with a parent) and its parent's name
    primary: usize,
    parent: Option<String>,
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

type Loaded = (Store, [u8; 32], HashMap<Vec<u8>, u32>);

fn load_dataset(path: &Path, dialect: &dyn Dialect) -> Result<Loaded, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let st = coqd::read_store(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let by_id = dialect.key_map(&st);
    Ok((st, Sha256::digest(&bytes).into(), by_id))
}

impl Engine {
    /// Open a dataset and (optionally) its `.coqi`. A missing, unreadable or mismatching `.coqi` is rebuilt.
    pub fn open(coqd: &Path, coqi_path: Option<&Path>) -> Result<Engine, String> {
        Self::open_with(coqd, coqi_path, Box::new(coqc_mtg::Mtg))
    }

    /// Like `open` with an explicit dialect (query syntax, derived columns, record JSON, key lookup).
    pub fn open_with(coqd: &Path, coqi_path: Option<&Path>, dialect: Box<dyn Dialect>) -> Result<Engine, String> {
        let (store, dsha, by_id) = load_dataset(coqd, dialect.as_ref())?;
        let primary = store.schema.tables.iter().position(|t| matches!(t.rel, Rel::Parent(_))).unwrap_or(0);
        let parent = match &store.schema.tables[primary].rel {
            Rel::Parent(p) => Some(p.clone()),
            _ => None,
        };
        let mut e = Engine {
            dialect,
            coqd_path: coqd.to_path_buf(),
            coqi_path: coqi_path.map(Path::to_path_buf),
            store, dsha, parts: None, by_id, sets: HashMap::new(), index_source: "none", primary, parent,
        };
        if let Some(p) = &e.coqi_path {
            if let Ok(b) = std::fs::read(p) {
                if let Ok((parts, sha)) = coqi::deserialize(&b, &e.store.schema) {
                    // attaching validates every column against the dataset's table sizes
                    if sha == e.dsha && GIndex::with_parts(&e.store, Some(e.dialect.hooks()), &parts).is_ok() {
                        e.parts = Some(parts);
                        e.index_source = "coqi";
                    }
                }
            }
        }
        e.ensure_index()?;
        Ok(e)
    }

    fn ensure_index(&mut self) -> Result<(), String> {
        if self.parts.is_none() {
            let spec = self.dialect.index_spec(&self.store)?;
            let parts = GIndex::build_parts(&self.store, spec, STOP_FRACTION, STOP_MIN)?;
            if let Some(p) = &self.coqi_path {
                // best effort: a failed write only costs a rebuild next launch
                let _ = write_atomic(p, &coqi::serialize(&parts, &self.store.schema, &self.dsha));
            }
            self.parts = Some(parts);
            self.index_source = "built";
        }
        Ok(())
    }

    pub fn search(&mut self, query: &str, limit: Option<usize>, offset: usize, scope: Option<&str>) -> Result<Page, String> {
        let mut q = self.dialect.parse_gquery(query)?;
        q.limit = limit;
        q.offset = offset;
        if let Some(name) = scope {
            q.scope = Some(self.sets.get(name).cloned().ok_or_else(|| format!("unknown set '{name}'"))?);
        }
        self.ensure_index()?;
        let ix = GIndex::with_parts(&self.store, Some(self.dialect.hooks()), self.parts.as_ref().unwrap())?;
        let r = ix.search(&q)?;
        Ok(Page { total: r.total, rows: r.rows })
    }

    pub fn card(&self, row: u32) -> Option<Value> {
        let t = &self.store.tables[self.primary];
        (!*t.tomb.get(row as usize)?).then(|| self.dialect.record_json(&self.store, row))
    }

    pub fn row_of(&self, scryfall_id: &str) -> Option<u32> {
        self.by_id.get(&self.dialect.parse_key(scryfall_id)?).copied()
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
        let (store, dsha, by_id) = load_dataset(&self.coqd_path, self.dialect.as_ref())?;
        (self.store, self.dsha, self.by_id) = (store, dsha, by_id);
        self.parts = None; // stale until an index delta is applied or the next search rebuilds
        self.index_source = "stale";
        Ok(())
    }

    /// Apply a `.coqi` delta (after `apply_patch`). On any failure the index stays stale and is rebuilt on demand.
    pub fn apply_index_delta(&mut self, delta_path: &Path) -> Result<(), String> {
        let p = self.coqi_path.clone().ok_or("no .coqi path configured")?;
        let old = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let d = std::fs::read(delta_path).map_err(|e| format!("{}: {e}", delta_path.display()))?;
        let new = coqi::apply_delta(&old, &d).map_err(|e| e.to_string())?;
        let (parts, sha) = coqi::deserialize(&new, &self.store.schema).map_err(|e| e.to_string())?;
        if sha != self.dsha {
            return Err("index delta targets a different dataset (apply the dataset patch first)".into());
        }
        GIndex::with_parts(&self.store, Some(self.dialect.hooks()), &parts)?;
        write_atomic(&p, &new)?;
        self.parts = Some(parts);
        self.index_source = "delta";
        Ok(())
    }

    pub fn info(&self) -> Value {
        json!({
            "printings": self.store.tables[self.primary].tomb.iter().filter(|t| !**t).count(),
            "oracleCards": self.parent.as_deref().and_then(|p| self.store.table(p)).map_or(0, |(_, t)| t.tomb.iter().filter(|t| !**t).count()),
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
