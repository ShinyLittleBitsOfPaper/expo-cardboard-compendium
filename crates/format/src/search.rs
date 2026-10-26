//! Search over a [`Dataset`]: derived in-memory index + query plan evaluation.
//!
//! The index is NOT stored in the file or in patches: it is rebuilt deterministically from the records
//! (see docs/search.md). All text is folded (lowercase, accents stripped) before indexing and matching.
//! Text predicates use a byte-trigram postings index (stop-grams dropped by a size gate), then verify
//! the candidates against the folded text; needles under 3 bytes, or made only of stop-grams, scan.
//! Everything is evaluated as bitsets over printing rows.

use crate::container::Dataset;
use crate::types::OracleRecord;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

pub mod coqs;

// ------------------------------------------------------------------ plan ---

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    /// `:` (colors/identity: contains; mv/rarity: equals)
    #[serde(rename = ":")]
    Colon,
    #[serde(rename = "=")]
    Eq,
    #[serde(rename = "!=")]
    Ne,
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = "<=")]
    Le,
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = ">=")]
    Ge,
}

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
}

#[derive(Clone, Debug, Default)]
pub struct SearchResult {
    /// matches before offset/limit
    pub total: usize,
    pub rows: Vec<u32>,
}

// ----------------------------------------------------------------- folding ---

fn fold_char(c: char, out: &mut String) {
    for l in c.to_lowercase() {
        match l {
            'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'ā' => out.push('a'),
            'é' | 'è' | 'ê' | 'ë' | 'ē' => out.push('e'),
            'í' | 'ì' | 'î' | 'ï' | 'ī' => out.push('i'),
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ō' => out.push('o'),
            'ú' | 'ù' | 'û' | 'ü' | 'ū' => out.push('u'),
            'ñ' => out.push('n'),
            'ç' => out.push('c'),
            'ý' | 'ÿ' => out.push('y'),
            'æ' => out.push_str("ae"),
            'œ' => out.push_str("oe"),
            'ß' => out.push_str("ss"),
            '’' | '‘' => out.push('\''),
            '—' | '–' => out.push('-'),
            x => out.push(x),
        }
    }
}

/// Generic referents modern Oracle text uses instead of the card name ("This land enters tapped.").
/// Verified on Scryfall: `o:"~ enters tapped"` matches such cards.
const THIS_WORDS: &[&str] = &[
    "creature", "land", "artifact", "enchantment", "permanent", "planeswalker", "battle", "spell", "card", "vehicle",
    "saga", "aura", "equipment", "class", "token", "attraction", "case", "room", "siege", "background", "contraption",
    "omen", "spacecraft", "planet", "emblem", "dungeon",
];

/// Cartesian substitution of every `~` (the gaps between `parts`) with each variant.
fn expand_tilde(parts: &[&str], variants: &[String]) -> Vec<String> {
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
fn json_str_field_set_type(extra: &str) -> Option<&str> {
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

/// Lowercase + strip common Latin accents; the same function is applied to text and needles.
pub fn fold(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        fold_char(c, &mut o);
    }
    o
}

// ----------------------------------------------------------------- bitsets ---

#[derive(Clone, Debug)]
struct Bits {
    w: Vec<u64>,
}

impl Bits {
    fn new(n: usize) -> Self {
        Self { w: vec![0; n.div_ceil(64)] }
    }
    fn set(&mut self, i: usize) {
        self.w[i >> 6] |= 1 << (i & 63);
    }
    fn get(&self, i: usize) -> bool {
        self.w[i >> 6] >> (i & 63) & 1 == 1
    }
    fn and(&mut self, o: &Bits) {
        self.w.iter_mut().zip(&o.w).for_each(|(a, b)| *a &= *b);
    }
    fn or(&mut self, o: &Bits) {
        self.w.iter_mut().zip(&o.w).for_each(|(a, b)| *a |= *b);
    }
    fn clear(&mut self, o: &Bits) {
        for (a, b) in self.w.iter_mut().zip(&o.w) {
            *a &= !*b;
        }
    }
    fn not_within(&mut self, universe: &Bits) {
        self.w.iter_mut().zip(&universe.w).for_each(|(a, u)| *a = !*a & *u);
    }
    fn ones(&self) -> impl Iterator<Item = usize> + '_ {
        self.w.iter().enumerate().flat_map(|(wi, &word)| {
            let mut x = word;
            std::iter::from_fn(move || {
                if x == 0 {
                    None
                } else {
                    let t = x.trailing_zeros() as usize;
                    x &= x - 1;
                    Some(wi * 64 + t)
                }
            })
        })
    }
}

