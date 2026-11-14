//! On-device engine over a COQD dataset + COQI search index.
//!
//! API (see docs/engine.md): `open`, `search`, `getRecords` (= `getCards`), `getByKey` (= `getByScryfallId`), `getByKeys`, `rowsOf`, `registerSet`, `registerRows`, `dropSet`, `saveSets`, `loadSets`,
//! `applyPatch`, `applyIndexDelta`, `attachOverlay`, `detachOverlay`, `getPrices`, `info`. `Engine::call` is the JSON dispatcher the C ABI (`ffi`) and the Expo module use.
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
    /// Timing of the last `open_with` phases (0.0.10-2 diagnostics), e.g. `read 40, decode 2100, ...` in ms; empty when unknown.
    open_phases: String,
    /// the primary table (the one with a parent) and its parent's name
    primary: usize,
    parent: Option<String>,
    overlays: Vec<(String, Overlay)>,
}

/// An attached overlay dataset (e.g. `prices`): row i of its first table belongs to row i of the primary table.
struct Overlay {
    store: Store,
    sha: [u8; 32],
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

/// The dialect for a schema id: `mtg` -> Magic, everything else -> the generic schema-driven dialect.
pub fn dialect_for(schema_id: &str) -> Box<dyn Dialect> {
    match schema_id {
        "mtg" => Box::new(coqc_mtg::Mtg),
        _ => Box::new(coqc_generic::Generic),
    }
}

type Loaded = (Store, [u8; 32], HashMap<Vec<u8>, u32>, String);

fn load_dataset(path: &Path, dialect: &dyn Dialect) -> Result<Loaded, String> {
    let ms = |t: std::time::Instant| t.elapsed().as_millis();
    let t = std::time::Instant::now();
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let t_read = ms(t);
    let t = std::time::Instant::now();
    let st = coqd::read_store(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let t_decode = ms(t);
    let t = std::time::Instant::now();
    let by_id = dialect.key_map(&st);
    let t_keys = ms(t);
    let t = std::time::Instant::now();
    let sha: [u8; 32] = Sha256::digest(&bytes).into();
    let t_sha = ms(t);
    Ok((st, sha, by_id, format!("coqd read {t_read}, decode {t_decode}, keys {t_keys}, sha {t_sha}")))
}

fn str_list(a: &Value, k: &str) -> Result<Vec<String>, String> {
    Ok(a.get(k).and_then(Value::as_array).ok_or(format!("missing {k}"))?.iter().filter_map(|v| v.as_str().map(String::from)).collect())
}

impl Engine {
    /// Open a dataset and (optionally) its `.coqi`. A missing, unreadable or mismatching `.coqi` is rebuilt.
    /// The dialect is picked from the dataset's schema id: `mtg` is the Magic dialect, any other schema is served by the
    /// schema-driven generic dialect ([`dialect_for`]).
    pub fn open(coqd: &Path, coqi_path: Option<&Path>) -> Result<Engine, String> {
        // The whole file is read only to learn the schema id; free it BEFORE `open_with` reads it again (it used to stay alive
        // for the whole open, i.e. two copies of the compressed dataset on top of the decoded store).
        let id = {
            let bytes = std::fs::read(coqd).map_err(|e| format!("{}: {e}", coqd.display()))?;
            let id = coqd::parse(&bytes).and_then(|p| p.schema()).map_err(|e| format!("{}: {e}", coqd.display()))?.id;
            id
        };
        Self::open_with(coqd, coqi_path, dialect_for(&id))
    }

    /// Like `open` with an explicit dialect (query syntax, derived columns, record JSON, key lookup).
    pub fn open_with(coqd: &Path, coqi_path: Option<&Path>, dialect: Box<dyn Dialect>) -> Result<Engine, String> {
        let (store, dsha, by_id, mut phases) = load_dataset(coqd, dialect.as_ref())?;
        let primary = store.schema.tables.iter().position(|t| matches!(t.rel, Rel::Parent(_))).unwrap_or(0);
        let parent = match &store.schema.tables[primary].rel {
            Rel::Parent(p) => Some(p.clone()),
            _ => None,
        };
        let mut e = Engine {
            dialect,
            coqd_path: coqd.to_path_buf(),
            coqi_path: coqi_path.map(Path::to_path_buf),
            store, dsha, parts: None, by_id, sets: HashMap::new(), index_source: "none", open_phases: String::new(), primary, parent, overlays: Vec::new(),
        };
        if let Some(p) = &e.coqi_path {
            let t = std::time::Instant::now();
            if let Ok(b) = std::fs::read(p) {
                let t_read = t.elapsed().as_millis();
                let t = std::time::Instant::now();
                if let Ok((parts, sha)) = coqi::deserialize(&b, &e.store.schema) {
                    let t_des = t.elapsed().as_millis();
                    let t = std::time::Instant::now();
                    // attaching validates every column against the dataset's table sizes
                    if sha == e.dsha && GIndex::with_parts(&e.store, Some(e.dialect.hooks()), &parts).is_ok() {
                        phases.push_str(&format!(", coqi read {t_read}, deserialize {t_des}, attach {}", t.elapsed().as_millis()));
                        e.parts = Some(parts);
                        e.index_source = "coqi";
                    }
                }
            }
        }
        e.open_phases = phases;
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
        self.ensure_index()?;
        let ov: Vec<(String, &Store)> = self.overlays.iter().map(|(id, o)| (id.clone(), &o.store)).collect();
        let mut q = self.dialect.parse_gquery(query, &self.store, &ov)?;
        q.limit = limit;
        q.offset = offset;
        if let Some(name) = scope {
            q.scope = Some(self.sets.get(name).cloned().ok_or_else(|| format!("unknown set '{name}'"))?);
        }
        let ix = GIndex::with_parts(&self.store, Some(self.dialect.hooks()), self.parts.as_ref().unwrap())?.with_overlays(ov);
        let r = ix.search(&q)?;
        Ok(Page { total: r.total, rows: r.rows })
    }

    /// Attach (or replace) an overlay dataset from a `.coqd` file. It must belong to the open dataset's epoch, and its first table
    /// must follow the primary table (`overlay_of` prop) or the sync layer has given the engine the wrong file.
    pub fn attach_overlay(&mut self, id: &str, path: &Path) -> Result<(), String> {
        if id.is_empty() {
            return Err("missing overlay id".into());
        }
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let store = coqd::read_store(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        if store.epoch != self.store.epoch {
            return Err(format!("overlay {id} is for card epoch {}, the open dataset has {}", store.epoch, self.store.epoch));
        }
        if store.tables.is_empty() {
            return Err(format!("overlay {id} has no table"));
        }
        let o = Overlay { store, sha: Sha256::digest(&bytes).into() };
        match self.overlays.iter_mut().find(|(i, _)| i == id) {
            Some(slot) => slot.1 = o,
            None => self.overlays.push((id.to_string(), o)),
        }
        Ok(())
    }


    /// Query hashes of an image file (the photo-side hashes of `coqc_phash::query_hashes`: 9 shifted inner-80% pHashes). With `card`
    /// the file is a straightened whole-card image and the art box is cut out first.
    pub fn hash_image(&self, path: &Path, card: bool) -> Result<Vec<u64>, String> {
        let g = coqc_phash::load_gray(path)?;
        Ok(coqc_phash::query_hashes(&if card { coqc_phash::card_art_region(&g) } else { g }))
    }

    /// Rank candidate printing rows by art distance to a photo, using the attached `arthash` overlay (docs/phash-design.md).
    /// `q` are the photo's query hashes; a candidate's distance is the smallest Hamming distance to any of them. Rows without a hash
    /// (no art, unknown row) come back in `unhashed`. `confident` is a tie-break guard: the best distance is at most `max_distance`
    /// and the best art beats the next-best DISTINCT art by at least `margin` bits (a single distinct art is confident by distance
    /// alone, but then there is nothing to decide, so `decisive` is false). Printings sharing one art hash tie by construction.
    pub fn match_art(&self, rows: &[u32], q: &[u64], max_distance: u32, margin: u32) -> Result<Value, String> {
        let (_, o) = self.overlays.iter().find(|(i, _)| i == "arthash").ok_or("arthash overlay not attached (attachOverlay id \"arthash\")")?;
        let (def, t) = o.store.table("art").ok_or("arthash overlay has no art table")?;
        let col = |name: &str| def.columns.iter().position(|c| c.name == name).ok_or_else(|| format!("arthash overlay lacks {name}"));
        let (coqc_format::table::ColData::Int(hi), coqc_format::table::ColData::Int(lo)) = (&t.cols[col("phash_hi")?], &t.cols[col("phash_lo")?]) else {
            return Err("bad arthash column".into());
        };
        let live = &self.store.tables[self.primary].tomb;
        let (mut ranked, mut unhashed): (Vec<(u32, u32, u64)>, Vec<u32>) = (Vec::new(), Vec::new());
        for &r in rows {
            let h = (r as usize) < hi.len() && !live.get(r as usize).is_none_or(|t| *t);
            match h.then(|| (hi[r as usize], lo[r as usize])) {
                Some((Some(h), Some(l))) => {
                    let hash = ((h as u64) << 32) | (l as u64 & 0xffff_ffff);
                    ranked.push((q.iter().map(|&x| coqc_phash::hamming(x, hash)).min().unwrap_or(64), r, hash));
                }
                _ => unhashed.push(r),
            }
        }
        ranked.sort();
        let best = ranked.first().map(|x| x.0);
        let next_art = ranked.iter().find(|x| Some(x.2) != ranked.first().map(|f| f.2)).map(|x| x.0);
        let margin_bits = match (best, next_art) {
            (Some(b), Some(n)) => Some(n - b),
            _ => None,
        };
        let decisive = next_art.is_some();
        let confident = best.is_some_and(|b| b <= max_distance) && margin_bits.is_none_or(|m| m >= margin);
        Ok(json!({
            "ranked": ranked.iter().map(|(d, r, _)| json!({"row": r, "distance": d})).collect::<Vec<_>>(),
            "unhashed": unhashed,
            "best": best,
            "margin": margin_bits,
            "decisive": decisive,
            "confident": confident && decisive,
        }))
    }

    /// MTGJSON prices of printing rows from the attached `prices` overlay, Scryfall-shaped (`usd`, `usd_foil`, `usd_etched`, `eur`,
    /// `eur_foil`, `eur_etched`: strings with two decimals, null = no price). `usd*` is TCGplayer retail, `eur*` Cardmarket retail.
    /// Row i of the result belongs to `rows[i]`; an unknown or removed row gives null.
    pub fn prices(&self, rows: &[u32]) -> Result<Value, String> {
        let (_, o) = self.overlays.iter().find(|(i, _)| i == "prices").ok_or("prices overlay not attached (attachOverlay)")?;
        let (def, t) = o.store.table("price").ok_or("prices overlay has no price table")?;
        let col = |name: &str| def.columns.iter().position(|c| c.name == name).ok_or_else(|| format!("prices overlay lacks {name}"));
        let keys = [("usd", "tcg_normal"), ("usd_foil", "tcg_foil"), ("usd_etched", "tcg_etched"), ("eur", "cm_normal"), ("eur_foil", "cm_foil"), ("eur_etched", "cm_etched")];
        let mut cols = Vec::new();
        for (k, c) in keys {
            match &t.cols[col(c)?] {
                coqc_format::table::ColData::Int(v) => cols.push((k, v)),
                _ => return Err("bad price column".into()),
            }
        }
        let live = &self.store.tables[self.primary].tomb;
        Ok(Value::Array(
            rows.iter()
                .map(|&r| {
                    if live.get(r as usize).is_none_or(|t| *t) {
                        return Value::Null;
                    }
                    let mut m = serde_json::Map::new();
                    for (k, v) in &cols {
                        let cents = v.get(r as usize).copied().flatten();
                        m.insert(k.to_string(), cents.map_or(Value::Null, |c| json!(format!("{}.{:02}", c / 100, c % 100))));
                    }
                    Value::Object(m)
                })
                .collect(),
        ))
    }

    /// Set `prices` on record JSON objects (`records[i]` is row `rows[i]`); records without a row or price stay untouched.
    fn add_prices(&self, rows: &[u32], records: &mut [Value]) -> Result<(), String> {
        let p = self.prices(rows)?;
        for (rec, pr) in records.iter_mut().zip(p.as_array().into_iter().flatten()) {
            if let (Some(o), false) = (rec.as_object_mut(), pr.is_null()) {
                o.insert("prices".into(), pr.clone());
            }
        }
        Ok(())
    }

    pub fn detach_overlay(&mut self, id: &str) {
        self.overlays.retain(|(i, _)| i != id);
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

    /// Rows of many keys at once (None = unknown key), in the order given.
    pub fn rows_of(&self, keys: &[String]) -> Vec<Option<u32>> {
        keys.iter().map(|k| self.row_of(k)).collect()
    }

    /// Register (or replace) a named scope from rows (e.g. from `rows_of`); out-of-range or removed rows are dropped and counted.
    pub fn register_rows(&mut self, name: &str, rows: &[u32]) -> (usize, usize) {
        let t = &self.store.tables[self.primary];
        let mut v: Vec<u32> = rows.iter().copied().filter(|&r| t.tomb.get(r as usize).is_some_and(|d| !*d)).collect();
        v.sort_unstable();
        v.dedup();
        let resolved = v.len();
        self.sets.insert(name.to_string(), v);
        (resolved, rows.len() - resolved)
    }

    /// Persist the named scopes (rows are stable across patches, not across epochs, so the epoch is stored with them).
    pub fn save_sets(&self, path: &Path) -> Result<(), String> {
        let sets: serde_json::Map<String, Value> = self.sets.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        write_atomic(path, json!({"epoch": self.store.epoch, "sets": sets}).to_string().as_bytes())
    }

    /// Load scopes written by `save_sets`. A missing file, another epoch or unreadable content loads nothing (returns 0): the
    /// caller re-registers from its own data. Existing scopes of the same name are replaced. Rows removed since are dropped.
    pub fn load_sets(&mut self, path: &Path) -> usize {
        let Ok(b) = std::fs::read(path) else { return 0 };
        let Ok(v) = serde_json::from_slice::<Value>(&b) else { return 0 };
        if v.get("epoch").and_then(Value::as_u64) != Some(self.store.epoch as u64) {
            return 0;
        }
        let Some(m) = v.get("sets").and_then(Value::as_object) else { return 0 };
        let mut n = 0;
        for (k, rows) in m {
            let Some(a) = rows.as_array() else { continue };
            let rows: Vec<u32> = a.iter().filter_map(|r| r.as_u64().map(|r| r as u32)).collect();
            self.register_rows(k, &rows);
            n += 1;
        }
        n
    }

    pub fn apply_patch(&mut self, patch_path: &Path) -> Result<(), String> {
        let old = std::fs::read(&self.coqd_path).map_err(|e| e.to_string())?;
        let p = std::fs::read(patch_path).map_err(|e| format!("{}: {e}", patch_path.display()))?;
        let new = patch::apply_patch(&old, &p)?;
        write_atomic(&self.coqd_path, &new)?;
        let (store, dsha, by_id, _) = load_dataset(&self.coqd_path, self.dialect.as_ref())?;
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
            "openPhases": self.open_phases,
            "overlays": self.overlays.iter().map(|(k, o)| (k.clone(), json!({"sha256": hex(&o.sha), "rows": o.store.tables[0].rows(), "sourceDate": o.store.source_date}))).collect::<serde_json::Map<_, _>>(),
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
                    let mut cards: Vec<Value> = page.rows.iter().filter_map(|&r| self.card(r)).collect();
                    if a.get("prices").and_then(Value::as_bool) == Some(true) {
                        self.add_prices(&page.rows, &mut cards)?;
                    }
                    out["cards"] = Value::Array(cards);
                }
                Ok(out)
            }
            "attachOverlay" => {
                self.attach_overlay(s("id").ok_or("missing id")?, Path::new(s("path").ok_or("missing path")?))?;
                Ok(self.info())
            }
            "detachOverlay" => {
                self.detach_overlay(s("id").ok_or("missing id")?);
                Ok(self.info())
            }
            "getCards" | "getRecords" => {
                let rows = a.get("rows").and_then(Value::as_array).ok_or("missing rows")?;
                let rows: Vec<u32> = rows.iter().map(|r| r.as_u64().map_or(u32::MAX, |r| r as u32)).collect();
                let mut out: Vec<Value> = rows.iter().map(|&r| self.card(r).unwrap_or(Value::Null)).collect();
                if a.get("prices").and_then(Value::as_bool) == Some(true) {
                    self.add_prices(&rows, &mut out)?;
                }
                Ok(Value::Array(out))
            }
            "hashImage" => {
                let h = self.hash_image(Path::new(s("path").ok_or("missing path")?), a.get("card").and_then(Value::as_bool) == Some(true))?;
                Ok(json!({"hashes": h.iter().map(|x| format!("{x:016x}")).collect::<Vec<_>>()}))
            }
            "matchArtHash" => {
                let rows: Vec<u32> = a.get("rows").and_then(Value::as_array).ok_or("missing rows")?.iter().filter_map(|v| v.as_u64().map(|r| r as u32)).collect();
                let q: Vec<u64> = match a.get("hashes").and_then(Value::as_array) {
                    Some(hs) => hs.iter().map(|h| h.as_str().and_then(|h| u64::from_str_radix(h, 16).ok()).ok_or("bad hash (16 hex digits)")).collect::<Result<_, _>>()?,
                    None => self.hash_image(Path::new(s("path").ok_or("give hashes or path")?), a.get("card").and_then(Value::as_bool) == Some(true))?,
                };
                let num = |k: &str, d: u32| a.get(k).and_then(Value::as_u64).map_or(d, |v| v as u32);
                self.match_art(&rows, &q, num("maxDistance", 24), num("margin", 2))
            }
            "getPrices" => {
                let rows = a.get("rows").and_then(Value::as_array).ok_or("missing rows")?;
                let rows: Vec<u32> = rows.iter().map(|r| r.as_u64().map_or(u32::MAX, |r| r as u32)).collect();
                self.prices(&rows)
            }
            "getByScryfallId" | "getByKey" => {
                let id = s("id").or_else(|| s("key")).ok_or("missing id")?;
                Ok(self.row_of(id).and_then(|r| self.card(r)).unwrap_or(Value::Null))
            }
            "rowsOf" => {
                let keys = str_list(a, "ids").or_else(|_| str_list(a, "keys"))?;
                Ok(Value::Array(self.rows_of(&keys).into_iter().map(|r| r.map_or(Value::Null, |r| json!(r))).collect()))
            }
            "getByKeys" | "getByScryfallIds" => {
                let keys = str_list(a, "ids").or_else(|_| str_list(a, "keys"))?;
                let rows = self.rows_of(&keys);
                let mut out: Vec<Value> = rows.iter().map(|r| r.and_then(|r| self.card(r)).unwrap_or(Value::Null)).collect();
                if a.get("prices").and_then(Value::as_bool) == Some(true) {
                    let rr: Vec<u32> = rows.iter().map(|r| r.unwrap_or(u32::MAX)).collect();
                    self.add_prices(&rr, &mut out)?;
                }
                Ok(Value::Array(out))
            }
            "registerRows" => {
                let rows: Vec<u32> = a.get("rows").and_then(Value::as_array).ok_or("missing rows")?.iter().filter_map(|v| v.as_u64().map(|r| r as u32)).collect();
                let (resolved, unknown) = self.register_rows(s("name").ok_or("missing name")?, &rows);
                Ok(json!({"resolved": resolved, "unknown": unknown}))
            }
            "dropSet" => {
                self.sets.remove(s("name").ok_or("missing name")?);
                Ok(self.info())
            }
            "saveSets" => self.save_sets(Path::new(s("path").ok_or("missing path")?)).map(|_| Value::Null),
            "loadSets" => Ok(json!({"loaded": self.load_sets(Path::new(s("path").ok_or("missing path")?))})),
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
