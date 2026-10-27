//! Compiles Scryfall bulk card data (JSONL or a JSON array) into a COQD dataset.
//!
//! Two streaming passes over the input keep memory at "one card plus the
//! compact output": pass 1 collects vocabularies and picks the card that
//! represents each oracle id; pass 2 builds printing records and verifies that
//! every card rebuilds to exactly its original JSON (see `heal`).

pub use coqc_format::patch;
pub mod manifest;
pub mod prices;
pub mod quick;
pub mod synth;

use coqc_format::json::*;
use coqc_format::{Dataset, Dict, Images, Meta, OracleFace, OracleRecord, PrintingFace, PrintingRecord, Ranks};
use serde::de::{SeqAccess, Visitor};
use serde::Deserializer as _;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

pub type Obj = Map<String, Value>;
pub type Id = [u8; 16];

/// How NEW printing rows are ordered when first assigned (existing rows never move).
/// `Set` clusters printings of a set so each block holds similar data; `Uuid` is random.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RowOrder {
    #[default]
    Set,
    Uuid,
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    pub row_order: RowOrder,
    /// Ignore the previous release's row ids and dictionaries (new epoch).
    pub rebase: bool,
    /// YYYYMMDD of the source file; stored in the header (0 = unknown).
    pub source_date: u32,
    /// Keep Scryfall's volatile `prices` object (default: dropped).
    pub keep_prices: bool,
    /// Print progress lines to stderr while compiling.
    pub progress: bool,
    /// Leave out digital-only printings (Arena/MTGO/Alchemy). Default: included, as Scryfall search includes them.
    pub skip_digital: bool,
    /// Leave out art_series (memorabilia) cards. Default: included (hidden by default at search time).
    pub skip_art_series: bool,
}

struct Progress {
    on: bool,
    start: std::time::Instant,
}

impl Progress {
    fn new(on: bool) -> Self {
        Self { on, start: std::time::Instant::now() }
    }
    fn say(&self, msg: &str) {
        if self.on {
            eprintln!("[{:>6.1}s] {msg}", self.start.elapsed().as_secs_f64());
        }
    }
    fn tick(&self, label: &str, n: u64, total: Option<u64>) {
        if self.on && n % 10_000 == 0 {
            match total {
                Some(t) => self.say(&format!("{label}: {n}/{t} ({}%)", n * 100 / t.max(1))),
                None => self.say(&format!("{label}: {n} cards")),
            }
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct Stats {
    pub lines: u64,
    pub parse_errors: u64,
    pub skipped_digital: u64,
    pub skipped_art_series: u64,
    pub skipped_no_oracle_id: u64,
    pub skipped_bad_id: u64,
    pub duplicate_ids: u64,
    pub printings: u64,
    pub oracle_cards: u64,
    pub new_oracle: u64,
    pub new_printings: u64,
    pub tombstoned_oracle: u64,
    pub tombstoned_printings: u64,
    pub images_canonical: u64,
    pub images_raw: u64,
    pub healed_printings: u64,
    pub healed_keys: BTreeMap<String, u64>,
    /// Derivable URL keys (see `DERIVED_KEYS`): (not stored because derivable, stored because different).
    pub derived: BTreeMap<String, (u64, u64)>,
    /// First card seen for each healed key, as "name [id]".
    pub healed_examples: BTreeMap<String, String>,
    /// Why image URLs failed the canonical check: normalized signature -> count.
    pub image_mismatches: BTreeMap<String, u64>,
    /// First value seen for each residual key (truncated), to study derivable fields.
    pub extra_examples: BTreeMap<String, String>,
    pub extra_bytes_by_key: BTreeMap<String, u64>,
}

// ------------------------------------------------------------------ input ---

struct SeqVisitor<'a, F: FnMut(Obj)>(&'a mut F);

impl<'de, 'a, F: FnMut(Obj)> Visitor<'de> for SeqVisitor<'a, F> {
    type Value = u64;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "an array of card objects")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<u64, A::Error> {
        let mut n = 0;
        while let Some(card) = seq.next_element::<Obj>()? {
            (self.0)(clean(card));
            n += 1;
        }
        Ok(n)
    }
}

/// Streams cards from JSONL or a top-level JSON array (auto-detected).
/// Returns (cards or lines read, unparseable lines).
pub fn for_each_card(path: &Path, mut f: impl FnMut(Obj)) -> Result<(u64, u64), String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut reader = BufReader::with_capacity(1 << 20, file);
    let first = loop {
        let buf = reader.fill_buf().map_err(|e| e.to_string())?;
        if buf.is_empty() {
            return Ok((0, 0));
        }
        if let Some(i) = buf.iter().position(|b| !b.is_ascii_whitespace()) {
            break buf[i];
        }
        let n = buf.len();
        reader.consume(n);
    };
    if first == b'[' {
        let mut de = serde_json::Deserializer::from_reader(reader);
        let n = de.deserialize_seq(SeqVisitor(&mut f)).map_err(|e| format!("json array: {e}"))?;
        return Ok((n, 0));
    }
    let (mut lines, mut errors) = (0u64, 0u64);
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line).map_err(|e| e.to_string())? == 0 {
            break;
        }
        let t = line.trim_ascii();
        if t.is_empty() {
            continue;
        }
        lines += 1;
        match serde_json::from_slice::<Value>(t) {
            Ok(Value::Object(o)) => f(clean(o)),
            _ => errors += 1,
        }
    }
    Ok((lines, errors))
}

