//! In-memory table store (S2a): columnar, one typed vector per column, one tombstone flag per row. Rows are append-only and keep
//! their id for ever, exactly like the oracle/printing vectors of the old `Dataset`. On disk (S2b) rows are still written in
//! zstd blocks so appends touch only tail blocks; the columnar layout is the in-memory and search-time shape.
use crate::schema::*;
use crate::types::Dict;

#[derive(Clone, Debug, PartialEq)]
pub enum ColData {
    Str(Vec<Option<String>>),
    Int(Vec<Option<i64>>),
    Float(Vec<Option<f64>>),
    Bool(Vec<Option<bool>>),
    Uuid(Vec<[u8; 16]>),
    Enum(Vec<Option<u32>>),
    Set(Vec<Option<Vec<u32>>>),
    Ref(Vec<u32>),
    Bytes(Vec<Option<Vec<u8>>>),
    Json(Vec<Option<String>>),
}

impl ColData {
    pub fn new(ty: &ColType) -> Self {
        match ty {
            ColType::Str => Self::Str(vec![]),
            ColType::Int => Self::Int(vec![]),
            ColType::Float => Self::Float(vec![]),
            ColType::Bool => Self::Bool(vec![]),
            ColType::Uuid => Self::Uuid(vec![]),
            ColType::Enum(_) => Self::Enum(vec![]),
            ColType::Set(_) => Self::Set(vec![]),
            ColType::Ref(_) => Self::Ref(vec![]),
            ColType::Bytes => Self::Bytes(vec![]),
            ColType::Json => Self::Json(vec![]),
        }
    }
    pub fn len(&self) -> usize {
        match self {
            Self::Str(v) => v.len(),
            Self::Int(v) => v.len(),
            Self::Float(v) => v.len(),
            Self::Bool(v) => v.len(),
            Self::Uuid(v) => v.len(),
            Self::Enum(v) => v.len(),
            Self::Set(v) => v.len(),
            Self::Ref(v) => v.len(),
            Self::Bytes(v) => v.len(),
            Self::Json(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Append the empty value (None / zero id); used for tombstoned rows and placeholders.
    pub fn push_default(&mut self) {
        match self {
            Self::Str(v) | Self::Json(v) => v.push(None),
            Self::Int(v) => v.push(None),
            Self::Float(v) => v.push(None),
            Self::Bool(v) => v.push(None),
            Self::Uuid(v) => v.push([0; 16]),
            Self::Enum(v) => v.push(None),
            Self::Set(v) => v.push(None),
            Self::Ref(v) => v.push(0),
            Self::Bytes(v) => v.push(None),
        }
    }
    /// Copy `src[j]` into row `i` (`i == len` appends). Both columns must have the same type.
    pub fn set_from(&mut self, i: usize, src: &ColData, j: usize) {
        macro_rules! go {
            ($d:expr, $s:expr) => {{
                let x = $s[j].clone();
                if i == $d.len() {
                    $d.push(x)
                } else {
                    $d[i] = x
                }
            }};
        }
        match (self, src) {
            (Self::Str(d), Self::Str(s)) | (Self::Json(d), Self::Json(s)) => go!(d, s),
            (Self::Int(d), Self::Int(s)) => go!(d, s),
            (Self::Float(d), Self::Float(s)) => go!(d, s),
            (Self::Bool(d), Self::Bool(s)) => go!(d, s),
            (Self::Uuid(d), Self::Uuid(s)) => go!(d, s),
            (Self::Enum(d), Self::Enum(s)) => go!(d, s),
            (Self::Set(d), Self::Set(s)) => go!(d, s),
            (Self::Ref(d), Self::Ref(s)) => go!(d, s),
            (Self::Bytes(d), Self::Bytes(s)) => go!(d, s),
            _ => panic!("set_from: column type mismatch"),
        }
    }
    fn matches(&self, ty: &ColType) -> bool {
        matches!(
            (self, ty),
            (Self::Str(_), ColType::Str)
                | (Self::Int(_), ColType::Int)
                | (Self::Float(_), ColType::Float)
                | (Self::Bool(_), ColType::Bool)
                | (Self::Uuid(_), ColType::Uuid)
                | (Self::Enum(_), ColType::Enum(_))
                | (Self::Set(_), ColType::Set(_))
                | (Self::Ref(_), ColType::Ref(_))
                | (Self::Bytes(_), ColType::Bytes)
                | (Self::Json(_), ColType::Json)
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    /// `tomb[row]`: the row was removed; its natural key stays, every other column holds the empty value.
    pub tomb: Vec<bool>,
    /// One entry per `TableDef::columns`, same order.
    pub cols: Vec<ColData>,
}

impl Table {
    pub fn new(def: &TableDef) -> Self {
        Self { tomb: vec![], cols: def.columns.iter().map(|c| ColData::new(&c.ty)).collect() }
    }
    pub fn rows(&self) -> usize {
        self.tomb.len()
    }
    /// Append one tombstoned placeholder row.
    pub fn push_default(&mut self) {
        self.tomb.push(true);
        self.cols.iter_mut().for_each(|c| c.push_default());
    }
    /// Copy row `j` of `src` into row `i` (`i == rows()` appends).
    pub fn set_row_from(&mut self, i: usize, src: &Table, j: usize) {
        if i == self.tomb.len() {
            self.tomb.push(src.tomb[j]);
        } else {
            self.tomb[i] = src.tomb[j];
        }
        for (d, s) in self.cols.iter_mut().zip(&src.cols) {
            d.set_from(i, s, j);
        }
    }
}

/// A whole dataset: its schema, dataset-wide numbers, dictionaries (aligned with `schema.dicts`) and tables (aligned with `schema.tables`).
#[derive(Clone, Debug, PartialEq)]
pub struct Store {
    pub schema: Schema,
    pub epoch: u32,
    /// YYYYMMDD of the source (0 = unknown); never a wall-clock time, so identical input gives identical bytes.
    pub source_date: u32,
    pub dicts: Vec<Dict>,
    /// Dataset-level string lists a dialect needs (MTG: `dropped_keys`).
    pub props: Vec<(String, Vec<String>)>,
    pub tables: Vec<Table>,
}

impl Store {
    pub fn new(schema: Schema) -> Self {
        let dicts = schema.dicts.iter().map(|_| Dict::default()).collect();
        let tables = schema.tables.iter().map(Table::new).collect();
        Self { schema, epoch: 0, source_date: 0, dicts, props: vec![], tables }
    }
    pub fn table(&self, name: &str) -> Option<(&TableDef, &Table)> {
        let i = self.schema.table(name)?;
        Some((&self.schema.tables[i], &self.tables[i]))
    }
    pub fn col(&self, table: &str, column: &str) -> Option<&ColData> {
        let (def, t) = self.table(table)?;
        let j = def.columns.iter().position(|c| c.name == column)?;
        Some(&t.cols[j])
    }
    pub fn prop(&self, name: &str) -> Option<&[String]> {
        self.props.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_slice())
    }
    /// Consistency check: column types match the schema, every column is as long as the table, refs point at existing rows,
    /// sidecars have exactly one row per parent row, enum ids are inside their dictionaries.
    pub fn check(&self) -> std::result::Result<(), String> {
        self.schema.validate()?;
        if self.tables.len() != self.schema.tables.len() || self.dicts.len() != self.schema.dicts.len() {
            return Err("tables/dicts not aligned with the schema".into());
        }
        for (def, t) in self.schema.tables.iter().zip(&self.tables) {
            if t.cols.len() != def.columns.len() {
                return Err(format!("{}: wrong column count", def.name));
            }
            for (c, d) in def.columns.iter().zip(&t.cols) {
                if !d.matches(&c.ty) || d.len() != t.rows() {
                    return Err(format!("{}.{}: type or length mismatch", def.name, c.name));
                }
                let dict_len = |n: &str| self.schema.dict(n).map(|i| self.dicts[i].len()).unwrap_or(0) as u32;
                match (&c.ty, d) {
                    (ColType::Enum(n), ColData::Enum(v)) if v.iter().flatten().any(|&x| x >= dict_len(n)) => return Err(format!("{}.{}: enum id out of range", def.name, c.name)),
                    (ColType::Set(n), ColData::Set(v)) if v.iter().flatten().flatten().any(|&x| x >= dict_len(n)) => return Err(format!("{}.{}: set id out of range", def.name, c.name)),
                    (ColType::Ref(n), ColData::Ref(v)) => {
                        let rows = self.table(n).map(|(_, t)| t.rows()).unwrap_or(0) as u32;
                        if v.iter().zip(&t.tomb).any(|(&x, &dead)| !dead && x >= rows) {
                            return Err(format!("{}.{}: ref out of range", def.name, c.name));
                        }
                    }
                    _ => {}
                }
            }
            if let Rel::Sidecar(p) = &def.rel {
                if self.table(p).map(|(_, pt)| pt.rows()) != Some(t.rows()) {
                    return Err(format!("{}: sidecar must have one row per {p} row", def.name));
                }
            }
        }
        Ok(())
    }
}
