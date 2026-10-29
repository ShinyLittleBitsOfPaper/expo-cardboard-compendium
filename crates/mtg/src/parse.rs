//! Scryfall-syntax query parser: raw query string -> `search::Query`.
//! Mirrors https://scryfall.com/docs/syntax. Unknown/unsupported keywords are errors (as on Scryfall),
//! never silently treated as name text. Grammar: `or` < implicit/`and` < `-` / `( )` / terms.
use coqc_format::search::*;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError(pub String);
impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ParseError {}
type R<T> = Result<T, ParseError>;
fn err<T>(m: impl Into<String>) -> R<T> {
    Err(ParseError(m.into()))
}

const NAMED: &[(&str, &str)] = &[
    ("white", "w"), ("blue", "u"), ("black", "b"), ("red", "r"), ("green", "g"),
    ("azorius", "wu"), ("dimir", "ub"), ("rakdos", "br"), ("gruul", "rg"), ("selesnya", "gw"),
    ("orzhov", "wb"), ("izzet", "ur"), ("golgari", "bg"), ("boros", "rw"), ("simic", "gu"),
    ("bant", "gwu"), ("esper", "wub"), ("grixis", "ubr"), ("jund", "brg"), ("naya", "rgw"),
    ("lorehold", "rw"), ("prismari", "ur"), ("quandrix", "gu"), ("silverquill", "wb"), ("witherbloom", "bg"),
    ("abzan", "wbg"), ("jeskai", "urw"), ("sultai", "bgu"), ("mardu", "rwb"), ("temur", "gur"),
    ("chaos", "ubrg"), ("aggression", "wbrg"), ("altruism", "wubg"), ("growth", "wurg"), ("artifice", "wubr"),
];

struct P {
    c: Vec<char>,
    i: usize,
    order: Option<SortField>,
    dir: Option<bool>,
    unique: Unique,
    prefer: Prefer,
    extras: bool,
}

pub fn parse(q: &str) -> R<Query> {
    let mut p = P { c: q.chars().collect(), i: 0, order: None, dir: None, unique: Unique::Cards, prefer: Prefer::Newest, extras: false };
    let expr = p.or()?;
    p.ws();
    if p.i < p.c.len() {
        return err("unbalanced ')'");
    }
    let reveals = expr.as_ref().is_some_and(names_type_or_set);
    let mut out = Query { expr, unique: p.unique, prefer: p.prefer, include_extras: p.extras || reveals, ..Default::default() };
    if let Some(f) = p.order {
        let desc = p.dir.unwrap_or(matches!(f, SortField::Released));
        out.sort = vec![SortKey { field: f, desc }];
        if f != SortField::Name {
            out.sort.push(SortKey { field: SortField::Name, desc: false });
        }
    } else {
        // Scryfall default: alphabetical
        out.sort = vec![SortKey { field: SortField::Name, desc: p.dir.unwrap_or(false) }];
    }
    Ok(out)
}