/// `null` is treated as "absent" everywhere (Scryfall uses both); drop it on ingest.
fn clean(o: Obj) -> Obj {
    let mut v = Value::Object(o);
    normalize(&mut v);
    match v {
        Value::Object(o) => o,
        _ => unreachable!(),
    }
}

enum Skip {
    Digital,
    ArtSeries,
    NoOracleId,
    BadId,
}

struct Accepted {
    id: Id,
    oracle_id: Id,
}

fn oracle_id_of(card: &Obj) -> Option<Id> {
    if let Some(Value::String(s)) = card.get("oracle_id") {
        return parse_uuid(s);
    }
    // reversible_card: oracle_id lives on each face instead.
    card.get("card_faces")?
        .as_array()?
        .first()?
        .get("oracle_id")?
        .as_str()
        .and_then(parse_uuid)
}

fn accept(card: &Obj, opts: &Options) -> Result<Accepted, Skip> {
    let id = card.get("id").and_then(Value::as_str).and_then(parse_uuid).ok_or(Skip::BadId)?;
    if opts.skip_digital && card.get("digital").and_then(Value::as_bool) == Some(true) {
        return Err(Skip::Digital);
    }
    if opts.skip_art_series && card.get("layout").and_then(Value::as_str) == Some("art_series") {
        return Err(Skip::ArtSeries);
    }
    let oracle_id = oracle_id_of(card).ok_or(Skip::NoOracleId)?;
    Ok(Accepted { id, oracle_id })
}

fn released_num(card: &Obj) -> u32 {
    card.get("released_at")
        .and_then(Value::as_str)
        .map(|s| s.replace('-', "").parse::<u32>().unwrap_or(0))
        .unwrap_or(0)
}

// ------------------------------------------------------------ field split ---

fn take_str(rest: &mut Obj, spill: &mut Obj, k: &str) -> Option<String> {
    match rest.remove(k) {
        Some(Value::String(s)) => Some(s),
        None | Some(Value::Null) => None,
        Some(v) => {
            spill.insert(k.to_string(), v);
            None
        }
    }
}

fn take_bool(rest: &mut Obj, spill: &mut Obj, k: &str) -> Option<bool> {
    match rest.remove(k) {
        Some(Value::Bool(b)) => Some(b),
        None | Some(Value::Null) => None,
        Some(v) => {
            spill.insert(k.to_string(), v);
            None
        }
    }
}

fn take_u32(rest: &mut Obj, spill: &mut Obj, k: &str) -> Option<u32> {
    match rest.remove(k) {
        Some(Value::Number(n)) => match n.as_u64().and_then(|v| u32::try_from(v).ok()) {
            Some(v) => Some(v),
            None => {
                spill.insert(k.to_string(), Value::Number(n));
                None
            }
        },
        None | Some(Value::Null) => None,
        Some(v) => {
            spill.insert(k.to_string(), v);
            None
        }
    }
}

fn take_dict(rest: &mut Obj, spill: &mut Obj, k: &str, d: &Dict) -> Option<u32> {
    let s = take_str(rest, spill, k)?;
    match d.id(&s) {
        Some(i) => Some(i),
        None => {
            spill.insert(k.to_string(), Value::from(s));
            None
        }
    }
}

fn color_mask(v: &Value) -> Option<u8> {
    let mut mask = 0u8;
    for c in v.as_array()? {
        let i = COLOR_LETTERS.iter().position(|l| Some(*l) == c.as_str())?;
        mask |= 1 << i;
    }
    // Only canonical WUBRG order without repeats round-trips through a mask.
    (colors_to_json(mask) == *v).then_some(mask)
}

fn take_colors(rest: &mut Obj, spill: &mut Obj, k: &str) -> Option<u8> {
    match rest.remove(k) {
        None | Some(Value::Null) => None,
        Some(v) => match color_mask(&v) {
            Some(m) => Some(m),
            None => {
                spill.insert(k.to_string(), v);
                None
            }
        },
    }
}

fn to_text(m: Obj) -> Option<String> {
    if m.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&Value::Object(m)).unwrap())
    }
}

fn legalities_of(rest: &mut Obj, spill: &mut Obj, formats: &Dict) -> Option<Vec<u8>> {
    let v = rest.remove("legalities")?;
    let Value::Object(m) = &v else {
        if !v.is_null() {
            spill.insert("legalities".into(), v);
        }
        return None;
    };
    let mut out = vec![0u8; formats.len()];
    for (k, val) in m {
        let code = val.as_str().and_then(|s| LEGALITY_NAMES.iter().position(|n| *n == s && !n.is_empty()));
        match (formats.id(k), code) {
            (Some(i), Some(c)) => out[i as usize] = c as u8,
            _ => {
                spill.insert("legalities".into(), v.clone());
                return None;
            }
        }
    }
    Some(out)
}

