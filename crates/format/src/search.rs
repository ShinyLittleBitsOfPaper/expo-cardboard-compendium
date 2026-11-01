//! Text-index primitives shared by every dataset: folding, bitsets, a byte-trigram postings table over an arena of folded strings, and
//! the numeric comparison operator. The generic evaluator is `gindex`; the serialised form is `coqi` (see docs/search.md).
//!
//! All text is folded (lowercase, accents stripped) before indexing and matching. Text predicates use the trigram postings
//! (stop-grams dropped by a size gate), then verify candidates against the folded text; needles under 3 bytes, or made only of
//! stop-grams, scan.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

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
pub struct Bits {
    w: Vec<u64>,
}

impl Bits {
    pub fn new(n: usize) -> Self {
        Self { w: vec![0; n.div_ceil(64)] }
    }
    pub fn set(&mut self, i: usize) {
        self.w[i >> 6] |= 1 << (i & 63);
    }
    pub fn get(&self, i: usize) -> bool {
        self.w[i >> 6] >> (i & 63) & 1 == 1
    }
    pub fn and(&mut self, o: &Bits) {
        self.w.iter_mut().zip(&o.w).for_each(|(a, b)| *a &= *b);
    }
    pub fn or(&mut self, o: &Bits) {
        self.w.iter_mut().zip(&o.w).for_each(|(a, b)| *a |= *b);
    }
    pub fn clear(&mut self, o: &Bits) {
        for (a, b) in self.w.iter_mut().zip(&o.w) {
            *a &= !*b;
        }
    }
    pub fn not_within(&mut self, universe: &Bits) {
        self.w.iter_mut().zip(&universe.w).for_each(|(a, u)| *a = !*a & *u);
    }
    pub fn ones(&self) -> impl Iterator<Item = usize> + '_ {
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
pub struct Arena {
    pub(crate) text: String,
    pub(crate) off: Vec<u32>,
}

impl Arena {
    pub fn push(&mut self, s: &str) {
        self.text.push_str(s);
        self.off.push(self.text.len() as u32);
    }
    pub fn get(&self, i: usize) -> &str {
        let a = if i == 0 { 0 } else { self.off[i - 1] } as usize;
        &self.text[a..self.off[i] as usize]
    }
    pub fn len(&self) -> usize {
        self.off.len()
    }
}

#[derive(Default)]
pub struct Trigrams {
    /// sorted gram keys; `start[i]..start[i+1]` is the postings range in `post`
    pub(crate) keys: Vec<u32>,
    pub(crate) start: Vec<u32>,
    /// delta-varint row ids
    pub(crate) post: Vec<u8>,
    /// grams present in more than `stop_df` rows: not stored, never used to narrow
    pub(crate) stop: Vec<u32>,
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
    pub fn build(arena: &Arena, live: impl Fn(usize) -> bool, stop_fraction: f64, stop_min: usize) -> Self {
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

    /// Rows whose text contains `needle`, using trigrams to narrow and verifying against the text.
    pub fn find(&self, arena: &Arena, live: &dyn Fn(usize) -> bool, needle: &str) -> Vec<u32> {
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


/// Size gate: a gram present in more than max(25% of rows, 8) rows is not stored (it barely narrows).
pub const STOP_FRACTION: f64 = 0.25;
pub const STOP_MIN: usize = 8;

pub fn num_cmp(a: f64, b: f64, op: Op) -> bool {
    match op {
        Op::Colon | Op::Eq => a == b,
        Op::Ne => a != b,
        Op::Lt => a < b,
        Op::Le => a <= b,
        Op::Gt => a > b,
        Op::Ge => a >= b,
    }
}

