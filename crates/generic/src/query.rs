//! The generic query language. Fields, value types and operators come from the schema, so there is nothing to configure:
//!
//! ```text
//! term      := [-] ( field OP value | word | "phrase" | ( expr ) )     terms are ANDed; `or` between terms; `-` negates
//! OP        := :  =  !=  <  <=  >  >=
//! word      := substring search over every Text column of the primary table and its parent
//! directive := order:field | order:-field | dir:desc | unique:<parent table> | prefer:field | prefer:-field | has:field
//! ```
//! Per column type: Text `:` substring (folded), `=`/`!=` whole value; other Str `:`/`=` equals; Int/Float all operators;
//! Bool `:true`/`:false`; Enum `:name` (case-insensitive, `!=`); Set `:a,b` includes all (`=` exactly, `!=`, `<=` within).
//! A field not found in the dataset is looked up in the attached overlays (`price>1000`, or `overlay.price>1000`).
use crate::primary_table;
use coqc_format::gindex::{GExpr, GPrefer, GQuery, GSort, Val};
use coqc_format::schema::{ColType, IndexKind, Rel};
use coqc_format::search::{fold, Op};
use coqc_format::table::Store;

enum Tok {
    L,
    R,
    Or,
    Word(String),
}

fn tokenize(q: &str) -> Result<Vec<Tok>, String> {
    let cs: Vec<char> = q.chars().collect();
    let (mut i, mut out) = (0, vec![]);
    while i < cs.len() {
        match cs[i] {
            c if c.is_whitespace() => i += 1,
            '(' => {
                out.push(Tok::L);
                i += 1
            }
            ')' => {
                out.push(Tok::R);
                i += 1
            }
            _ => {
                let (mut w, mut inq) = (String::new(), false);
                while i < cs.len() && (inq || !(cs[i].is_whitespace() || cs[i] == ')')) {
                    if cs[i] == '"' {
                        inq = !inq;
                    }
                    w.push(cs[i]);
                    i += 1;
                }
                if inq {
                    return Err("unterminated quote".into());
                }
                out.push(if w.eq_ignore_ascii_case("or") { Tok::Or } else { Tok::Word(w) });
            }
        }
    }
    Ok(out)
}

fn unquote(s: &str) -> String {
    s.chars().filter(|c| *c != '"').collect()
}

/// `(field, op, value)` when the word is `ident OP value`.
fn split_term(w: &str) -> Option<(&str, Op, &str)> {
    let end = w.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))?;
    if end == 0 {
        return None;
    }
    let (f, rest) = w.split_at(end);
    for (s, op) in [(">=", Op::Ge), ("<=", Op::Le), ("!=", Op::Ne), (":", Op::Colon), ("=", Op::Eq), (">", Op::Gt), ("<", Op::Lt)] {
        if let Some(v) = rest.strip_prefix(s) {
            return Some((f, op, v));
        }
    }
    None
}

fn op_str(op: Op) -> &'static str {
    match op {
        Op::Colon => ":",
        Op::Eq => "=",
        Op::Ne => "!=",
        Op::Lt => "<",
        Op::Le => "<=",
        Op::Gt => ">",
        Op::Ge => ">=",
    }
}

struct P<'a> {
    st: &'a Store,
    ov: &'a [(String, &'a Store)],
    toks: Vec<Tok>,
    pos: usize,
    primary: usize,
    sorts: Vec<(String, bool)>,
    dir_desc: bool,
    unique: bool,
    prefer: Option<GPrefer>,
}

impl<'a> P<'a> {
    /// (table, column) of a field name: `table.column` or a column/alias, primary table first (the index resolves the same way).
    fn find(&self, name: &str) -> Option<(usize, usize)> {
        let sc = &self.st.schema;
        let hit = |t: usize, c: &str| sc.tables[t].columns.iter().position(|d| d.name.eq_ignore_ascii_case(c) || d.aliases.iter().any(|a| a.eq_ignore_ascii_case(c))).map(|ci| (t, ci));
        if let Some((t, c)) = name.split_once('.') {
            if let Some(ti) = sc.tables.iter().position(|d| d.name.eq_ignore_ascii_case(t)) {
                return hit(ti, c);
            }
        }
        std::iter::once(self.primary).chain((0..sc.tables.len()).filter(|&t| t != self.primary)).find_map(|t| hit(t, name))
    }