fn oracle_from_card(card: &Obj, oracle_id: Id) -> (OracleRecord, Ranks) {
    let mut rest = card.clone();
    let mut spill = Obj::new();
    let name = take_str(&mut rest, &mut spill, "name");
    let mana_cost = take_str(&mut rest, &mut spill, "mana_cost");
    let type_line = take_str(&mut rest, &mut spill, "type_line");
    let oracle_text = take_str(&mut rest, &mut spill, "oracle_text");
    let colors = take_colors(&mut rest, &mut spill, "colors");
    let color_identity = take_colors(&mut rest, &mut spill, "color_identity");
    let cmc = match rest.remove("cmc") {
        Some(Value::Number(n)) => n.as_f64(),
        _ => None,
    };
    // volatile ranks live in their own table, not in the oracle blocks
    let edhrec = take_u32(&mut rest, &mut spill, "edhrec_rank");
    let penny = take_u32(&mut rest, &mut spill, "penny_rank");
    for k in ORACLE_EXTRA_KEYS {
        if let Some(v) = rest.remove(*k) {
            spill.insert(k.to_string(), v);
        }
    }
    let faces = match card.get("card_faces") {
        Some(Value::Array(fs)) if fs.iter().all(Value::is_object) => Some(
            fs.iter()
                .map(|f| {
                    let mut f = f.as_object().unwrap().clone();
                    let mut fspill = Obj::new();
                    let name = take_str(&mut f, &mut fspill, "name");
                    let mana_cost = take_str(&mut f, &mut fspill, "mana_cost");
                    let type_line = take_str(&mut f, &mut fspill, "type_line");
                    let oracle_text = take_str(&mut f, &mut fspill, "oracle_text");
                    for k in ORACLE_FACE_EXTRA_KEYS {
                        if let Some(v) = f.remove(*k) {
                            fspill.insert(k.to_string(), v);
                        }
                    }
                    OracleFace { name, mana_cost, type_line, oracle_text, extra: to_text(fspill) }
                })
                .collect(),
        ),
        _ => None, // absent or malformed: healing restores the original
    };
    let rec = OracleRecord {
        tombstone: false,
        oracle_id,
        name,
        mana_cost,
        type_line,
        oracle_text,
        colors,
        color_identity,
        cmc,
        faces,
        extra: to_text(spill),
    };
    (rec, Ranks { edhrec, penny })
}

/// File extension of an image URL (`jpg`, `png`, `webp`, ...), ignoring the `?stamp`.
fn image_ext(url: &str) -> Option<&str> {
    let name = url.split('?').next()?.rsplit('/').next()?;
    let (_, ext) = name.rsplit_once('.')?;
    (!ext.is_empty() && ext.len() <= 5).then_some(ext)
}

/// First path segment after the CDN host (`normal` in `.../normal/front/...`).
/// It is NOT always equal to the JSON key (key `jpeg` points at `/normal/`).
fn image_seg(url: &str) -> Option<&str> {
    url.strip_prefix("https://cards.scryfall.io/")?.split('/').next().filter(|s| !s.is_empty())
}

/// Dictionary entries are `key=segment.ext`, e.g. `normal=normal.jpg`, `jpeg=normal.jpg`, `art=art.webp`.
fn image_entry(k: &str, url: &str) -> Option<String> {
    Some(format!("{k}={}.{}", image_seg(url)?, image_ext(url)?))
}

fn add_image_entries(vocab: &mut BTreeSet<String>, m: &Obj) {
    for (k, u) in m {
        if let Some(e) = u.as_str().and_then(|u| image_entry(k, u)) {
            vocab.insert(e);
        }
    }
}

/// Collapses ids and long digit runs so equal failure shapes count together.
fn signature(why: &str, id: &Id) -> String {
    let mut out = String::new();
    let mut digits = String::new();
    for c in why.replace(&uuid_to_string(id), "<id>").chars() {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        out.push_str(&if digits.len() >= 9 { "<n>".to_string() } else { std::mem::take(&mut digits) });
        digits.clear();
        out.push(c);
    }
    out.push_str(&if digits.len() >= 9 { "<n>".to_string() } else { digits });
    out
}

fn parse_stamp(url: &str) -> Option<u32> {
    url.split_once('?')?.1.parse().ok()
}

fn check_image(k: &str, u: &Value, side: &str, id: &Id, keys: &Dict, stamp: &mut Option<u32>) -> Result<u32, String> {
    let url = u.as_str().ok_or("non-string image url")?;
    let st = parse_stamp(url).ok_or("no numeric ?stamp")?;
    match *stamp {
        Some(prev) if prev != st => return Err(format!("stamp differs from the other sizes: {url}")),
        _ => *stamp = Some(st),
    }
    let seg = image_seg(url).ok_or_else(|| format!("unexpected host/path: {url}"))?;
    let ext = image_ext(url).ok_or_else(|| format!("no file extension: {url}"))?;
    let expected = image_url(seg, ext, side, id, st);
    if url != expected {
        return Err(format!("{url}  !=  {expected}"));
    }
    keys.id(&format!("{k}={seg}.{ext}")).ok_or_else(|| "entry missing from dictionary".to_string())
}

fn images_from(v: Option<Value>, side: &str, id: &Id, keys: &Dict, stats: &mut Stats) -> Images {
    let Some(v) = v else { return Images::None };
    if v.is_null() {
        return Images::None;
    }
    let Some(m) = v.as_object() else {
        stats.images_raw += 1;
        return Images::Raw(serde_json::to_string(&v).unwrap());
    };
    let (mut stamp, mut ids, mut ok) = (None, Vec::new(), true);
    for (k, u) in m {
        match check_image(k, u, side, id, keys, &mut stamp) {
            Ok(i) => ids.push(i),
            Err(why) => {
                ok = false;
                *stats.image_mismatches.entry(format!("[{k}] {}", signature(&why, id))).or_default() += 1;
            }
        }
    }
    if ok {
        stats.images_canonical += 1;
        Images::Canonical { keys: ids, stamp: stamp.unwrap_or(0) }
    } else {
        stats.images_raw += 1;
        Images::Raw(serde_json::to_string(&v).unwrap())
    }
}