impl P {
    fn ws(&mut self) {
        while self.c.get(self.i).is_some_and(|c| c.is_whitespace()) {
            self.i += 1;
        }
    }
    fn peek(&self) -> Option<char> {
        self.c.get(self.i).copied()
    }
    fn kw(&self, w: &str) -> bool {
        let n = w.len();
        let s: String = self.c.iter().skip(self.i).take(n).collect();
        s.eq_ignore_ascii_case(w) && self.c.get(self.i + n).map_or(true, |c| c.is_whitespace() || *c == '(')
    }
    fn or(&mut self) -> R<Option<Expr>> {
        let mut alts: Vec<Expr> = Vec::new();
        alts.extend(self.and()?);
        loop {
            self.ws();
            if self.kw("or") {
                self.i += 2;
                alts.extend(self.and()?);
            } else {
                break;
            }
        }
        Ok(match alts.len() {
            0 => None,
            1 => alts.pop(),
            _ => Some(Expr::Or(alts)),
        })
    }
    fn and(&mut self) -> R<Option<Expr>> {
        let mut items: Vec<Expr> = Vec::new();
        loop {
            self.ws();
            if self.peek().is_none() || self.peek() == Some(')') || self.kw("or") {
                break;
            }
            if self.kw("and") {
                self.i += 3;
                continue;
            }
            items.extend(self.unary()?);
        }
        Ok(match items.len() {
            0 => None,
            1 => items.pop(),
            _ => Some(Expr::And(items)),
        })
    }
    fn quoted(&mut self) -> R<String> {
        self.i += 1; // opening quote
        let mut s = String::new();
        while let Some(c) = self.peek() {
            self.i += 1;
            match c {
                '"' => return Ok(s),
                '\\' if self.peek() == Some('"') => {
                    s.push('"');
                    self.i += 1;
                }
                c => s.push(c),
            }
        }
        err("unterminated quote")
    }
    fn word(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_whitespace() || c == ')' {
                break;
            }
            s.push(c);
            self.i += 1;
        }
        s
    }
    fn value(&mut self) -> R<String> {
        if self.peek() == Some('"') {
            self.quoted()
        } else {
            Ok(self.word())
        }
    }
    fn unary(&mut self) -> R<Option<Expr>> {
        match self.peek() {
            Some('-') if self.c.get(self.i + 1).is_some_and(|c| !c.is_whitespace()) => {
                self.i += 1;
                match self.unary()? {
                    Some(e) => Ok(Some(Expr::Not(Box::new(e)))),
                    None => err("cannot negate a directive"),
                }
            }
            Some('(') => {
                self.i += 1;
                let e = self.or()?;
                self.ws();
                if self.peek() != Some(')') {
                    return err("missing ')'");
                }
                self.i += 1;
                Ok(e)
            }
            Some('!') => {
                self.i += 1;
                let v = self.value()?;
                Ok(Some(Expr::NameExact(v)))
            }
            Some('"') => Ok(Some(Expr::Name(self.quoted()?))),
            _ => {
                let start = self.i;
                let mut key = String::new();
                while let Some(c) = self.peek().filter(|c| c.is_ascii_alphabetic()) {
                    key.push(c);
                    self.i += 1;
                }
                if !key.is_empty() {
                    if let Some(op) = self.op() {
                        let v = self.value()?;
                        return self.term(&key.to_ascii_lowercase(), op, &v);
                    }
                }
                self.i = start;
                Ok(Some(Expr::Name(self.word())))
            }
        }
    }
    fn op(&mut self) -> Option<Op> {
        let rest: String = self.c.iter().skip(self.i).take(2).collect();
        for (s, op) in [(">=", Op::Ge), ("<=", Op::Le), ("!=", Op::Ne), (":", Op::Colon), ("=", Op::Eq), ("<", Op::Lt), (">", Op::Gt)] {
            if rest.starts_with(s) {
                self.i += s.len();
                return Some(op);
            }
        }
        None
    }
    fn term(&mut self, key: &str, op: Op, v: &str) -> R<Option<Expr>> {
        let text_op = |op: Op| if matches!(op, Op::Colon | Op::Eq) { Ok(()) } else { err(format!("`{key}` only supports : and =")) };
        Ok(Some(match key {
            "name" => { text_op(op)?; Expr::Name(v.into()) }
            "t" | "type" => { text_op(op)?; Expr::Type(v.into()) }
            "o" | "oracle" => { text_op(op)?; Expr::Oracle(v.into()) }
            "fo" | "fulloracle" => { text_op(op)?; Expr::FullOracle(v.into()) }
            "c" | "color" | "colors" => color(op, v, false)?,
            "id" | "identity" | "ci" => color(op, v, true)?,
            "mv" | "manavalue" | "cmc" => {
                if op == Op::Colon && matches!(v.to_ascii_lowercase().as_str(), "even" | "odd") {
                    Expr::MvParity(v.to_ascii_lowercase())
                } else {
                    Expr::Mv(MvCmp { op, value: v.parse().map_err(|_| ParseError(format!("bad number: {v}")))? })
                }
            }
            "r" | "rarity" => {
                if !matches!(v.to_ascii_lowercase().as_str(), "common" | "uncommon" | "rare" | "special" | "mythic" | "bonus" | "c" | "u" | "r" | "m") {
                    return err(format!("unknown rarity: {v}"));
                }
                let full = match v.to_ascii_lowercase().as_str() { "c" => "common", "u" => "uncommon", "r" => "rare", "m" => "mythic", x => return Ok(Some(Expr::Rarity(RarityCmp { op, value: x.into() }))) };
                Expr::Rarity(RarityCmp { op, value: full.into() })
            }
            "s" | "set" | "e" | "edition" => { text_op(op)?; Expr::Set(v.into()) }
            "cn" | "number" => {
                if matches!(op, Op::Colon | Op::Eq) {
                    Expr::Cn(v.into())
                } else {
                    Expr::CnNum(MvCmp { op, value: v.parse().map_err(|_| ParseError(format!("bad number: {v}")))? })
                }
            }
            "order" => {
                self.order = Some(match v.to_ascii_lowercase().as_str() {
                    "name" => SortField::Name,
                    "released" => SortField::Released,
                    "cmc" => SortField::Mv,
                    "rarity" => SortField::Rarity,
                    x => return err(format!("unsupported order: {x}")),
                });
                return Ok(None);
            }
            "direction" => {
                self.dir = match v.to_ascii_lowercase().as_str() {
                    "asc" => Some(false),
                    "desc" => Some(true),
                    "auto" => None,
                    x => return err(format!("bad direction: {x}")),
                };
                return Ok(None);
            }
            "unique" => {
                self.unique = match v.to_ascii_lowercase().as_str() {
                    "cards" => Unique::Cards,
                    "prints" => Unique::Prints,
                    x => return err(format!("unsupported unique: {x}")),
                };
                return Ok(None);
            }
            "prefer" => {
                self.prefer = match v.to_ascii_lowercase().as_str() {
                    "newest" => Prefer::Newest,
                    "oldest" => Prefer::Oldest,
                    x => return err(format!("unsupported prefer: {x}")),
                };
                return Ok(None);
            }
            "include" if v.eq_ignore_ascii_case("extras") => {
                self.extras = true;
                return Ok(None);
            }
            other => return err(format!("unsupported keyword: {other}")),
        }))
    }
}

