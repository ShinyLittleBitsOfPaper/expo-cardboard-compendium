//! S3a: a generic in-memory index and query evaluator over a [`Store`] (docs/s3-design.md). Nothing here names a dataset: columns
//! come from the schema (`IndexKind` picks the structure) and the dialect adds *derived* columns (folded text, masks, numbers,
//! flags) and `Custom` predicates. The old `SearchIndex` stays until S3b/S3c; tests cross-check the two.
//!
//! Row space: queries are evaluated over the *primary* table (the one with `Rel::Parent`); a predicate on a parent table (or its
//! sidecar) is evaluated over the parent's rows and lifted through the primary's `Ref` column. Needles for `Text` and `Str`
//! comparisons are used exactly as given: the dialect folds them the same way it folded the derived column.
use crate::schema::{ColType, IndexKind, Rel};
use crate::search::{num_cmp, Arena, Bits, Op, Trigrams, STOP_FRACTION, STOP_MIN};
use crate::table::{ColData, Store};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

pub enum DData {
    Text(Vec<String>),
    Mask(Vec<u32>),
    Num(Vec<Option<f64>>),
    Flag(Vec<bool>),
}

/// A column the dialect computes at index-build time (one value per row of `table`, tombstoned rows included).
pub struct DerivedCol {
    pub table: String,
    pub name: String,
    pub data: DData,
    /// Text columns with the same group share one trigram index (built over their values joined by NUL); candidates are
    /// verified against each column's own text. `None` = no trigram index (scan).
    pub tri_group: Option<String>,
}