fn printing_from_card(
    card: &Obj,
    id: Id,
    oracle_row: u32,
    o: &OracleRecord,
    meta: &Meta,
    stats: &mut Stats,
) -> PrintingRecord {
    let mut rest = card.clone();
    let mut spill = Obj::new();
    for k in [
        "id", "oracle_id", "name", "mana_cost", "type_line", "oracle_text", "colors", "color_identity", "cmc",
    ] {
        rest.remove(k);
    }
    for k in ORACLE_EXTRA_KEYS {
        rest.remove(*k);
    }
    let layout = take_dict(&mut rest, &mut spill, "layout", &meta.layouts);
    let lang = take_dict(&mut rest, &mut spill, "lang", &meta.langs);
    let released_at = match rest.remove("released_at") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => match s.replace('-', "").parse::<u32>() {
            Ok(n) if released_to_string(n) == s => Some(n),
            _ => {
                spill.insert("released_at".into(), Value::from(s));
                None
            }
        },
        Some(v) => {
            spill.insert("released_at".into(), v);
            None
        }
    };
    let set_code = take_str(&mut rest, &mut spill, "set");
    let set_name = take_str(&mut rest, &mut spill, "set_name");
    let collector_number = take_str(&mut rest, &mut spill, "collector_number");
    for k in DERIVED_KEYS {
        let want = derived_value(k, &id, &o.oracle_id, set_code.as_deref(), collector_number.as_deref(), o.name.as_deref());
        match (rest.get(k), want) {
            (Some(Value::String(have)), Some(want)) if *have == want => {
                rest.remove(k);
                stats.derived.entry(k.to_string()).or_default().0 += 1;
            }
            (Some(_), _) => stats.derived.entry(k.to_string()).or_default().1 += 1,
            _ => {}
        }
    }
    let rarity = take_dict(&mut rest, &mut spill, "rarity", &meta.rarities);
    let border_color = take_dict(&mut rest, &mut spill, "border_color", &meta.border_colors);
    let promo = take_bool(&mut rest, &mut spill, "promo");
    let reprint = take_bool(&mut rest, &mut spill, "reprint");
    let digital = take_bool(&mut rest, &mut spill, "digital");
    let finishes = match rest.remove("finishes") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let mask = v.as_array().and_then(|a| {
                let mut m = 0u8;
                for x in a {
                    m |= 1 << FINISH_NAMES.iter().position(|n| Some(*n) == x.as_str())?;
                }
                Some(m)
            });
            let ok = mask.filter(|m| {
                let canon: Vec<Value> = FINISH_NAMES
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| m & (1 << i) != 0)
                    .map(|(_, n)| Value::from(*n))
                    .collect();
                Value::Array(canon) == v
            });
            if ok.is_none() {
                spill.insert("finishes".into(), v);
            }
            ok
        }
    };
    let promo_types = match rest.remove("promo_types") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let ids: Option<Vec<u32>> = v
                .as_array()
                .and_then(|a| a.iter().map(|x| x.as_str().and_then(|s| meta.promo_types.id(s))).collect());
            if ids.is_none() {
                spill.insert("promo_types".into(), v);
            }
            ids
        }
    };
    let legalities = legalities_of(&mut rest, &mut spill, &meta.formats);
    let flavor_text = take_str(&mut rest, &mut spill, "flavor_text");
    let artist = take_str(&mut rest, &mut spill, "artist");
    let images = images_from(rest.remove("image_uris"), "front", &id, &meta.image_keys, stats);
    let faces = match rest.remove("card_faces") {
        Some(Value::Array(fs)) if fs.iter().all(Value::is_object) => Some(
            fs.into_iter()
                .enumerate()
                .map(|(i, f)| {
                    let Value::Object(mut f) = f else { unreachable!() };
                    let mut fspill = Obj::new();
                    for k in ["name", "mana_cost", "type_line", "oracle_text"] {
                        f.remove(k);
                    }
                    for k in ORACLE_FACE_EXTRA_KEYS {
                        f.remove(*k);
                    }
                    let flavor_text = take_str(&mut f, &mut fspill, "flavor_text");
                    let artist = take_str(&mut f, &mut fspill, "artist");
                    let side = if i == 0 { "front" } else { "back" };
                    let images = images_from(f.remove("image_uris"), side, &id, &meta.image_keys, stats);
                    for (k, v) in f {
                        fspill.insert(k, v);
                    }
                    PrintingFace { flavor_text, artist, images, extra: to_text(fspill) }
                })
                .collect(),
        ),
        Some(Value::Null) | None => None,
        Some(v) => {
            spill.insert("card_faces".into(), v);
            None
        }
    };
    for (k, v) in rest {
        spill.insert(k, v); // every key we do not model, including future ones
    }
    PrintingRecord {
        tombstone: false,
        scryfall_id: id,
        oracle_row,
        layout,
        lang,
        released_at,
        set_code,
        set_name,
        collector_number,
        rarity,
        border_color,
        promo,
        reprint,
        digital,
        finishes,
        promo_types,
        legalities,
        flavor_text,
        artist,
        images,
        faces,
        extra: to_text(spill),
    }
}

fn example(o: &Obj) -> String {
    format!(
        "{} [{}]",
        o.get("name").and_then(Value::as_str).unwrap_or("?"),
        o.get("id").and_then(Value::as_str).unwrap_or("?")
    )
}

