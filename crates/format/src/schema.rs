//! Self-describing dataset schema (S2a of docs/schema-design.md): tables, typed columns, relations, index kinds and field aliases.
//! A schema is a plain value: it serialises to JSON (authoring, `compile --schema`) and to a compact binary block (inside the
//! `.coqd`/`.coqi` files from S2b). Nothing here knows about any particular dataset.
use crate::codec::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ColType {
    /// Optional UTF-8 text.
    Str,
    /// Optional signed integer (dates as YYYYMMDD, bitmasks, counts).
    Int,
    /// Optional f64.
    Float,
    /// Optional tri-state bool (absent / false / true).
    Bool,
    /// Non-null 16-byte id (natural keys).
    Uuid,
    /// Optional id into the named append-only dictionary (`Schema::dicts`).
    Enum(String),
    /// Optional set of ids into the named dictionary.
    Set(String),
    /// Non-null row id in the named table (parent link).
    Ref(String),
    /// Optional opaque bytes, encoded and decoded by the dialect (not searchable).
    Bytes,
    /// Optional JSON object text: the lossless passthrough for keys the schema does not type.
    Json,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexKind {
    None,
    /// Exact value lookup (natural keys, enums, small codes).
    Exact,
    /// Case/diacritic-folded substring search via trigrams.
    Text,
    /// Numeric range (sorted values).
    Range,
    /// Set membership (bitmask or posting lists per id).
    Set,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnDef {
    pub name: String,
    pub ty: ColType,
    pub index: IndexKind,
    /// Alternative field names accepted by the query language (`cmc` for `mv`).
    pub aliases: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Rel {
    /// A root table.
    None,
    /// Many rows per row of `table` (MTG: printing -> oracle). The child has a `Ref` column to the parent.
    Parent(String),
    /// Exactly one row per row of `table`, stored separately so volatile columns do not disturb the parent's blocks (MTG: ranks).
    Sidecar(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TableDef {
    pub name: String,
    pub rel: Rel,
    /// Natural-key column (must be `Uuid`); looked up by `getByKey` and kept stable across releases.
    pub key: Option<String>,
    pub columns: Vec<ColumnDef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Schema {
    /// Dialect id (`"mtg"`); empty for a schema with no dialect.
    pub id: String,
    pub version: u32,
    /// Names of the dataset's append-only dictionaries, in storage order.
    pub dicts: Vec<String>,
    pub tables: Vec<TableDef>,
}

impl ColumnDef {
    pub fn new(name: &str, ty: ColType, index: IndexKind, aliases: &[&str]) -> Self {
        Self { name: name.into(), ty, index, aliases: aliases.iter().map(|s| s.to_string()).collect() }
    }
}

impl Schema {
    pub fn table(&self, name: &str) -> Option<usize> {
        self.tables.iter().position(|t| t.name == name)
    }
    pub fn dict(&self, name: &str) -> Option<usize> {
        self.dicts.iter().position(|d| d == name)
    }

    /// Structural checks: unique names, known parents and dictionaries, a `Uuid` key column, a `Ref` column for every `Parent`.
    pub fn validate(&self) -> std::result::Result<(), String> {
        for (i, t) in self.tables.iter().enumerate() {
            if self.tables[..i].iter().any(|o| o.name == t.name) {
                return Err(format!("duplicate table {}", t.name));
            }
            match &t.rel {
                Rel::None => {}
                Rel::Parent(p) | Rel::Sidecar(p) => {
                    let pi = self.table(p).ok_or(format!("{}: unknown parent {p}", t.name))?;
                    if pi >= i {
                        return Err(format!("{}: parent {p} must be declared before it", t.name));
                    }
                }
            }
            for (j, c) in t.columns.iter().enumerate() {
                if t.columns[..j].iter().any(|o| o.name == c.name) {
                    return Err(format!("{}: duplicate column {}", t.name, c.name));
                }
                match &c.ty {
                    ColType::Enum(d) | ColType::Set(d) if self.dict(d).is_none() => return Err(format!("{}.{}: unknown dict {d}", t.name, c.name)),
                    ColType::Ref(r) if self.table(r).is_none() => return Err(format!("{}.{}: unknown table {r}", t.name, c.name)),
                    _ => {}
                }
            }
            if let Some(k) = &t.key {
                match t.columns.iter().find(|c| &c.name == k) {
                    Some(c) if c.ty == ColType::Uuid => {}
                    _ => return Err(format!("{}: key {k} must be a Uuid column", t.name)),
                }
            }
            if let Rel::Parent(p) = &t.rel {
                if !t.columns.iter().any(|c| c.ty == ColType::Ref(p.clone())) {
                    return Err(format!("{}: needs a Ref({p}) column", t.name));
                }
            }
        }
        Ok(())
    }

    pub fn encode(&self, w: &mut Writer) {
        w.str(&self.id);
        w.u32(self.version);
        w.uvarint(self.dicts.len() as u64);
        for d in &self.dicts {
            w.str(d);
        }
        w.uvarint(self.tables.len() as u64);
        for t in &self.tables {
            w.str(&t.name);
            match &t.rel {
                Rel::None => w.u8(0),
                Rel::Parent(p) => {
                    w.u8(1);
                    w.str(p);
                }
                Rel::Sidecar(p) => {
                    w.u8(2);
                    w.str(p);
                }
            }
            w.opt_str(t.key.as_deref());
            w.uvarint(t.columns.len() as u64);
            for c in &t.columns {
                w.str(&c.name);
                let (tag, arg) = match &c.ty {
                    ColType::Str => (0, None),
                    ColType::Int => (1, None),
                    ColType::Float => (2, None),
                    ColType::Bool => (3, None),
                    ColType::Uuid => (4, None),
                    ColType::Enum(d) => (5, Some(d)),
                    ColType::Set(d) => (6, Some(d)),
                    ColType::Ref(t) => (7, Some(t)),
                    ColType::Bytes => (8, None),
                    ColType::Json => (9, None),
                };
                w.u8(tag);
                if let Some(a) = arg {
                    w.str(a);
                }
                w.u8(match c.index {
                    IndexKind::None => 0,
                    IndexKind::Exact => 1,
                    IndexKind::Text => 2,
                    IndexKind::Range => 3,
                    IndexKind::Set => 4,
                });
                w.uvarint(c.aliases.len() as u64);
                for a in &c.aliases {
                    w.str(a);
                }
            }
        }
    }

    pub fn decode(r: &mut Reader) -> Result<Self> {
        let id = r.string()?;
        let version = r.u32()?;
        let n = r.uvarint()? as usize;
        let mut dicts = Vec::with_capacity(n);
        for _ in 0..n {
            dicts.push(r.string()?);
        }
        let n = r.uvarint()? as usize;
        let mut tables = Vec::with_capacity(n);
        for _ in 0..n {
            let name = r.string()?;
            let rel = match r.u8()? {
                0 => Rel::None,
                1 => Rel::Parent(r.string()?),
                2 => Rel::Sidecar(r.string()?),
                _ => return Err(Error::Corrupt("bad relation tag")),
            };
            let key = r.opt_string()?;
            let nc = r.uvarint()? as usize;
            let mut columns = Vec::with_capacity(nc);
            for _ in 0..nc {
                let cname = r.string()?;
                let ty = match r.u8()? {
                    0 => ColType::Str,
                    1 => ColType::Int,
                    2 => ColType::Float,
                    3 => ColType::Bool,
                    4 => ColType::Uuid,
                    5 => ColType::Enum(r.string()?),
                    6 => ColType::Set(r.string()?),
                    7 => ColType::Ref(r.string()?),
                    8 => ColType::Bytes,
                    9 => ColType::Json,
                    _ => return Err(Error::Corrupt("bad column type tag")),
                };
                let index = match r.u8()? {
                    0 => IndexKind::None,
                    1 => IndexKind::Exact,
                    2 => IndexKind::Text,
                    3 => IndexKind::Range,
                    4 => IndexKind::Set,
                    _ => return Err(Error::Corrupt("bad index kind")),
                };
                let na = r.uvarint()? as usize;
                let mut aliases = Vec::with_capacity(na);
                for _ in 0..na {
                    aliases.push(r.string()?);
                }
                columns.push(ColumnDef { name: cname, ty, index, aliases });
            }
            tables.push(TableDef { name, rel, key, columns });
        }
        Ok(Self { id, version, dicts, tables })
    }
}