pub trait Hooks: Send + Sync {
    /// Evaluate a `Custom` predicate to a bitset over primary rows.
    fn custom(&self, ix: &GIndex, name: &str, args: &[String]) -> Result<Bits, String>;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Val {
    Num(f64),
    Mask(u32),
    Str(String),
}

/// Generic predicate tree. `:` on a mask means "includes" (`>=`); on a number it means equals; on a string, equals.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GExpr {
    All,
    None,
    And(Vec<GExpr>),
    Or(Vec<GExpr>),
    Not(Box<GExpr>),
    Cmp { field: String, op: Op, val: Val },
    /// popcount of a mask column compared with `n`
    Count { field: String, op: Op, n: f64 },
    /// the column has a value
    Present { field: String },
    /// substring of a text column (needle already folded by the caller)
    Text { field: String, needle: String },
    Custom { name: String, args: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GSort {
    pub field: String,
    #[serde(default)]
    pub desc: bool,
    /// value used for a missing number
    #[serde(default)]
    pub null: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GPrefer {
    pub field: String,
    pub newest: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GQuery {
    #[serde(default)]
    pub expr: Option<GExpr>,
    #[serde(default)]
    pub sort: Vec<GSort>,
    /// one primary row per parent row, chosen by `prefer` before sorting
    #[serde(default)]
    pub unique_parent: bool,
    #[serde(default)]
    pub prefer: Option<GPrefer>,
    /// flag column whose rows are removed from the result (the dialect's default filter)
    #[serde(default)]
    pub hide: Option<String>,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub scope: Option<Vec<u32>>,
}

#[derive(Clone, Debug, Default)]
pub struct GResult {
    pub total: usize,
    pub rows: Vec<u32>,
}

enum DKind {
    Text(Arena),
    Mask(Vec<u32>),
    Num(Vec<Option<f64>>),
    Flag(Vec<bool>),
}

struct Dcol {
    table: usize,
    name: String,
    kind: DKind,
    tri: Option<usize>,
}

#[derive(Clone, Copy)]
enum Src {
    Store(usize),
    Derived(usize),
}

#[derive(Clone, Copy)]
struct Field {
    table: usize,
    src: Src,
}

enum Acc<'b> {
    Num(Box<dyn Fn(usize) -> Option<f64> + 'b>),
    Mask(Box<dyn Fn(usize) -> Option<u32> + 'b>),
    Str(Box<dyn Fn(usize) -> Option<&'b str> + 'b>),
}

pub struct GIndex<'a> {
    store: &'a Store,
    hooks: Option<&'a dyn Hooks>,
    primary: usize,
    parent_ref: Option<usize>,
    dcols: Vec<Dcol>,
    tris: Vec<Trigrams>,
    tlive: Vec<Vec<bool>>,
    /// per table: primary row -> row of that table (None for the primary itself or when unreachable)
    maps: Vec<Option<&'a [u32]>>,
    live_p: Bits,
}

fn mask_cmp(a: u32, b: u32, op: Op) -> bool {
    match op {
        Op::Colon | Op::Ge => a & b == b,
        Op::Gt => a & b == b && a != b,
        Op::Eq => a == b,
        Op::Ne => a != b,
        Op::Le => a & b == a,
        Op::Lt => a & b == a && a != b,
    }
}

impl<'a> GIndex<'a> {
    pub fn build(store: &'a Store, derived: Vec<DerivedCol>, hooks: Option<&'a dyn Hooks>) -> Result<Self, String> {
        Self::build_with(store, derived, hooks, STOP_FRACTION, STOP_MIN)
    }

    pub fn build_with(store: &'a Store, derived: Vec<DerivedCol>, hooks: Option<&'a dyn Hooks>, stop_fraction: f64, stop_min: usize) -> Result<Self, String> {
        let sc = &store.schema;
        let tid = |n: &str| sc.table(n).ok_or_else(|| format!("unknown table {n}"));
        let primary = sc.tables.iter().position(|t| matches!(t.rel, Rel::Parent(_))).unwrap_or(0);
        // per-table liveness; a sidecar follows its target
        let mut tlive: Vec<Vec<bool>> = store.tables.iter().map(|t| t.tomb.iter().map(|x| !x).collect()).collect();
        for (i, t) in sc.tables.iter().enumerate() {
            if let Rel::Sidecar(p) = &t.rel {
                tlive[i] = tlive[tid(p)?].clone();
            }
        }
        // reachability: primary row -> row of table t
        let pt = &store.tables[primary];
        let ref_to = |target: &str| -> Option<(usize, &'a [u32])> {
            let pd = &sc.tables[primary];
            pd.columns.iter().enumerate().find_map(|(ci, c)| match (&c.ty, &store.tables[primary].cols[ci]) {
                (ColType::Ref(t), ColData::Ref(v)) if t == target => Some((ci, v.as_slice())),
                _ => None,
            })
        };
        let mut maps: Vec<Option<&'a [u32]>> = vec![None; sc.tables.len()];
        for (i, t) in sc.tables.iter().enumerate() {
            if i == primary {
                continue;
            }
            let target = match &t.rel {
                Rel::Sidecar(p) => p.as_str(),
                _ => t.name.as_str(),
            };
            maps[i] = ref_to(target).map(|x| x.1);
        }
        let parent_ref = match &sc.tables[primary].rel {
            Rel::Parent(p) => ref_to(p).map(|x| x.0),
            _ => None,
        };
        let mut live_p = Bits::new(pt.rows());
        for (i, &l) in tlive[primary].iter().enumerate() {
            if l {
                live_p.set(i);
            }
        }
        let mut ix = GIndex { store, hooks, primary, parent_ref, dcols: vec![], tris: vec![], tlive, maps, live_p };
        // dialect-derived columns first (they win name lookups), then automatic text columns from the schema
        let mut groups: Vec<(String, usize, Vec<usize>)> = vec![]; // (group, table, members)
        for d in derived {
            let table = tid(&d.table)?;
            let n = store.tables[table].rows();
            let (kind, len) = match d.data {
                DData::Text(v) => {
                    let len = v.len();
                    let mut a = Arena::default();
                    v.iter().for_each(|s| a.push(s));
                    (DKind::Text(a), len)
                }
                DData::Mask(v) => (DKind::Mask(v.clone()), v.len()),
                DData::Num(v) => (DKind::Num(v.clone()), v.len()),
                DData::Flag(v) => (DKind::Flag(v.clone()), v.len()),
            };
            if len != n {
                return Err(format!("derived column {}.{} has {len} rows, table has {n}", d.table, d.name));
            }
            let idx = ix.dcols.len();
            ix.dcols.push(Dcol { table, name: d.name, kind, tri: None });
            if let Some(g) = d.tri_group {
                match groups.iter_mut().find(|x| x.0 == g) {
                    Some(x) => {
                        if x.1 != table {
                            return Err(format!("trigram group {g} spans tables"));
                        }
                        x.2.push(idx)
                    }
                    None => groups.push((g, table, vec![idx])),
                }
            }
        }
        for (ti, t) in sc.tables.iter().enumerate() {
            for (ci, c) in t.columns.iter().enumerate() {
                if c.index == IndexKind::Text && c.ty == ColType::Str && !ix.dcols.iter().any(|d| d.table == ti && d.name == c.name) {
                    if let ColData::Str(v) = &store.tables[ti].cols[ci] {
                        let mut a = Arena::default();
                        v.iter().for_each(|s| a.push(&crate::search::fold(s.as_deref().unwrap_or(""))));
                        let idx = ix.dcols.len();
                        ix.dcols.push(Dcol { table: ti, name: c.name.clone(), kind: DKind::Text(a), tri: None });
                        groups.push((format!("\u{1}{ti}.{}", c.name), ti, vec![idx]));
                    }
                }
            }
        }
        for (_, table, members) in groups {
            let n = store.tables[table].rows();
            let mut both = Arena::default();
            for i in 0..n {
                let parts: Vec<&str> = members.iter().map(|&m| if let DKind::Text(a) = &ix.dcols[m].kind { a.get(i) } else { "" }).collect();
                both.push(&parts.join("\u{0}"));
            }
            let live = &ix.tlive[table];
            let tri = Trigrams::build(&both, |i| live[i], stop_fraction, stop_min);
            let id = ix.tris.len();
            ix.tris.push(tri);
            for m in members {
                ix.dcols[m].tri = Some(id);
            }
        }
        Ok(ix)
    }

    // ------------------------------------------------------------ helpers (also for dialect hooks) ---

    pub fn table_id(&self, name: &str) -> Option<usize> {
        self.store.schema.table(name)
    }

    pub fn rows_of(&self, table: &str) -> Result<usize, String> {
        Ok(self.store.tables[self.table_id(table).ok_or(format!("unknown table {table}"))?].rows())
    }

    pub fn primary_rows(&self) -> usize {
        self.store.tables[self.primary].rows()
    }

    pub fn row_live(&self, table: &str, row: usize) -> bool {
        self.table_id(table).is_some_and(|t| self.tlive[t].get(row).copied().unwrap_or(false))
    }

    /// Resolve a column or derived-column name (`table.column` to qualify). Dialect columns win, then the primary table, then the others.
    fn resolve(&self, name: &str) -> Result<Field, String> {
        let (tq, col) = match name.split_once('.') {
            Some((t, c)) => (self.table_id(t), c),
            None => (None, name),
        };
        let order: Vec<usize> = match tq {
            Some(t) => vec![t],
            None => std::iter::once(self.primary).chain((0..self.store.tables.len()).filter(|&t| t != self.primary)).collect(),
        };
        for &t in &order {
            if let Some(i) = self.dcols.iter().position(|d| d.table == t && d.name == col) {
                return Ok(Field { table: t, src: Src::Derived(i) });
            }
        }
        for &t in &order {
            let td = &self.store.schema.tables[t];
            if let Some(i) = td.columns.iter().position(|c| c.name == col || c.aliases.iter().any(|a| a == col)) {
                return Ok(Field { table: t, src: Src::Store(i) });
            }
        }
        Err(format!("unknown field {name}"))
    }

    fn reachable(&self, t: usize) -> Result<(), String> {
        if t == self.primary || self.maps[t].is_some() {
            Ok(())
        } else {
            Err(format!("table {} is not reachable from {}", self.store.schema.tables[t].name, self.store.schema.tables[self.primary].name))
        }
    }

    fn lift(&self, t: usize, tb: &Bits) -> Bits {
        if t == self.primary {
            return tb.clone();
        }
        let m = self.maps[t].expect("reachable");
        let mut b = Bits::new(self.primary_rows());
        for (i, &r) in m.iter().enumerate() {
            if self.tlive[self.primary][i] && tb.get(r as usize) {
                b.set(i);
            }
        }
        b
    }

    fn pred_t(&self, t: usize, f: impl Fn(usize) -> bool) -> Result<Bits, String> {
        self.reachable(t)?;
        let n = self.store.tables[t].rows();
        let mut tb = Bits::new(n);
        for r in 0..n {
            if self.tlive[t][r] && f(r) {
                tb.set(r);
            }
        }
        Ok(self.lift(t, &tb))
    }

    /// Bitset over primary rows of those rows of `table` (live ones only) for which `f` holds.
    pub fn pred(&self, table: &str, f: impl Fn(usize) -> bool) -> Result<Bits, String> {
        self.pred_t(self.table_id(table).ok_or(format!("unknown table {table}"))?, f)
    }

    /// Lift table rows to a bitset over primary rows.
    pub fn lift_rows(&self, table: &str, rows: &[u32]) -> Result<Bits, String> {
        let t = self.table_id(table).ok_or(format!("unknown table {table}"))?;
        self.reachable(t)?;
        let mut tb = Bits::new(self.store.tables[t].rows());
        rows.iter().for_each(|&r| tb.set(r as usize));
        Ok(self.lift(t, &tb))
    }

    pub fn live_rows(&self, table: &str) -> Result<Vec<u32>, String> {
        let t = self.table_id(table).ok_or(format!("unknown table {table}"))?;
        Ok((0..self.tlive[t].len() as u32).filter(|&i| self.tlive[t][i as usize]).collect())
    }

    pub fn text_arena(&self, field: &str) -> Result<&Arena, String> {
        match self.resolve(field)?.src {
            Src::Derived(i) => match &self.dcols[i].kind {
                DKind::Text(a) => Ok(a),
                _ => Err(format!("{field} is not a text column")),
            },
            _ => Err(format!("{field} has no folded text (declare it Text in the schema or derive it)")),
        }
    }

    /// Rows (of the field's table) whose folded text contains `needle`; narrowed by trigrams when the column has them.
    pub fn text_find_rows(&self, field: &str, needle: &str) -> Result<Vec<u32>, String> {
        let f = self.resolve(field)?;
        let Src::Derived(i) = f.src else { return Err(format!("{field} is not a text-indexed column")) };
        let d = &self.dcols[i];
        let DKind::Text(a) = &d.kind else { return Err(format!("{field} is not a text column")) };
        let live = &self.tlive[d.table];
        Ok(match d.tri {
            Some(t) => self.tris[t].find(a, &|r| live[r], needle),
            None => (0..a.len()).filter(|&r| live[r] && a.get(r).contains(needle)).map(|r| r as u32).collect(),
        })
    }

    /// Numeric value of a column for a row of its table.
    pub fn num_at(&self, field: &str) -> Result<Box<dyn Fn(usize) -> Option<f64> + '_>, String> {
        match self.acc(&self.resolve(field)?)? {
            Acc::Num(f) => Ok(f),
            _ => Err(format!("{field} is not numeric")),
        }
    }

    fn acc(&self, f: &Field) -> Result<Acc<'_>, String> {
        let mask = self.store.schema.tables[f.table].columns.get(match f.src {
            Src::Store(c) => c,
            _ => usize::MAX,
        });
        Ok(match f.src {
            Src::Derived(i) => match &self.dcols[i].kind {
                DKind::Text(a) => Acc::Str(Box::new(move |r| Some(a.get(r)))),
                DKind::Mask(v) => Acc::Mask(Box::new(move |r| Some(v[r]))),
                DKind::Num(v) => Acc::Num(Box::new(move |r| v[r])),
                DKind::Flag(v) => Acc::Num(Box::new(move |r| Some(v[r] as u8 as f64))),
            },
            Src::Store(c) => match &self.store.tables[f.table].cols[c] {
                ColData::Int(v) if mask.is_some_and(|m| m.index == IndexKind::Set) => Acc::Mask(Box::new(move |r| v[r].map(|x| x as u32))),
                ColData::Int(v) => Acc::Num(Box::new(move |r| v[r].map(|x| x as f64))),
                ColData::Float(v) => Acc::Num(Box::new(move |r| v[r])),
                ColData::Bool(v) => Acc::Num(Box::new(move |r| v[r].map(|x| x as u8 as f64))),
                ColData::Enum(v) => Acc::Num(Box::new(move |r| v[r].map(|x| x as f64))),
                ColData::Str(v) => Acc::Str(Box::new(move |r| v[r].as_deref())),
                _ => return Err(format!("column {} cannot be compared", self.store.schema.tables[f.table].columns[c].name)),
            },
        })
    }

    // ------------------------------------------------------------------------------ evaluation ---

    pub fn eval(&self, e: &GExpr) -> Result<Bits, String> {
        Ok(match e {
            GExpr::All => self.live_p.clone(),
            GExpr::None => Bits::new(self.primary_rows()),
            GExpr::And(v) => {
                let mut acc = self.live_p.clone();
                for x in v {
                    acc.and(&self.eval(x)?);
                }
                acc
            }
            GExpr::Or(v) => {
                let mut acc = Bits::new(self.primary_rows());
                for x in v {
                    acc.or(&self.eval(x)?);
                }
                acc
            }
            GExpr::Not(x) => {
                let mut b = self.eval(x)?;
                b.not_within(&self.live_p);
                b
            }
            GExpr::Cmp { field, op, val } => {
                let f = self.resolve(field)?;
                let op = *op;
                match (self.acc(&f)?, val) {
                    (Acc::Num(g), Val::Num(q)) => self.pred_t(f.table, |r| g(r).is_some_and(|v| num_cmp(v, *q, op)))?,
                    (Acc::Mask(g), Val::Mask(q)) => self.pred_t(f.table, |r| g(r).is_some_and(|v| mask_cmp(v, *q, op)))?,
                    (Acc::Str(g), Val::Str(q)) => match op {
                        Op::Colon | Op::Eq => self.pred_t(f.table, |r| g(r).is_some_and(|v| v == q))?,
                        Op::Ne => self.pred_t(f.table, |r| g(r).is_some_and(|v| v != q))?,
                        _ => return Err(format!("operator {op:?} is not defined for text column {field}")),
                    },
                    _ => return Err(format!("value type does not fit column {field}")),
                }
            }
            GExpr::Count { field, op, n } => {
                let f = self.resolve(field)?;
                match self.acc(&f)? {
                    Acc::Mask(g) => self.pred_t(f.table, |r| g(r).is_some_and(|v| num_cmp(v.count_ones() as f64, *n, *op)))?,
                    _ => return Err(format!("{field} is not a mask column")),
                }
            }
            GExpr::Present { field } => {
                let f = self.resolve(field)?;
                match self.acc(&f)? {
                    Acc::Num(g) => self.pred_t(f.table, |r| g(r).is_some())?,
                    Acc::Mask(g) => self.pred_t(f.table, |r| g(r).is_some())?,
                    Acc::Str(g) => self.pred_t(f.table, |r| g(r).is_some())?,
                }
            }
            GExpr::Text { field, needle } => {
                let f = self.resolve(field)?;
                match f.src {
                    Src::Derived(_) => {
                        let rows = self.text_find_rows(field, needle)?;
                        self.reachable(f.table)?;
                        let mut tb = Bits::new(self.store.tables[f.table].rows());
                        rows.iter().for_each(|&r| tb.set(r as usize));
                        self.lift(f.table, &tb)
                    }
                    Src::Store(_) => match self.acc(&f)? {
                        Acc::Str(g) => self.pred_t(f.table, |r| g(r).is_some_and(|v| crate::search::fold(v).contains(needle.as_str())))?,
                        _ => return Err(format!("{field} is not a text column")),
                    },
                }
            }
            GExpr::Custom { name, args } => match self.hooks {
                Some(h) => h.custom(self, name, args)?,
                None => return Err(format!("no dialect to evaluate {name}")),
            },
        })
    }

    pub fn search(&self, q: &GQuery) -> Result<GResult, String> {
        let mut hits = match &q.expr {
            Some(e) => {
                let mut b = self.eval(e)?;
                b.and(&self.live_p);
                b
            }
            None => self.live_p.clone(),
        };
        if let Some(h) = &q.hide {
            let f = self.resolve(h)?;
            if f.table != self.primary {
                return Err(format!("hide flag {h} must be on the primary table"));
            }
            if let Acc::Num(g) = self.acc(&f)? {
                let mut hb = Bits::new(self.primary_rows());
                (0..self.primary_rows()).filter(|&r| g(r) == Some(1.0)).for_each(|r| hb.set(r));
                hits.clear(&hb);
            }
        }
        if let Some(scope) = &q.scope {
            let n = self.primary_rows();
            let mut sb = Bits::new(n);
            scope.iter().filter(|&&r| (r as usize) < n).for_each(|&r| sb.set(r as usize));
            hits.and(&sb);
        }
        let mut rows: Vec<u32> = hits.ones().map(|i| i as u32).collect();
        // number getters over primary rows
        let num_p = |name: &str, null: f64| -> Result<Box<dyn Fn(usize) -> f64 + '_>, String> {
            let f = self.resolve(name)?;
            self.reachable(f.table)?;
            let Acc::Num(g) = self.acc(&f)? else { return Err(format!("{name} is not numeric")) };
            let m = self.maps[f.table];
            Ok(Box::new(move |p| g(m.map_or(p, |m| m[p] as usize)).unwrap_or(null)))
        };
        if q.unique_parent {
            let pr = match self.parent_ref {
                Some(c) => match &self.store.tables[self.primary].cols[c] {
                    ColData::Ref(v) => v,
                    _ => unreachable!(),
                },
                None => return Err("unique needs a parent table".into()),
            };
            let (pf, newest) = match &q.prefer {
                Some(p) => (num_p(&p.field, 0.0)?, p.newest),
                None => (Box::new(|_| 0.0) as Box<dyn Fn(usize) -> f64 + '_>, true),
            };
            let mut best: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
            for &r in &rows {
                let o = pr[r as usize];
                let better = |cur: u32| {
                    let (a, b) = (pf(r as usize), pf(cur as usize));
                    if newest {
                        a > b || (a == b && r < cur)
                    } else {
                        a < b || (a == b && r < cur)
                    }
                };
                best.entry(o).and_modify(|c| if better(*c) { *c = r }).or_insert(r);
            }
            rows.retain(|r| best[&pr[*r as usize]] == *r);
        }
        enum K<'b> {
            N(Box<dyn Fn(usize) -> f64 + 'b>),
            S(Box<dyn Fn(usize) -> &'b str + 'b>),
        }
        let mut keys: Vec<(K, bool)> = vec![];
        for s in &q.sort {
            let f = self.resolve(&s.field)?;
            self.reachable(f.table)?;
            let m = self.maps[f.table];
            let k = match self.acc(&f)? {
                Acc::Num(g) => K::N(Box::new(move |p| g(m.map_or(p, |m| m[p] as usize)).unwrap_or(s.null))),
                Acc::Str(g) => K::S(Box::new(move |p| g(m.map_or(p, |m| m[p] as usize)).unwrap_or(""))),
                Acc::Mask(g) => K::N(Box::new(move |p| g(m.map_or(p, |m| m[p] as usize)).map_or(s.null, |v| v as f64))),
            };
            keys.push((k, s.desc));
        }
        rows.sort_by(|&a, &b| {
            for (k, desc) in &keys {
                let (a, b) = (a as usize, b as usize);
                let ord = match k {
                    K::N(g) => g(a).total_cmp(&g(b)),
                    K::S(g) => g(a).cmp(g(b)),
                };
                let ord = if *desc { ord.reverse() } else { ord };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            a.cmp(&b)
        });
        let total = rows.len();
        let end = q.limit.map_or(total, |l| (q.offset + l).min(total));
        let rows = if q.offset >= total { Vec::new() } else { rows[q.offset..end].to_vec() };
        Ok(GResult { total, rows })
    }
}