/// Guarantees losslessness: if the typed reconstruction differs from the input
/// on any top-level key, the input's value for that key is stored in `extra`
/// (and keys that should not exist are listed under `__absent`).
struct Cand {
    digest: [u8; 8],
    count: u32,
    key: (u32, Id),
    card: Obj,
}

/// Hash of everything that lands on the ORACLE record, to group printings that agree.
fn oracle_digest(card: &Obj) -> [u8; 8] {
    let mut m = Obj::new();
    for k in ["name", "mana_cost", "type_line", "oracle_text", "colors", "color_identity", "cmc"]
        .iter()
        .chain(ORACLE_EXTRA_KEYS.iter())
    {
        if let Some(v) = card.get(*k) {
            m.insert(k.to_string(), v.clone());
        }
    }
    if let Some(Value::Array(fs)) = card.get("card_faces") {
        let faces: Vec<Value> = fs
            .iter()
            .map(|f| {
                let mut fm = Obj::new();
                if let Some(o) = f.as_object() {
                    for k in ["name", "mana_cost", "type_line", "oracle_text"].iter().chain(ORACLE_FACE_EXTRA_KEYS.iter()) {
                        if let Some(v) = o.get(*k) {
                            fm.insert(k.to_string(), v.clone());
                        }
                    }
                }
                Value::Object(fm)
            })
            .collect();
        m.insert("card_faces".into(), Value::Array(faces));
    }
    Sha256::digest(serde_json::to_string(&Value::Object(m)).unwrap().as_bytes())[..8].try_into().unwrap()
}

fn heal(meta: &Meta, o: &OracleRecord, ranks: &Ranks, p: &mut PrintingRecord, orig: &Obj, stats: &mut Stats) -> bool {
    let mut orig = Value::Object(orig.clone());
    normalize(&mut orig);
    let mut recon = card_json(meta, o, ranks, p);
    normalize(&mut recon);
    if recon == orig {
        return true;
    }
    let (Value::Object(om), Value::Object(rm)) = (&orig, &recon) else { return false };
    let mut extra: Obj = match p.extra.as_deref().map(serde_json::from_str::<Value>) {
        Some(Ok(Value::Object(m))) => m,
        _ => Obj::new(),
    };
    let mut absent: Vec<Value> = Vec::new();
    for (k, v) in om {
        if rm.get(k) != Some(v) {
            extra.insert(k.clone(), v.clone());
            *stats.healed_keys.entry(k.clone()).or_default() += 1;
            stats.healed_examples.entry(k.clone()).or_insert_with(|| example(om));
        }
    }
    for k in rm.keys() {
        if !om.contains_key(k) {
            absent.push(Value::from(k.as_str()));
            *stats.healed_keys.entry(format!("-{k}")).or_default() += 1;
            stats.healed_examples.entry(format!("-{k}")).or_insert_with(|| example(om));
        }
    }
    if !absent.is_empty() {
        extra.insert("__absent".into(), Value::Array(absent));
    }
    p.extra = to_text(extra);
    stats.healed_printings += 1;
    let mut again = card_json(meta, o, ranks, p);
    normalize(&mut again);
    again == orig
}

// --------------------------------------------------------------- compile ---

fn assign_rows<K: Ord>(prev: &[Id], current: &BTreeSet<Id>, key: impl Fn(&Id) -> K) -> (Vec<Id>, HashMap<Id, u32>) {
    let mut rows: Vec<Id> = prev.to_vec();
    let known: BTreeSet<Id> = prev.iter().copied().collect();
    let mut fresh: Vec<Id> = current.iter().filter(|i| !known.contains(*i)).copied().collect();
    fresh.sort_by_cached_key(|id| (key(id), *id));
    rows.extend(fresh);
    let map = rows.iter().enumerate().map(|(i, id)| (*id, i as u32)).collect();
    (rows, map)
}

fn extend_dict(base: &Dict, vocab: &BTreeSet<String>) -> Dict {
    let mut d = base.clone();
    for s in vocab {
        d.push(s);
    }
    d
}