    fn qname(&self, t: usize, c: usize) -> String {
        format!("{}.{}", self.st.schema.tables[t].name, self.st.schema.tables[t].columns[c].name)
    }

    fn known_fields(&self) -> String {
        let sc = &self.st.schema;
        let mut v: Vec<&str> = std::iter::once(self.primary).chain((0..sc.tables.len()).filter(|&t| t != self.primary)).flat_map(|t| sc.tables[t].columns.iter().filter(|c| c.index != IndexKind::None).map(|c| c.name.as_str())).collect();
        for (_, o) in self.ov {
            v.extend(o.schema.tables.first().into_iter().flat_map(|t| t.columns.iter().map(|c| c.name.as_str())));
        }
        v.join(", ")
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn or(&mut self) -> Result<Option<GExpr>, String> {
        let mut alts = vec![];
        loop {
            alts.push(self.and()?);
            if matches!(self.peek(), Some(Tok::Or)) {
                self.pos += 1;
            } else {
                break;
            }
        }
        // an empty side of `or` (only directives) matches everything
        Ok(match alts.len() {
            1 => alts.pop().unwrap(),
            _ => Some(GExpr::Or(alts.into_iter().map(|a| a.unwrap_or(GExpr::All)).collect())),
        })
    }

    fn and(&mut self) -> Result<Option<GExpr>, String> {
        let mut v = vec![];
        while !matches!(self.peek(), None | Some(Tok::Or) | Some(Tok::R)) {
            if let Some(e) = self.unary()? {
                v.push(e);
            }
        }
        Ok(match v.len() {
            0 => None,
            1 => v.pop(),
            _ => Some(GExpr::And(v)),
        })
    }

    fn unary(&mut self) -> Result<Option<GExpr>, String> {
        let t = self.toks.get(self.pos).ok_or("unexpected end of query")?;
        self.pos += 1;
        match t {
            Tok::L => {
                let e = self.or()?;
                match self.peek() {
                    Some(Tok::R) => self.pos += 1,
                    _ => return Err("missing )".into()),
                }
                Ok(Some(e.unwrap_or(GExpr::All)))
            }
            Tok::R => Err("unexpected )".into()),
            Tok::Or => Err("unexpected or".into()),
            Tok::Word(w) => {
                let w = w.clone();
                let (neg, body) = match w.strip_prefix('-') {
                    Some(b) if !b.is_empty() => (true, b),
                    _ => (false, w.as_str()),
                };
                if body.starts_with('(') {
                    return Err("a negated group is not supported: negate the terms instead".into());
                }
                let e = match split_term(body) {
                    Some((f, op, v)) => match self.directive(f, op, v, neg)? {
                        true => return Ok(None),
                        false => self.field_term(f, op, &unquote(v))?,
                    },
                    None => self.bare(&unquote(body))?,
                };
                Ok(Some(if neg { GExpr::Not(Box::new(e)) } else { e }))
            }
        }
    }

    /// Handles `order:` `dir:` `unique:` `prefer:`; true when the term was a directive. `has:` is a predicate, not a directive.
    fn directive(&mut self, f: &str, op: Op, v: &str, neg: bool) -> Result<bool, String> {
        let name = f.to_ascii_lowercase();
        if !matches!(name.as_str(), "order" | "dir" | "direction" | "unique" | "prefer") {
            return Ok(false);
        }
        if neg || op != Op::Colon {
            return Err(format!("{name} takes `{name}:value` and cannot be negated"));
        }
        let v = unquote(v);
        match name.as_str() {
            "order" => {
                let (desc, field) = v.strip_prefix('-').map_or((false, v.as_str()), |x| (true, x));
                let (t, c) = self.find(field).ok_or_else(|| format!("order: unknown field {field}"))?;
                self.sorts.push((self.qname(t, c), desc));
            }
            "dir" | "direction" => match v.to_ascii_lowercase().as_str() {
                "desc" | "descending" => self.dir_desc = true,
                "asc" | "ascending" => self.dir_desc = false,
                _ => return Err("dir: use asc or desc".into()),
            },
            "unique" => {
                let parent = match &self.st.schema.tables[self.primary].rel {
                    Rel::Parent(p) => p.clone(),
                    _ => return Err("unique: the dataset has no parent table".into()),
                };
                match v.to_ascii_lowercase().as_str() {
                    "none" | "false" | "all" => self.unique = false,
                    x if x == "parent" || x == "true" || x.eq_ignore_ascii_case(&parent) => self.unique = true,
                    _ => return Err(format!("unique: use {parent} (one row per {parent}) or none")),
                }
            }
            _ => {
                let (low, field) = v.strip_prefix('-').map_or((false, v.as_str()), |x| (true, x));
                let (t, c) = self.find(field).ok_or_else(|| format!("prefer: unknown field {field}"))?;
                self.prefer = Some(GPrefer { field: self.qname(t, c), newest: !low });
            }
        }
        Ok(true)
    }

    fn bare(&self, w: &str) -> Result<GExpr, String> {
        if w.is_empty() {
            return Err("empty search word".into());
        }
        let sc = &self.st.schema;
        let needle = fold(w);
        let alts: Vec<GExpr> = std::iter::once(self.primary)
            .chain((0..sc.tables.len()).filter(|&t| t != self.primary))
            .flat_map(|t| sc.tables[t].columns.iter().enumerate().map(move |(c, d)| (t, c, d)))
            .filter(|(_, _, d)| d.ty == ColType::Str && d.index == IndexKind::Text)
            .map(|(t, c, _)| GExpr::Text { field: self.qname(t, c), needle: needle.clone() })
            .collect();
        match alts.len() {
            0 => Err("no text column to search; write field:value".into()),
            1 => Ok(alts.into_iter().next().unwrap()),
            _ => Ok(GExpr::Or(alts)),
        }
    }

    fn num(v: &str, field: &str) -> Result<f64, String> {
        v.trim().parse::<f64>().map_err(|_| format!("{field}: {v:?} is not a number"))
    }

    fn field_term(&self, f: &str, op: Op, v: &str) -> Result<GExpr, String> {
        if f.eq_ignore_ascii_case("has") {
            let (t, c) = self.find(v).ok_or_else(|| format!("has: unknown field {v}"))?;
            let (q, col) = (self.qname(t, c), &self.st.schema.tables[t].columns[c]);
            return Ok(match (&col.ty, col.index) {
                (ColType::Set(_), _) => GExpr::Count { field: q, op: Op::Gt, n: 0.0 },
                (ColType::Str, IndexKind::Text) => GExpr::Cmp { field: q, op: Op::Ne, val: Val::Str(String::new()) },
                _ => GExpr::Present { field: q },
            });
        }
        let Some((t, c)) = self.find(f) else { return self.overlay_term(f, op, v) };
        let sc = &self.st.schema;
        let col = &sc.tables[t].columns[c];
        let q = self.qname(t, c);
        if col.index == IndexKind::None {
            return Err(format!("{f} is not searchable (its index is None)"));
        }
        let cmp = |op: Op, val: Val| GExpr::Cmp { field: q.clone(), op, val };
        let only_eq = |op: Op| -> Result<(), String> {
            matches!(op, Op::Colon | Op::Eq | Op::Ne).then_some(()).ok_or_else(|| format!("{f}: operator {} is not defined for this column", op_str(op)))
        };
        let dict_of = |d: &str| sc.dict(d).map(|i| &self.st.dicts[i]).ok_or_else(|| format!("{f}: unknown dictionary {d}"));
        match &col.ty {
            ColType::Str if col.index == IndexKind::Text => {
                if v.is_empty() {
                    return Err(format!("{f}: empty value"));
                }
                match op {
                    Op::Colon => Ok(GExpr::Text { field: q, needle: fold(v) }),
                    Op::Eq | Op::Ne => Ok(cmp(op, Val::Str(fold(v)))),
                    _ => only_eq(op).map(|_| unreachable!()),
                }
            }
            ColType::Str => {
                only_eq(op)?;
                Ok(cmp(if op == Op::Ne { Op::Ne } else { Op::Eq }, Val::Str(v.to_string())))
            }
            ColType::Int if col.index == IndexKind::Set => Ok(cmp(op, Val::Mask(Self::num(v, f)? as u32))),
            ColType::Int | ColType::Float => Ok(cmp(op, Val::Num(Self::num(v, f)?))),
            ColType::Bool => {
                only_eq(op)?;
                let b = match v.to_ascii_lowercase().as_str() {
                    "true" | "yes" | "1" => 1.0,
                    "false" | "no" | "0" => 0.0,
                    _ => return Err(format!("{f}: use true or false")),
                };
                Ok(cmp(if op == Op::Ne { Op::Ne } else { Op::Eq }, Val::Num(b)))
            }
            ColType::Enum(d) => {
                only_eq(op)?;
                let dict = dict_of(d)?;
                let id = dict.id(v).or_else(|| dict.values().iter().position(|x| x.eq_ignore_ascii_case(v)).map(|i| i as u32));
                Ok(match (id, op) {
                    (Some(id), Op::Ne) => cmp(Op::Ne, Val::Num(id as f64)),
                    (Some(id), _) => cmp(Op::Eq, Val::Num(id as f64)),
                    (None, Op::Ne) => GExpr::Present { field: q },
                    (None, _) => GExpr::None,
                })
            }
            ColType::Set(d) => {
                let dict = dict_of(d)?;
                if dict.len() > crate::MAX_MASK_VALUES {
                    return Err(format!("{f}: dictionary {d} is too large for a mask-indexed set"));
                }
                let mut mask = 0u32;
                let mut unknown = false;
                for name in v.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    match dict.id(name).or_else(|| dict.values().iter().position(|x| x.eq_ignore_ascii_case(name)).map(|i| i as u32)) {
                        Some(i) => mask |= 1 << i,
                        None => unknown = true,
                    }
                }
                if mask == 0 && !unknown {
                    return Err(format!("{f}: empty value"));
                }
                if unknown && matches!(op, Op::Colon | Op::Eq | Op::Ge | Op::Gt) {
                    return Ok(GExpr::None);
                }
                Ok(cmp(op, Val::Mask(mask)))
            }
            _ => Err(format!("{f} cannot be searched (type {:?})", col.ty)),
        }
    }

