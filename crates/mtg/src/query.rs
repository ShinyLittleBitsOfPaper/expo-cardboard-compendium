//! The Scryfall-syntax query tree (the parser's output, `parse.rs`) and the MTG-specific folding/colour helpers. `gindex::translate`
//! turns a [`Query`] into the generic `GQuery` the index evaluates.
use crate::types::OracleRecord;
use coqc_format::search::{fold, Op};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorCmp {
    pub op: Op,
    /// Letters from `wubrg` (any case); empty or `c` = colorless.
    pub colors: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MvCmp {
    pub op: Op,
    pub value: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RarityCmp {
    pub op: Op,
    pub value: String,
}

/// Boolean tree of predicates, JSON-friendly: `{"and":[{"name":"bolt"},{"not":{"type":"land"}}]}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Expr {
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
    /// substring of the card name (all faces)
    Name(String),
    /// substring of oracle text without reminder text (all faces); `~` stands for the card's own name
    Oracle(String),
    /// substring of full oracle text including reminder text (`fo:`); `~` as above
    FullOracle(String),
    /// substring of type line (all faces)
    Type(String),
    /// `mv:even` / `mv:odd` (value is `"even"` or `"odd"`; fractional values match neither)
    MvParity(String),
    /// collector number compared numerically by its leading digits (`cn>50`); `cn:`/`cn=` use exact `Cn`
    CnNum(MvCmp),
    /// `!"name"`: whole card name equals (case/accent-folded) either the full name or one face
    NameExact(String),
    /// number of colors / identity colors compared with `value` (`c>=2`, `id=3`)
    ColorCount(MvCmp),
    IdentityCount(MvCmp),
    Color(ColorCmp),
    Identity(ColorCmp),
    Mv(MvCmp),
    Rarity(RarityCmp),
    /// set code, exact
    Set(String),
    /// collector number, exact
    Cn(String),
    /// `name:/re/`, `o:/re/`, `fo:/re/`, `t:/re/`; `field` is `name`, `oracle`, `fulloracle` or `type`. Case-insensitive, matched on the
    /// folded text.
    Regex { field: String, pattern: String },
    /// `pow`/`tou`/`loy` (`stat`) compared with a number or another stat/`mv` (`pow>tou`); any face may match
    Stat { stat: String, op: Op, rhs: StatRhs },
    /// `m:` mana cost; `cost` is canonical lowercase symbols (`{2}{r}{r}`), compared as a multiset; any face may match
    Mana { op: Op, cost: String },
    /// `f:`/`legal:` (`mode` = `legal`, legal or restricted), `banned:`, `restricted:` in `format` (lowercase)
    Legal { mode: String, format: String },
    /// `kw:`: the card has this keyword (exact, case-insensitive)
    Keyword(String),
    /// `a:`: printing artist contains
    Artist(String),
    /// `ft:`: printing flavor text contains
    Flavor(String),
    /// `is:` flag, canonical name from [`IS_O`] or [`IS_P`]
    Is(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatRhs {
    Num(f64),
    /// `pow`, `tou`, `loy` or `mv`
    Stat(String),
}

/// `is:` flags stored as bits of the derived oracle mask `is_o` (bit = position).
pub const IS_O: &[&str] = &["permanent", "spell", "historic", "vanilla", "commander", "gold", "hybrid", "phyrexian", "reserved"];
/// `is:` flags stored as bits of the derived printing mask `is_p`.
pub const IS_P: &[&str] = &[
    "foil", "nonfoil", "etched", "promo", "reprint", "digital", "fullart", "textless", "oversized", "variation", "spotlight", "funny", "split",
    "flip", "transform", "meld", "leveler", "saga", "adventure", "mdfc", "dfc", "token", "planar", "scheme", "vanguard", "augment", "host",
];

/// Canonical `is:` name (aliases resolved), or None when unsupported.
pub fn is_name(v: &str) -> Option<&'static str> {
    let v = match v.to_ascii_lowercase().as_str() {
        "full" => "fullart",
        "modal_dfc" => "mdfc",
        x => return IS_O.iter().chain(IS_P.iter()).find(|n| **n == x).copied(),
    };
    IS_O.iter().chain(IS_P.iter()).find(|n| **n == v).copied()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortField {
    Released,
    Name,
    Mv,
    Rarity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortKey {
    pub field: SortField,
    #[serde(default)]
    pub desc: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unique {
    /// every printing
    #[default]
    Prints,
    /// one printing per oracle card, chosen by `prefer` before sorting
    Cards,
}

/// Which printing represents a card when `unique` is `Cards` (`prefer:`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Prefer {
    #[default]
    Newest,
    Oldest,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Query {
    /// None = every live printing
    #[serde(default)]
    pub expr: Option<Expr>,
    /// empty = default order (released desc, name)
    #[serde(default)]
    pub sort: Vec<SortKey>,
    #[serde(default)]
    pub unique: Unique,
    /// Reveal vanguard/plane/scheme/phenomenon, memorabilia and playtest cards (Scryfall hides them unless
    /// `include:extras`, or the query names a type or set).
    #[serde(default)]
    pub include_extras: bool,
    #[serde(default)]
    pub prefer: Prefer,
    #[serde(default)]
    pub offset: usize,
    /// None = no limit
    #[serde(default)]
    pub limit: Option<usize>,
    /// Host-supplied scope: only these printing rows can match (a registered set, e.g. a collection or tag).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Vec<u32>>,
}

#[derive(Clone, Debug, Default)]
pub struct SearchResult {
    /// matches before offset/limit
    pub total: usize,
    pub rows: Vec<u32>,
}

/// Generic referents modern Oracle text uses instead of the card name ("This land enters tapped.").
/// Verified on Scryfall: `o:"~ enters tapped"` matches such cards.
pub const THIS_WORDS: &[&str] = &[
    "creature", "land", "artifact", "enchantment", "permanent", "planeswalker", "battle", "spell", "card", "vehicle",
    "saga", "aura", "equipment", "class", "token", "attraction", "case", "room", "siege", "background", "contraption",
    "omen", "spacecraft", "planet", "emblem", "dungeon",
];

/// Cartesian substitution of every `~` (the gaps between `parts`) with each variant.
pub fn expand_tilde(parts: &[&str], variants: &[String]) -> Vec<String> {
    let mut out = vec![parts[0].to_string()];
    for p in &parts[1..] {
        out = out.iter().flat_map(|o| variants.iter().map(move |v| format!("{o}{v}{p}"))).collect();
        if out.len() > 20_000 {
            break;
        }
    }
    out
}

/// `"set_type":"<v>"` out of a printing's residual JSON text (no full parse; values are plain ASCII words).
pub fn json_str_field_set_type(extra: &str) -> Option<&str> {
    let i = extra.find("\"set_type\":\"")? + 12;
    let rest = &extra[i..];
    Some(&rest[..rest.find('"')?])
}

/// Name key: `fold` with apostrophes and commas removed ("krrik son" finds "K'rrik, Son of Yawgmoth", as on Scryfall).
pub fn fold_name(s: &str) -> String {
    fold(s).chars().filter(|c| !matches!(c, '\'' | '’' | ',')).collect()
}

/// Oracle text without reminder text (parenthesised spans); Scryfall's `o:` ignores reminder text, `fo:` keeps it.
pub fn strip_reminder(s: &str) -> String {
    let (mut out, mut depth) = (String::with_capacity(s.len()), 0u32);
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join("\n")
}

fn mask_from_json(v: &serde_json::Value) -> Option<u8> {
    let a = v.as_array()?;
    Some(a.iter().filter_map(|c| c.as_str()).fold(0, |m, c| m | color_bit(c.chars().next().unwrap_or(' '))))
}

/// Typed mask, else the key kept in `extra` (non-canonical order is stored there), else (colors only)
/// the union of the faces' colors, else 0.
pub fn oracle_mask(o: &OracleRecord, typed: Option<u8>, key: &str, faces: bool) -> u8 {
    if let Some(m) = typed {
        return m;
    }
    if let Some(m) = o
        .extra
        .as_deref()
        .and_then(|x| serde_json::from_str::<serde_json::Value>(x).ok())
        .and_then(|v| v.get(key).and_then(mask_from_json))
    {
        return m;
    }
    if faces {
        faces_color_union(o).unwrap_or(0)
    } else {
        0
    }
}

pub fn faces_color_union(o: &OracleRecord) -> Option<u8> {
    let mut m = 0u8;
    let mut any = false;
    for f in o.faces.iter().flatten() {
        let Some(x) = f.extra.as_deref() else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(x) else { continue };
        if let Some(a) = v.get("colors").and_then(|c| c.as_array()) {
            any = true;
            for c in a.iter().filter_map(|c| c.as_str()) {
                m |= color_bit(c.chars().next().unwrap_or(' '));
            }
        }
    }
    any.then_some(m)
}

/// WUBRG bit (same order as the file's color masks).
pub fn color_bit(c: char) -> u8 {
    match c.to_ascii_uppercase() {
        'W' => 1,
        'U' => 2,
        'B' => 4,
        'R' => 8,
        'G' => 16,
        _ => 0,
    }
}

pub fn join_faces(top: &Option<String>, faces: &Option<Vec<crate::types::OracleFace>>, get: impl Fn(&crate::types::OracleFace) -> &Option<String>) -> String {
    if let Some(t) = top {
        return t.clone();
    }
    let mut s = String::new();
    for f in faces.iter().flatten() {
        if let Some(x) = get(f) {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(x);
        }
    }
    s
}

pub fn rarity_rank(s: &str) -> Option<u8> {
    match s {
        "common" => Some(0),
        "uncommon" => Some(1),
        "rare" => Some(2),
        "special" => Some(3),
        "mythic" => Some(4),
        "bonus" => Some(5),
        _ => None,
    }
}

pub fn mask_of(s: &str) -> u8 {
    s.chars().fold(0, |m, c| m | color_bit(c))
}


#[cfg(test)]
mod tilde_tests {
    use super::*;
    #[test]
    fn expands_names_and_this_referents() {
        let v: Vec<String> = vec!["azorius guildgate".into(), "this land".into()];
        let n = expand_tilde(&["", " enters tapped"], &v);
        assert!(n.contains(&"this land enters tapped".to_string()) && n.contains(&"azorius guildgate enters tapped".to_string()));
        assert_eq!(expand_tilde(&["a ", " b ", " c"], &v).len(), 4);
    }
}