pub fn compile(input: &Path, prev: Option<&Dataset>, opts: &Options) -> Result<(Dataset, Stats), String> {
    let mut stats = Stats::default();
    let prog = Progress::new(opts.progress);
    prog.say(&format!("pass 1/2: reading {}", input.display()));
    let mut seen = 0u64;
    let drop_keys: Vec<&str> = if opts.keep_prices { vec![] } else { vec!["prices"] };

    // ---- pass 1: vocabularies, oracle representatives, id sets
    // slots: 0 layout, 1 rarity, 2 border_color, 3 lang, 4 promo_types, 5 image keys, 6 formats
    let mut vocab: [BTreeSet<String>; 7] = Default::default();
    let mut cands: BTreeMap<Id, Vec<Cand>> = BTreeMap::new();
    let mut printing_ids: BTreeSet<Id> = BTreeSet::new();
    let mut order_keys: HashMap<Id, (String, u32, String)> = HashMap::new();
    let (lines, errs) = for_each_card(input, |mut card| {
        seen += 1;
        prog.tick("pass 1/2 reading", seen, None);
        let a = match accept(&card, opts) {
            Ok(a) => a,
            Err(Skip::Digital) => return stats.skipped_digital += 1,
            Err(Skip::ArtSeries) => return stats.skipped_art_series += 1,
            Err(Skip::NoOracleId) => return stats.skipped_no_oracle_id += 1,
            Err(Skip::BadId) => return stats.skipped_bad_id += 1,
        };
        for k in &drop_keys {
            card.remove(*k);
        }
        if !printing_ids.insert(a.id) {
            stats.duplicate_ids += 1;
        }
        if opts.row_order == RowOrder::Set {
            let set = card.get("set").and_then(Value::as_str).unwrap_or("").to_string();
            let cn = card.get("collector_number").and_then(Value::as_str).unwrap_or("").to_string();
            let num = cn.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(u32::MAX);
            order_keys.insert(a.id, (set, num, cn));
        }
        for (slot, key) in [(0, "layout"), (1, "rarity"), (2, "border_color"), (3, "lang")] {
            if let Some(s) = card.get(key).and_then(Value::as_str) {
                vocab[slot].insert(s.to_string());
            }
        }
        if let Some(Value::Array(arr)) = card.get("promo_types") {
            vocab[4].extend(arr.iter().filter_map(Value::as_str).map(String::from));
        }
        if let Some(Value::Object(m)) = card.get("image_uris") {
            add_image_entries(&mut vocab[5], m);
        }
        if let Some(Value::Array(fs)) = card.get("card_faces") {
            for f in fs {
                if let Some(Value::Object(m)) = f.get("image_uris") {
                    add_image_entries(&mut vocab[5], m);
                }
            }
        }
        if let Some(Value::Object(l)) = card.get("legalities") {
            vocab[6].extend(l.keys().cloned());
        }
        let key = (released_num(&card), a.id);
        let digest = oracle_digest(&card);
        let list = cands.entry(a.oracle_id).or_default();
        match list.iter_mut().find(|c| c.digest == digest) {
            Some(c) => {
                c.count += 1;
                c.key = c.key.max(key);
            }
            None => list.push(Cand { digest, count: 1, key, card }),
        }
    })?;
    // The oracle record comes from the version most printings agree on (ties:
    // the newest), so only genuine outliers (e.g. reversible printings) need healing.
    let best: BTreeMap<Id, Obj> = cands
        .into_iter()
        .map(|(id, v)| {
            let c = v.into_iter().max_by(|a, b| (a.count, a.key).cmp(&(b.count, b.key))).unwrap();
            (id, c.card)
        })
        .collect();
    stats.lines = lines;
    stats.parse_errors = errs;
    prog.say(&format!("pass 1 done: {} printings, {} oracle cards; assigning row ids", printing_ids.len(), best.len()));

    // ---- dictionaries and stable row ids
    let use_prev = prev.filter(|_| !opts.rebase);
    let base = use_prev.map(|p| p.meta.clone()).unwrap_or_default();
    let meta = Meta {
        cards_epoch: match prev {
            None => 1,
            Some(p) if opts.rebase => p.meta.cards_epoch + 1,
            Some(p) => p.meta.cards_epoch,
        },
        source_date: opts.source_date,
        layouts: extend_dict(&base.layouts, &vocab[0]),
        rarities: extend_dict(&base.rarities, &vocab[1]),
        border_colors: extend_dict(&base.border_colors, &vocab[2]),
        langs: extend_dict(&base.langs, &vocab[3]),
        promo_types: extend_dict(&base.promo_types, &vocab[4]),
        image_keys: extend_dict(&base.image_keys, &vocab[5]),
        formats: extend_dict(&base.formats, &vocab[6]),
        dropped_keys: drop_keys.iter().map(|s| s.to_string()).collect(),
    };
    let prev_oracle: Vec<Id> = use_prev.map(|p| p.oracle.iter().map(|r| r.oracle_id).collect()).unwrap_or_default();
    let prev_print: Vec<Id> =
        use_prev.map(|p| p.printings.iter().map(|r| r.scryfall_id).collect()).unwrap_or_default();
    let live_oracle: BTreeSet<Id> = best.keys().copied().collect();
    let (oracle_rows, oracle_map) = assign_rows(&prev_oracle, &live_oracle, |_| ());
    let (print_rows, print_map) = match opts.row_order {
        RowOrder::Uuid => assign_rows(&prev_print, &printing_ids, |_| (String::new(), 0u32, String::new())),
        RowOrder::Set => assign_rows(&prev_print, &printing_ids, |id| order_keys.get(id).cloned().unwrap_or_default()),
    };
    stats.new_oracle = (oracle_rows.len() - prev_oracle.len()) as u64;
    stats.new_printings = (print_rows.len() - prev_print.len()) as u64;

    let (oracle, ranks): (Vec<OracleRecord>, Vec<Ranks>) = oracle_rows
        .iter()
        .map(|id| match best.get(id) {
            Some(card) => oracle_from_card(card, *id),
            None => (OracleRecord::tombstone(*id), Ranks::default()),
        })
        .unzip();
    stats.oracle_cards = live_oracle.len() as u64;
    stats.tombstoned_oracle = oracle_rows.len() as u64 - stats.oracle_cards;
    drop(best);

    // ---- pass 2: printing records, verified against the original JSON
    let mut printings: Vec<PrintingRecord> = print_rows.iter().map(|id| PrintingRecord::tombstone(*id)).collect();
    let mut done: BTreeSet<Id> = BTreeSet::new();
    let mut unhealed = 0u64;
    let mut local = Stats::default();
    let total = printing_ids.len() as u64;
    prog.say(&format!("pass 2/2: building and verifying {total} printings"));
    for_each_card(input, |mut card| {
        let Ok(a) = accept(&card, opts) else { return };
        for k in &drop_keys {
            card.remove(*k);
        }
        if !done.insert(a.id) {
            return; // duplicate id: first occurrence wins (counted in pass 1)
        }
        let orow = oracle_map[&a.oracle_id];
        let mut p = printing_from_card(&card, a.id, orow, &oracle[orow as usize], &meta, &mut local);
        if !heal(&meta, &oracle[orow as usize], &ranks[orow as usize], &mut p, &card, &mut local) {
            unhealed += 1;
        }
        if let Some(e) = &p.extra {
            if let Ok(Value::Object(m)) = serde_json::from_str::<Value>(e) {
                for (k, v) in m {
                    let n = k.len() + serde_json::to_string(&v).map_or(0, |s| s.len());
                    local.extra_examples.entry(k.clone()).or_insert_with(|| {
                        let t = serde_json::to_string(&v).unwrap_or_default();
                        t.chars().take(400).collect()
                    });
                    *local.extra_bytes_by_key.entry(k).or_default() += n as u64;
                }
            }
        }
        printings[print_map[&a.id] as usize] = p;
        prog.tick("pass 2/2 printings", done.len() as u64, Some(total));
    })?;
    prog.say("compile finished");
    if unhealed > 0 {
        return Err(format!("{unhealed} printings failed the lossless round-trip check"));
    }
    stats.images_canonical = local.images_canonical;
    stats.images_raw = local.images_raw;
    stats.healed_printings = local.healed_printings;
    stats.healed_keys = local.healed_keys;
    stats.derived = local.derived;
    stats.healed_examples = local.healed_examples;
    stats.image_mismatches = local.image_mismatches;
    stats.extra_examples = local.extra_examples;
    stats.extra_bytes_by_key = local.extra_bytes_by_key;
    stats.printings = done.len() as u64;
    stats.tombstoned_printings = print_rows.len() as u64 - stats.printings;

    Ok((Dataset { meta, oracle, ranks, printings }, stats))
}