/// Scryfall shows hidden extras when the query names a type or a set.
fn names_type_or_set(e: &Expr) -> bool {
    match e {
        Expr::Type(_) | Expr::Set(_) => true,
        Expr::And(v) | Expr::Or(v) => v.iter().any(names_type_or_set),
        Expr::Not(x) => names_type_or_set(x),
        _ => false,
    }
}

fn color(op: Op, v: &str, identity: bool) -> R<Expr> {
    let l = v.to_ascii_lowercase();
    let cmp = |letters: String| {
        let c = ColorCmp { op, colors: letters };
        if identity { Expr::Identity(c) } else { Expr::Color(c) }
    };
    let count = |op: Op, n: f64| {
        let m = MvCmp { op, value: n };
        if identity { Expr::IdentityCount(m) } else { Expr::ColorCount(m) }
    };
    if let Ok(n) = l.parse::<f64>() {
        return Ok(count(op, n));
    }
    if l == "m" || l == "multicolor" {
        return match op {
            Op::Colon | Op::Eq | Op::Ge => Ok(count(Op::Ge, 2.0)),
            _ => err("multicolor only supports : = >="),
        };
    }
    if l == "c" || l == "colorless" {
        return Ok(cmp(String::new()));
    }
    if let Some((_, letters)) = NAMED.iter().find(|(n, _)| *n == l) {
        return Ok(cmp((*letters).into()));
    }
    if l.chars().all(|c| "wubrg".contains(c)) && !l.is_empty() {
        return Ok(cmp(l));
    }
    err(format!("unknown color: {v}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn e(q: &str) -> Expr {
        parse(q).unwrap().expr.unwrap()
    }
    #[test]
    fn precedence_and_negation() {
        assert_eq!(e("a b or c"), Expr::Or(vec![Expr::And(vec![Expr::Name("a".into()), Expr::Name("b".into())]), Expr::Name("c".into())]));
        assert_eq!(e("a (b or c)"), Expr::And(vec![Expr::Name("a".into()), Expr::Or(vec![Expr::Name("b".into()), Expr::Name("c".into())])]));
        assert_eq!(e("-t:land"), Expr::Not(Box::new(Expr::Type("land".into()))));
        assert_eq!(e("-(a or b)"), Expr::Not(Box::new(Expr::Or(vec![Expr::Name("a".into()), Expr::Name("b".into())]))));
        assert_eq!(e("lim-dul"), Expr::Name("lim-dul".into()));
    }
    #[test]
    fn terms() {
        assert_eq!(e("o:\"3 damage\""), Expr::Oracle("3 damage".into()));
        assert_eq!(e("!\"fire // ice\""), Expr::NameExact("fire // ice".into()));
        assert_eq!(e("id:bant"), Expr::Identity(ColorCmp { op: Op::Colon, colors: "gwu".into() }));
        assert_eq!(e("c:colorless"), Expr::Color(ColorCmp { op: Op::Colon, colors: "".into() }));
        assert_eq!(e("c:m"), Expr::ColorCount(MvCmp { op: Op::Ge, value: 2.0 }));
        assert_eq!(e("c>=2"), Expr::ColorCount(MvCmp { op: Op::Ge, value: 2.0 }));
        assert_eq!(e("mv:odd"), Expr::MvParity("odd".into()));
        assert_eq!(e("cn>50"), Expr::CnNum(MvCmp { op: Op::Gt, value: 50.0 }));
        assert_eq!(e("cn:150a"), Expr::Cn("150a".into()));
        assert_eq!(e("r:m"), Expr::Rarity(RarityCmp { op: Op::Colon, value: "mythic".into() }));
        assert_eq!(e("E:mh2"), Expr::Set("mh2".into()));
    }
    #[test]
    fn directives_and_errors() {
        let q = parse("bolt order:cmc direction:desc unique:cards include:extras").unwrap();
        assert_eq!(q.expr, Some(Expr::Name("bolt".into())));
        assert_eq!(q.unique, Unique::Cards);
        assert_eq!(q.sort[0], SortKey { field: SortField::Mv, desc: true });
        assert!(parse("").unwrap().expr.is_none());
        assert!(!parse("bolt").unwrap().include_extras);
        assert!(parse("bolt include:extras").unwrap().include_extras);
        assert!(parse("t:scheme").unwrap().include_extras && parse("s:mh2").unwrap().include_extras);
        assert!(parse("is:foil").is_err());
        assert!(parse("(a").is_err());
        assert!(parse("a)").is_err());
        assert!(parse("c:xyz").is_err());
        assert_eq!(parse("fo:flying").unwrap().expr, Some(Expr::FullOracle("flying".into())));
        let d = parse("bolt").unwrap();
        assert_eq!((d.unique, d.prefer, d.sort[0].field, d.sort[0].desc), (Unique::Cards, Prefer::Newest, SortField::Name, false));
    }
}