    fn overlay_term(&self, f: &str, op: Op, v: &str) -> Result<GExpr, String> {
        let (qual, name) = f.split_once('.').map_or((None, f), |(a, b)| (Some(a), b));
        for (id, o) in self.ov {
            if qual.is_some_and(|x| !x.eq_ignore_ascii_case(id)) {
                continue;
            }
            if let Some(c) = o.schema.tables.first().and_then(|t| t.columns.iter().find(|c| c.name.eq_ignore_ascii_case(name) || c.aliases.iter().any(|a| a.eq_ignore_ascii_case(name)))) {
                if !matches!(c.ty, ColType::Int | ColType::Float) {
                    return Err(format!("{f}: overlay column is not numeric"));
                }
                let n = Self::num(v, f)?;
                return Ok(GExpr::Custom { name: "overlay".into(), args: vec![id.clone(), c.name.clone(), op_str(op).into(), n.to_string()] });
            }
        }
        Err(format!("unknown field {f} (fields: {})", self.known_fields()))
    }
}

/// Parse a query of the generic language against an open dataset (and its attached overlays).
pub fn parse(q: &str, st: &Store, overlays: &[(String, &Store)]) -> Result<GQuery, String> {
    let mut p = P { st, ov: overlays, toks: tokenize(q)?, pos: 0, primary: primary_table(&st.schema), sorts: vec![], dir_desc: false, unique: false, prefer: None };
    let e = p.or()?;
    if p.pos < p.toks.len() {
        return Err("unbalanced )".into());
    }
    let mut g = GQuery { expr: e, ..Default::default() };
    for (f, desc) in &p.sorts {
        let desc = *desc || p.dir_desc;
        g.sort.push(GSort { field: f.clone(), desc, null: if desc { f64::MIN } else { f64::MAX } });
    }
    g.unique_parent = p.unique;
    g.prefer = p.prefer;
    Ok(g)
}