/// Reconstructs every live card and compares it with `input` (after dropping
/// `meta.dropped_keys` and `null` values). Returns (checked, mismatched).
pub fn verify(ds: &Dataset, input: &Path) -> Result<(u64, u64), String> {
    let pmap: HashMap<Id, usize> = ds.printings.iter().enumerate().map(|(i, p)| (p.scryfall_id, i)).collect();
    let (mut checked, mut bad) = (0u64, 0u64);
    let mut seen: BTreeSet<Id> = BTreeSet::new();
    for_each_card(input, |mut card| {
        let Ok(a) = accept(&card, &Options::default()) else { return };
        if !seen.insert(a.id) {
            return;
        }
        // datasets compiled with --skip-digital / --skip-art-series legitimately lack these
        let optional = card.get("digital").and_then(Value::as_bool) == Some(true) || card.get("layout").and_then(Value::as_str) == Some("art_series");
        if optional && !pmap.contains_key(&a.id) {
            return;
        }
        for k in &ds.meta.dropped_keys {
            card.remove(k);
        }
        let mut orig = Value::Object(card);
        normalize(&mut orig);
        checked += 1;
        let ok = pmap.get(&a.id).is_some_and(|&i| {
            let p = &ds.printings[i];
            if p.tombstone {
                return false;
            }
            let mut r = card_json(&ds.meta, &ds.oracle[p.oracle_row as usize], &ds.ranks[p.oracle_row as usize], p);
            normalize(&mut r);
            r == orig
        });
        if !ok {
            bad += 1;
        }
    })?;
    Ok((checked, bad))
}

#[derive(Clone, Debug, Default)]
pub struct KeyStat {
    pub raw: u64,
    pub stored: u64,
    /// Oracle cards with 2+ printings having this key on one (top keys only).
    pub oracles_with: u64,
    /// ...of which the value is NOT identical across all their printings.
    pub oracles_varied: u64,
}

/// Per residual key: raw bytes and compressed bytes, compressing each key's values
/// separately in 256-row groups like the real blocks. Only an estimate of how
/// much each key contributes to the stored size; it ignores cross-key sharing.
/// For the 12 most expensive keys it also measures how many oracle cards have the
/// same value on every printing (candidates to store once on the oracle record).
pub fn breakdown(ds: &Dataset) -> Result<BTreeMap<String, KeyStat>, String> {
    let level = coqc_format::container::ZSTD_LEVEL;
    let mut bufs: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut total: BTreeMap<String, KeyStat> = BTreeMap::new();
    let mut flush = |bufs: &mut BTreeMap<String, Vec<u8>>| -> Result<(), String> {
        for (k, b) in bufs.iter_mut() {
            if b.is_empty() {
                continue;
            }
            let c = zstd::bulk::compress(b, level).map_err(|e| e.to_string())?;
            let t = total.entry(k.clone()).or_default();
            t.raw += b.len() as u64;
            t.stored += c.len() as u64;
            b.clear();
        }
        Ok(())
    };
    let parse = |p: &PrintingRecord| -> Option<Obj> {
        match p.extra.as_deref().map(serde_json::from_str::<Value>) {
            Some(Ok(Value::Object(m))) => Some(m),
            _ => None,
        }
    };
    for (i, p) in ds.printings.iter().enumerate() {
        if let Some(m) = parse(p) {
            for (k, v) in m {
                let b = bufs.entry(k).or_default();
                b.extend_from_slice(serde_json::to_string(&v).unwrap_or_default().as_bytes());
                b.push(b'\n');
            }
        }
        if (i + 1) % 256 == 0 {
            flush(&mut bufs)?;
        }
    }
    flush(&mut bufs)?;

    let mut top: Vec<(String, u64)> = total.iter().map(|(k, v)| (k.clone(), v.stored)).collect();
    top.sort_by(|a, b| b.1.cmp(&a.1));
    let top: Vec<String> = top.into_iter().take(12).map(|(k, _)| k).collect();
    // Only oracle cards with 2+ live printings: a single printing is trivially "identical".
    let mut per_oracle: HashMap<u32, u32> = HashMap::new();
    for p in ds.printings.iter().filter(|p| !p.tombstone) {
        *per_oracle.entry(p.oracle_row).or_default() += 1;
    }
    let mut seen: HashMap<(u32, u8), (u64, bool)> = HashMap::new();
    for p in ds.printings.iter().filter(|p| !p.tombstone && per_oracle[&p.oracle_row] >= 2) {
        let Some(m) = parse(p) else { continue };
        for (ki, k) in top.iter().enumerate() {
            if let Some(v) = m.get(k) {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                serde_json::to_string(v).unwrap_or_default().hash(&mut h);
                let h = h.finish();
                let e = seen.entry((p.oracle_row, ki as u8)).or_insert((h, false));
                if e.0 != h {
                    e.1 = true;
                }
            }
        }
    }
    for ((_, ki), (_, varied)) in &seen {
        let t = total.get_mut(&top[*ki as usize]).unwrap();
        t.oracles_with += 1;
        t.oracles_varied += *varied as u64;
    }
    Ok(total)
}