// ----------------------------------------------------------------- trigrams ---

/// Text kept in one arena; `off[i]..off[i+1]` is row i.
#[derive(Default)]
struct Arena {
    text: String,
    off: Vec<u32>,
}

impl Arena {
    fn push(&mut self, s: &str) {
        self.text.push_str(s);
        self.off.push(self.text.len() as u32);
    }
    fn get(&self, i: usize) -> &str {
        let a = if i == 0 { 0 } else { self.off[i - 1] } as usize;
        &self.text[a..self.off[i] as usize]
    }
    fn len(&self) -> usize {
        self.off.len()
    }
}

#[derive(Default)]
struct Trigrams {
    /// sorted gram keys; `start[i]..start[i+1]` is the postings range in `post`
    keys: Vec<u32>,
    start: Vec<u32>,
    /// delta-varint row ids
    post: Vec<u8>,
    /// grams present in more than `stop_df` rows: not stored, never used to narrow
    stop: Vec<u32>,
}

fn gram(b: &[u8]) -> u32 {
    (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32
}

fn put_varint(out: &mut Vec<u8>, mut v: u32) {
    while v >= 0x80 {
        out.push(v as u8 & 0x7f | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

impl Trigrams {
    fn build(arena: &Arena, live: impl Fn(usize) -> bool, stop_fraction: f64, stop_min: usize) -> Self {
        let n = arena.len();
        let mut pairs: Vec<(u32, u32)> = Vec::new();
        let mut row: Vec<u32> = Vec::new();
        for i in 0..n {
            if !live(i) {
                continue;
            }
            row.clear();
            row.extend(arena.get(i).as_bytes().windows(3).map(gram));
            row.sort_unstable();
            row.dedup();
            pairs.extend(row.iter().map(|g| (*g, i as u32)));
        }
        pairs.sort_unstable();
        let stop_df = (((n as f64) * stop_fraction) as usize).max(stop_min);
        let mut t = Trigrams::default();
        let mut i = 0;
        while i < pairs.len() {
            let g = pairs[i].0;
            let mut j = i;
            while j < pairs.len() && pairs[j].0 == g {
                j += 1;
            }
            if j - i > stop_df {
                t.stop.push(g);
            } else {
                t.keys.push(g);
                t.start.push(t.post.len() as u32);
                let mut prev = 0u32;
                for p in &pairs[i..j] {
                    put_varint(&mut t.post, p.1 - prev);
                    prev = p.1;
                }
            }
            i = j;
        }
        t.start.push(t.post.len() as u32);
        t
    }

    fn postings(&self, g: u32) -> Option<Vec<u32>> {
        let k = self.keys.binary_search(&g).ok()?;
        let (mut p, end) = (self.start[k] as usize, self.start[k + 1] as usize);
        let mut out = Vec::new();
        let mut prev = 0u32;
        while p < end {
            let (mut v, mut sh) = (0u32, 0);
            loop {
                let b = self.post[p];
                p += 1;
                v |= ((b & 0x7f) as u32) << sh;
                if b & 0x80 == 0 {
                    break;
                }
                sh += 7;
            }
            prev += v;
            out.push(prev);
        }
        Some(out)
    }

    /// Serialized size estimate: gram key + offset per gram, plus the postings, plus stop list.
    fn size_bytes(&self) -> usize {
        self.keys.len() * 8 + self.post.len() + self.stop.len() * 4
    }

    /// Rows whose text contains `needle`, using trigrams to narrow and verifying against the text.
    fn find(&self, arena: &Arena, live: &dyn Fn(usize) -> bool, needle: &str) -> Vec<u32> {
        let nb = needle.as_bytes();
        let mut lists: Vec<Vec<u32>> = Vec::new();
        if nb.len() >= 3 {
            let mut gs: Vec<u32> = nb.windows(3).map(gram).collect();
            gs.sort_unstable();
            gs.dedup();
            for g in gs {
                if self.stop.binary_search(&g).is_ok() {
                    continue;
                }
                match self.postings(g) {
                    Some(l) => lists.push(l),
                    None => return Vec::new(), // no row has this gram
                }
            }
        }
        let verify = |i: usize| live(i) && arena.get(i).contains(needle);
        if lists.is_empty() {
            return (0..arena.len()).filter(|&i| verify(i)).map(|i| i as u32).collect();
        }
        lists.sort_by_key(|l| l.len());
        let mut cand = lists.remove(0);
        for l in &lists {
            let mut out = Vec::with_capacity(cand.len().min(l.len()));
            let (mut a, mut b) = (0, 0);
            while a < cand.len() && b < l.len() {
                match cand[a].cmp(&l[b]) {
                    Ordering::Less => a += 1,
                    Ordering::Greater => b += 1,
                    Ordering::Equal => {
                        out.push(cand[a]);
                        a += 1;
                        b += 1;
                    }
                }
            }
            cand = out;
            if cand.is_empty() {
                return cand;
            }
        }
        cand.retain(|&i| verify(i as usize));
        cand
    }
}

// ------------------------------------------------------------------- index ---

#[derive(Clone, Copy, Debug, Default)]
pub struct IndexStats {
    pub oracle_rows: usize,
    pub name_bytes: usize,
    pub text_bytes: usize,
    pub name_grams: usize,
    pub text_grams: usize,
    pub name_stop: usize,
    pub text_stop: usize,
    /// estimated serialized size if the index were stored
    pub name_index_bytes: usize,
    pub text_index_bytes: usize,
}

/// Size gate: a gram present in more than max(25% of rows, 8) rows is not stored (it barely narrows).
pub const STOP_FRACTION: f64 = 0.25;
pub const STOP_MIN: usize = 8;

pub struct SearchIndex {
    // oracle level
    o_live: Vec<bool>,
    names: Arena,   // fold(name): sort, `~` variants
    names_n: Arena, // fold_name(name): name search / exact
    names_s: Arena, // letters and digits only: sort key (Scryfall ignores spaces, hyphens, quotes, apostrophes when ordering)
    types: Arena,
    texts: Arena,      // reminder text stripped (o:)
    full_texts: Arena, // full text (fo:)
    name_tri: Trigrams,
    text_tri: Trigrams,
    o_colors: Vec<u8>,
    o_identity: Vec<u8>,
    o_mv: Vec<Option<f64>>,
    // printing level
    p_oracle: Vec<u32>,
    p_live: Vec<bool>,
    p_set: Vec<String>,
    p_cn: Vec<String>,
    p_rarity: Vec<Option<u32>>,
    p_released: Vec<u32>,
    rarities: Vec<String>,
    live_p: Bits,
    /// printings Scryfall hides by default (see `Query::include_extras`)
    hidden_p: Bits,
}

fn mask_from_json(v: &serde_json::Value) -> Option<u8> {
    let a = v.as_array()?;
    Some(a.iter().filter_map(|c| c.as_str()).fold(0, |m, c| m | color_bit(c.chars().next().unwrap_or(' '))))
}

/// Typed mask, else the key kept in `extra` (non-canonical order is stored there), else (colors only)
/// the union of the faces' colors, else 0.
fn oracle_mask(o: &OracleRecord, typed: Option<u8>, key: &str, faces: bool) -> u8 {
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

fn faces_color_union(o: &OracleRecord) -> Option<u8> {
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
fn color_bit(c: char) -> u8 {
    match c.to_ascii_uppercase() {
        'W' => 1,
        'U' => 2,
        'B' => 4,
        'R' => 8,
        'G' => 16,
        _ => 0,
    }
}

fn join_faces(top: &Option<String>, faces: &Option<Vec<crate::types::OracleFace>>, get: impl Fn(&crate::types::OracleFace) -> &Option<String>) -> String {
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

fn rarity_rank(s: &str) -> Option<u8> {
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

impl SearchIndex {
    pub fn build(ds: &Dataset) -> Self {
        Self::build_with(ds, STOP_FRACTION, STOP_MIN)
    }

    pub fn build_with(ds: &Dataset, stop_fraction: f64, stop_min: usize) -> Self {
        let no = ds.oracle.len();
        let mut s = SearchIndex {
            o_live: Vec::with_capacity(no),
            names: Arena::default(),
            names_n: Arena::default(),
            names_s: Arena::default(),
            types: Arena::default(),
            texts: Arena::default(),
            full_texts: Arena::default(),
            name_tri: Trigrams::default(),
            text_tri: Trigrams::default(),
            o_colors: Vec::with_capacity(no),
            o_identity: Vec::with_capacity(no),
            o_mv: Vec::with_capacity(no),
            p_oracle: Vec::new(),
            p_live: Vec::new(),
            p_set: Vec::new(),
            p_cn: Vec::new(),
            p_rarity: Vec::new(),
            p_released: Vec::new(),
            rarities: ds.meta.rarities.values().iter().map(|r| r.to_string()).collect(),
            live_p: Bits::new(ds.printings.len()),
            hidden_p: Bits::new(ds.printings.len()),
        };
        let layouts = ds.meta.layouts.values();
        let promo_types = ds.meta.promo_types.values();
        for o in &ds.oracle {
            s.o_live.push(!o.tombstone);
            let name = o.name.clone().unwrap_or_default();
            s.names.push(&fold(&name));
            s.names_n.push(&fold_name(&name));
            s.names_s.push(&fold(&name).chars().filter(|c| c.is_alphanumeric()).collect::<String>());
            s.types.push(&fold(&join_faces(&o.type_line, &o.faces, |f| &f.type_line)));
            let full = join_faces(&o.oracle_text, &o.faces, |f| &f.oracle_text);
            s.texts.push(&fold(&strip_reminder(&full)));
            s.full_texts.push(&fold(&full));
            s.o_colors.push(oracle_mask(o, o.colors, "colors", true));
            s.o_identity.push(oracle_mask(o, o.color_identity, "color_identity", false));
            s.o_mv.push(o.cmc);
        }
        let live = |i: usize| s.o_live[i];
        s.name_tri = Trigrams::build(&s.names_n, live, stop_fraction, stop_min);
        // grams of stripped AND full text (so grams spanning a removed reminder are indexed too); a row's text is
        // a superset check only, each query verifies against the arena it targets.
        let mut both = Arena::default();
        for i in 0..s.texts.len() {
            both.push(&format!("{}\u{0}{}", s.texts.get(i), s.full_texts.get(i)));
        }
        s.text_tri = Trigrams::build(&both, live, stop_fraction, stop_min);
        for (i, p) in ds.printings.iter().enumerate() {
            s.p_oracle.push(p.oracle_row);
            s.p_live.push(!p.tombstone);
            if !p.tombstone {
                s.live_p.set(i);
            }
            let layout = p.layout.and_then(|l| layouts.get(l as usize)).map(|x| x.as_str());
            let set_type = p.extra.as_deref().and_then(json_str_field_set_type);
            let playtest = p.promo_types.as_ref().is_some_and(|v| v.iter().any(|&t| promo_types.get(t as usize).is_some_and(|n| n == "playtest")));
            if matches!(layout, Some("vanguard" | "planar" | "scheme" | "phenomenon" | "art_series")) || matches!(set_type, Some("memorabilia")) || playtest {
                s.hidden_p.set(i);
            }
            s.p_set.push(p.set_code.as_deref().map(fold).unwrap_or_default());
            s.p_cn.push(p.collector_number.as_deref().map(fold).unwrap_or_default());
            s.p_rarity.push(p.rarity);
            s.p_released.push(p.released_at.unwrap_or(0));
        }
        s
    }

    pub fn stats(&self) -> IndexStats {
        IndexStats {
            oracle_rows: self.o_live.iter().filter(|x| **x).count(),
            name_bytes: self.names.text.len(),
            text_bytes: self.texts.text.len(),
            name_grams: self.name_tri.keys.len(),
            text_grams: self.text_tri.keys.len(),
            name_stop: self.name_tri.stop.len(),
            text_stop: self.text_tri.stop.len(),
            name_index_bytes: self.name_tri.size_bytes(),
            text_index_bytes: self.text_tri.size_bytes(),
        }
    }

    pub fn name_of(&self, oracle_row: usize) -> &str {
        self.names.get(oracle_row)
    }

    fn expand(&self, oracle_rows: &[u32]) -> Bits {
        let mut o = Bits::new(self.o_live.len());
        oracle_rows.iter().for_each(|&r| o.set(r as usize));
        let mut b = Bits::new(self.p_oracle.len());
        for (i, &or) in self.p_oracle.iter().enumerate() {
            if self.p_live[i] && o.get(or as usize) {
                b.set(i);
            }
        }
        b
    }

    fn oracle_pred(&self, f: impl Fn(usize) -> bool) -> Bits {
        let mut b = Bits::new(self.p_oracle.len());
        for (i, &or) in self.p_oracle.iter().enumerate() {
            if self.p_live[i] && self.o_live[or as usize] && f(or as usize) {
                b.set(i);
            }
        }
        b
    }

    fn eval(&self, e: &Expr) -> Bits {
        match e {
            Expr::And(v) => {
                let mut acc = self.live_p.clone();
                for x in v {
                    acc.and(&self.eval(x));
                }
                acc
            }
            Expr::Or(v) => {
                let mut acc = Bits::new(self.p_oracle.len());
                for x in v {
                    acc.or(&self.eval(x));
                }
                acc
            }
            Expr::Not(x) => {
                let mut b = self.eval(x);
                b.not_within(&self.live_p);
                b
            }
            Expr::Name(q) => self.expand(&self.name_tri.find(&self.names_n, &|i| self.o_live[i], &fold_name(q))),
            Expr::Oracle(q) => self.expand(&self.text_find(&self.texts, q)),
            Expr::FullOracle(q) => self.expand(&self.text_find(&self.full_texts, q)),
            Expr::Type(q) => {
                let q = fold(q);
                self.oracle_pred(|i| self.types.get(i).contains(&q))
            }
            Expr::MvParity(w) => {
                let odd = w.eq_ignore_ascii_case("odd");
                self.oracle_pred(|i| self.o_mv[i].is_some_and(|v| v.fract() == 0.0 && ((v as i64) % 2 != 0) == odd))
            }
            Expr::CnNum(m) => self.printing_pred(|i| {
                let d: String = self.p_cn[i].chars().take_while(|c| c.is_ascii_digit()).collect();
                d.parse::<f64>().is_ok_and(|v| num_cmp(v, m.value, m.op))
            }),
            Expr::NameExact(q) => {
                let q = fold_name(q);
                self.oracle_pred(|i| {
                    let n = self.names_n.get(i);
                    n == q || n.split(" // ").any(|f| f == q)
                })
            }
            Expr::ColorCount(m) => self.oracle_pred(|i| num_cmp(self.o_colors[i].count_ones() as f64, m.value, m.op)),
            Expr::IdentityCount(m) => self.oracle_pred(|i| num_cmp(self.o_identity[i].count_ones() as f64, m.value, m.op)),
            Expr::Color(c) => {
                let q = mask_of(&c.colors);
                self.oracle_pred(|i| mask_cmp(self.o_colors[i], q, c.op))
            }
            Expr::Identity(c) => {
                let q = mask_of(&c.colors);
                // Scryfall: bare `:` on identity = subset (`id<=`)
                let op = if c.op == Op::Colon { Op::Le } else { c.op };
                self.oracle_pred(|i| mask_cmp(self.o_identity[i], q, op))
            }
            Expr::Mv(m) => self.oracle_pred(|i| self.o_mv[i].is_some_and(|v| num_cmp(v, m.value, m.op))),
            Expr::Rarity(r) => {
                let want = r.value.to_ascii_lowercase();
                let wr = rarity_rank(&want);
                let mut b = Bits::new(self.p_oracle.len());
                for i in 0..self.p_oracle.len() {
                    if !self.p_live[i] {
                        continue;
                    }
                    let name = self.p_rarity[i].and_then(|id| self.rarities.get(id as usize)).map(|s| s.as_str());
                    let ok = match (name, wr) {
                        (Some(n), Some(w)) => match rarity_rank(n) {
                            Some(x) => num_cmp(x as f64, w as f64, r.op),
                            None => matches!(r.op, Op::Ne),
                        },
                        (Some(n), None) => (n == want) == !matches!(r.op, Op::Ne),
                        (None, _) => false,
                    };
                    if ok {
                        b.set(i);
                    }
                }
                b
            }
            Expr::Set(s) => {
                let s = fold(s);
                self.printing_pred(|i| self.p_set[i] == s)
            }
            Expr::Cn(c) => {
                let c = fold(c);
                self.printing_pred(|i| self.p_cn[i] == c)
            }
        }
    }

    /// Oracle rows whose `arena` text contains `q`; each `~` stands for the card's own name (full name, a face
    /// name, or the part before the first comma) or a generic referent such as "this land".
    fn text_find(&self, arena: &Arena, q: &str) -> Vec<u32> {
        let q = fold(q);
        if !q.contains('~') {
            return self.text_tri.find(arena, &|i| self.o_live[i], &q);
        }
        let parts: Vec<&str> = q.split('~').collect();
        let lit = parts.iter().max_by_key(|p| p.len()).copied().unwrap_or("");
        let cand: Vec<u32> = if lit.len() >= 3 {
            // narrow by the longest `~`-free literal (verify happens below against the substituted needle)
            self.text_tri.find(arena, &|i| self.o_live[i], lit)
        } else {
            (0..arena.len() as u32).filter(|&i| self.o_live[i as usize]).collect()
        };
        cand.into_iter()
            .filter(|&i| {
                let name = self.names.get(i as usize);
                let mut variants: Vec<String> = vec![name.to_string()];
                for f in name.split(" // ") {
                    variants.push(f.to_string());
                    if let Some((short, _)) = f.split_once(',') {
                        variants.push(short.to_string());
                    }
                }
                variants.extend(THIS_WORDS.iter().map(|w| format!("this {w}")));
                let text = arena.get(i as usize);
                expand_tilde(&parts, &variants).iter().any(|n| text.contains(n.as_str()))
            })
            .collect()
    }

    fn printing_pred(&self, f: impl Fn(usize) -> bool) -> Bits {
        let mut b = Bits::new(self.p_oracle.len());
        for i in 0..self.p_oracle.len() {
            if self.p_live[i] && f(i) {
                b.set(i);
            }
        }
        b
    }

    pub fn search(&self, q: &Query) -> SearchResult {
        let hits = match &q.expr {
            Some(e) => {
                let mut b = self.eval(e);
                b.and(&self.live_p);
                b
            }
            None => self.live_p.clone(),
        };
        let mut hits = hits;
        if !q.include_extras {
            hits.clear(&self.hidden_p);
        }
        let mut rows: Vec<u32> = hits.ones().map(|i| i as u32).collect();
        if q.unique == Unique::Cards {
            // one printing per card, chosen by `prefer` (ties: later/earlier row), before sorting
            let mut best: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
            for &r in &rows {
                let o = self.p_oracle[r as usize];
                let better = |cur: u32| {
                    let (a, b) = (self.p_released[r as usize], self.p_released[cur as usize]);
                    match q.prefer {
                        Prefer::Newest => (a, !r) > (b, !cur),
                        Prefer::Oldest => (a, r) < (b, cur),
                    }
                };
                best.entry(o).and_modify(|c| if better(*c) { *c = r }).or_insert(r);
            }
            rows.retain(|r| best[&self.p_oracle[*r as usize]] == *r);
        }
        let default = [SortKey { field: SortField::Released, desc: true }, SortKey { field: SortField::Name, desc: false }];
        let keys: &[SortKey] = if q.sort.is_empty() { &default } else { &q.sort };
        rows.sort_by(|&a, &b| {
            for k in keys {
                let (a, b) = (a as usize, b as usize);
                let (oa, ob) = (self.p_oracle[a] as usize, self.p_oracle[b] as usize);
                let ord = match k.field {
                    SortField::Released => self.p_released[a].cmp(&self.p_released[b]),
                    SortField::Name => self.names_s.get(oa).cmp(self.names_s.get(ob)),
                    SortField::Mv => self.o_mv[oa].unwrap_or(0.0).total_cmp(&self.o_mv[ob].unwrap_or(0.0)),
                    SortField::Rarity => self.rarity_key(a).cmp(&self.rarity_key(b)),
                };
                let ord = if k.desc { ord.reverse() } else { ord };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            a.cmp(&b)
        });
        let total = rows.len();
        let end = q.limit.map_or(total, |l| (q.offset + l).min(total));
        let rows = if q.offset >= total { Vec::new() } else { rows[q.offset..end].to_vec() };
        SearchResult { total, rows }
    }

    fn rarity_key(&self, p: usize) -> i32 {
        self.p_rarity[p]
            .and_then(|id| self.rarities.get(id as usize))
            .and_then(|n| rarity_rank(n))
            .map_or(-1, |r| r as i32)
    }
}


fn mask_of(s: &str) -> u8 {
    s.chars().fold(0, |m, c| m | color_bit(c))
}

/// `a` = the card's colors, `b` = the query's. Scryfall semantics: `:` and `>=` mean "includes".
fn mask_cmp(a: u8, b: u8, op: Op) -> bool {
    match op {
        Op::Colon | Op::Ge => a & b == b,
        Op::Gt => a & b == b && a != b,
        Op::Eq => a == b,
        Op::Ne => a != b,
        Op::Le => a & b == a,
        Op::Lt => a & b == a && a != b,
    }
}

fn num_cmp(a: f64, b: f64, op: Op) -> bool {
    match op {
        Op::Colon | Op::Eq => a == b,
        Op::Ne => a != b,
        Op::Lt => a < b,
        Op::Le => a <= b,
        Op::Gt => a > b,
        Op::Ge => a >= b,
    }
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