/// What changed between two releases, per key (typed field names and residual keys alike).
#[derive(Clone, Debug, Default)]
pub struct KeyDiff {
    pub rows_old: u64,
    pub rows_new: u64,
    pub rows_changed: u64,
    pub became_tombstone: u64,
    pub revived: u64,
    pub by_key: BTreeMap<String, u64>,
}

fn add_extra(m: &mut BTreeMap<String, String>, extra: &Option<String>) {
    if let Some(Ok(Value::Object(o))) = extra.as_deref().map(serde_json::from_str::<Value>) {
        for (k, v) in o {
            m.insert(k, v.to_string());
        }
    }
}

fn flatten_oracle(o: &OracleRecord) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for (k, v) in [
        ("name", format!("{:?}", o.name)),
        ("mana_cost", format!("{:?}", o.mana_cost)),
        ("type_line", format!("{:?}", o.type_line)),
        ("oracle_text", format!("{:?}", o.oracle_text)),
        ("colors", format!("{:?}", o.colors)),
        ("color_identity", format!("{:?}", o.color_identity)),
        ("cmc", format!("{:?}", o.cmc)),
        ("card_faces", format!("{:?}", o.faces)),
    ] {
        m.insert(k.to_string(), v);
    }
    add_extra(&mut m, &o.extra);
    m
}

fn flatten_printing(p: &PrintingRecord) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for (k, v) in [
        ("(oracle_row)", format!("{:?}", p.oracle_row)),
        ("layout", format!("{:?}", p.layout)),
        ("lang", format!("{:?}", p.lang)),
        ("released_at", format!("{:?}", p.released_at)),
        ("set", format!("{:?}", p.set_code)),
        ("set_name", format!("{:?}", p.set_name)),
        ("collector_number", format!("{:?}", p.collector_number)),
        ("rarity", format!("{:?}", p.rarity)),
        ("border_color", format!("{:?}", p.border_color)),
        ("promo/reprint/digital", format!("{:?}", (p.promo, p.reprint, p.digital))),
        ("finishes", format!("{:?}", p.finishes)),
        ("promo_types", format!("{:?}", p.promo_types)),
        ("legalities", format!("{:?}", p.legalities)),
        ("flavor_text", format!("{:?}", p.flavor_text)),
        ("artist", format!("{:?}", p.artist)),
        ("(images: urls/stamps)", format!("{:?}", p.images)),
        ("(card_faces: printing part)", format!("{:?}", p.faces)),
    ] {
        m.insert(k.to_string(), v);
    }
    add_extra(&mut m, &p.extra);
    m
}

fn diff_rows<T>(old: &[T], new: &[T], tomb: impl Fn(&T) -> bool, flat: impl Fn(&T) -> BTreeMap<String, String>) -> KeyDiff {
    let mut d = KeyDiff { rows_old: old.len() as u64, rows_new: new.len() as u64, ..Default::default() };
    for (a, b) in old.iter().zip(new) {
        match (tomb(a), tomb(b)) {
            (false, true) => d.became_tombstone += 1,
            (true, false) => d.revived += 1,
            (true, true) => {}
            (false, false) => {
                let (fa, fb) = (flat(a), flat(b));
                let mut changed = false;
                for k in fa.keys().chain(fb.keys()).collect::<BTreeSet<_>>() {
                    if fa.get(k) != fb.get(k) {
                        *d.by_key.entry(k.clone()).or_default() += 1;
                        changed = true;
                    }
                }
                d.rows_changed += changed as u64;
            }
        }
    }
    d
}

/// Compares two releases row by row (rows are stable across releases) and counts which keys changed.
/// The third value is how many oracle rows changed in the separate ranks table.
pub fn diff_keys(old: &Dataset, new: &Dataset) -> (KeyDiff, KeyDiff, u64) {
    let ranks = old.ranks.iter().zip(&new.ranks).filter(|(a, b)| a != b).count() as u64;
    (
        diff_rows(&old.oracle, &new.oracle, |r| r.tombstone, flatten_oracle),
        diff_rows(&old.printings, &new.printings, |r| r.tombstone, flatten_printing),
        ranks,
    )
}
