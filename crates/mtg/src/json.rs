//! Rebuild Scryfall-shaped card JSON from an oracle + printing record pair.
//! The compiler uses this as a self-check; engines use it to serve full cards.

use coqc_format::types::*;
use serde_json::{Map, Value};

/// Top-level keys carried on the ORACLE record's `extra` (rules-level data).
pub const ORACLE_EXTRA_KEYS: &[&str] = &[
    "power",
    "toughness",
    "loyalty",
    "defense",
    "color_indicator",
    "keywords",
    "produced_mana",
    "reserved",
    "game_changer",
    "edhrec_rank",
    "penny_rank",
    "hand_modifier",
    "life_modifier",
    "attraction_lights",
];

/// Face keys carried on the ORACLE face's `extra`.
pub const ORACLE_FACE_EXTRA_KEYS: &[&str] = &[
    "power",
    "toughness",
    "loyalty",
    "defense",
    "colors",
    "color_indicator",
    "oracle_id",
    "cmc",
];

pub const FINISH_NAMES: [&str; 3] = ["nonfoil", "foil", "etched"];
pub const COLOR_LETTERS: [&str; 5] = ["W", "U", "B", "R", "G"];
pub const LEGALITY_NAMES: [&str; 5] = ["", "legal", "not_legal", "restricted", "banned"];

/// Keys whose value Scryfall derives from other fields; the compiler stores them only when
/// the real value differs from what `derived_value` produces.
pub const DERIVED_KEYS: [&str; 4] = ["uri", "rulings_uri", "prints_search_uri", "scryfall_uri"];

/// Scryfall-style URL slug: lowercase ASCII letters/digits, apostrophes dropped, other runs -> `-`.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in name.chars() {
        if c == '\'' || c == '\u{2019}' {
            continue;
        }
        if c.is_ascii_alphanumeric() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(c.to_ascii_lowercase());
        } else {
            dash = true;
        }
    }
    out
}

pub fn derived_value(
    key: &str,
    id: &[u8; 16],
    oracle_id: &[u8; 16],
    set: Option<&str>,
    collector_number: Option<&str>,
    name: Option<&str>,
) -> Option<String> {
    let id = uuid_to_string(id);
    Some(match key {
        "uri" => format!("https://api.scryfall.com/cards/{id}"),
        "rulings_uri" => format!("https://api.scryfall.com/cards/{id}/rulings"),
        "prints_search_uri" => format!(
            "https://api.scryfall.com/cards/search?order=released&q=oracleid%3A{}&unique=prints",
            uuid_to_string(oracle_id)
        ),
        "scryfall_uri" => {
            format!("https://scryfall.com/card/{}/{}/{}?utm_source=api", set?, collector_number?, slug(name?))
        }
        _ => return None,
    })
}

pub fn uuid_to_string(u: &[u8; 16]) -> String {
    let h: String = u.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

pub fn parse_uuid(s: &str) -> Option<[u8; 16]> {
    let hex: Vec<u8> = s.bytes().filter(|b| *b != b'-').collect();
    if hex.len() != 32 || s.len() != 36 {
        return None;
    }
    let mut out = [0u8; 16];
    for i in 0..16 {
        let hi = (hex[i * 2] as char).to_digit(16)?;
        let lo = (hex[i * 2 + 1] as char).to_digit(16)?;
        out[i] = (hi * 16 + lo) as u8;
    }
    Some(out)
}

pub fn colors_to_json(mask: u8) -> Value {
    Value::Array(
        COLOR_LETTERS
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .map(|(_, c)| Value::from(*c))
            .collect(),
    )
}

pub fn released_to_string(v: u32) -> String {
    format!("{:04}-{:02}-{:02}", v / 10000, v / 100 % 100, v % 100)
}

/// `https://cards.scryfall.io/{key}/{side}/{a}/{b}/{id}.{ext}?{stamp}`
pub fn image_url(key: &str, ext: &str, side: &str, id: &[u8; 16], stamp: u32) -> String {
    let s = uuid_to_string(id);
    format!(
        "https://cards.scryfall.io/{key}/{side}/{}/{}/{s}.{ext}?{stamp}",
        &s[0..1],
        &s[1..2]
    )
}

fn images_to_json(meta: &Meta, img: &Images, side: &str, id: &[u8; 16]) -> Option<Value> {
    match img {
        Images::None => None,
        Images::Canonical { keys, stamp } => {
            let mut m = Map::new();
            for k in keys {
                // entry = `key=segment.ext`
                let (key, rest) = meta.image_keys.get(*k)?.split_once('=')?;
                let (seg, ext) = rest.rsplit_once('.')?;
                m.insert(key.to_string(), Value::from(image_url(seg, ext, side, id, *stamp)));
            }
            Some(Value::Object(m))
        }
        Images::Raw(s) => serde_json::from_str(s).ok(),
    }
}

fn merge_extra(m: &mut Map<String, Value>, extra: &Option<String>) {
    if let Some(s) = extra {
        if let Ok(Value::Object(o)) = serde_json::from_str::<Value>(s) {
            for (k, v) in o {
                m.insert(k, v);
            }
        }
    }
}

fn put_str(m: &mut Map<String, Value>, k: &str, v: &Option<String>) {
    if let Some(s) = v {
        m.insert(k.to_string(), Value::from(s.as_str()));
    }
}

/// Rebuild the card object. `null` values are never emitted (absent == null).
pub fn card_json(meta: &Meta, o: &OracleRecord, ranks: &Ranks, p: &PrintingRecord) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), Value::from(uuid_to_string(&p.scryfall_id)));
    m.insert("oracle_id".into(), Value::from(uuid_to_string(&o.oracle_id)));
    put_str(&mut m, "name", &o.name);
    put_str(&mut m, "mana_cost", &o.mana_cost);
    put_str(&mut m, "type_line", &o.type_line);
    put_str(&mut m, "oracle_text", &o.oracle_text);
    if let Some(c) = o.colors {
        m.insert("colors".into(), colors_to_json(c));
    }
    if let Some(c) = o.color_identity {
        m.insert("color_identity".into(), colors_to_json(c));
    }
    if let Some(c) = o.cmc {
        if let Some(n) = serde_json::Number::from_f64(c) {
            m.insert("cmc".into(), Value::Number(n));
        }
    }
    merge_extra(&mut m, &o.extra);
    if let Some(v) = ranks.edhrec {
        m.insert("edhrec_rank".into(), Value::from(v));
    }
    if let Some(v) = ranks.penny {
        m.insert("penny_rank".into(), Value::from(v));
    }

    if let Some(i) = p.layout {
        if let Some(s) = meta.layouts.get(i) {
            m.insert("layout".into(), Value::from(s));
        }
    }
    if let Some(i) = p.lang {
        if let Some(s) = meta.langs.get(i) {
            m.insert("lang".into(), Value::from(s));
        }
    }
    if let Some(d) = p.released_at {
        m.insert("released_at".into(), Value::from(released_to_string(d)));
    }
    put_str(&mut m, "set", &p.set_code);
    put_str(&mut m, "set_name", &p.set_name);
    put_str(&mut m, "collector_number", &p.collector_number);
    if let Some(s) = p.rarity.and_then(|i| meta.rarities.get(i)) {
        m.insert("rarity".into(), Value::from(s));
    }
    if let Some(s) = p.border_color.and_then(|i| meta.border_colors.get(i)) {
        m.insert("border_color".into(), Value::from(s));
    }
    for (k, v) in [("promo", p.promo), ("reprint", p.reprint), ("digital", p.digital)] {
        if let Some(b) = v {
            m.insert(k.into(), Value::from(b));
        }
    }
    if let Some(f) = p.finishes {
        let arr = FINISH_NAMES
            .iter()
            .enumerate()
            .filter(|(i, _)| f & (1 << i) != 0)
            .map(|(_, n)| Value::from(*n))
            .collect();
        m.insert("finishes".into(), Value::Array(arr));
    }
    if let Some(l) = &p.legalities {
        let mut lm = Map::new();
        for (i, v) in l.iter().enumerate() {
            if *v != 0 {
                if let Some(name) = meta.formats.get(i as u32) {
                    lm.insert(name.to_string(), Value::from(LEGALITY_NAMES[*v as usize % 5]));
                }
            }
        }
        m.insert("legalities".into(), Value::Object(lm));
    }
    if let Some(pt) = &p.promo_types {
        let arr = pt
            .iter()
            .filter_map(|i| meta.promo_types.get(*i))
            .map(Value::from)
            .collect();
        m.insert("promo_types".into(), Value::Array(arr));
    }
    put_str(&mut m, "flavor_text", &p.flavor_text);
    put_str(&mut m, "artist", &p.artist);
    if let Some(v) = images_to_json(meta, &p.images, "front", &p.scryfall_id) {
        m.insert("image_uris".into(), v);
    }
    for k in DERIVED_KEYS {
        let d = derived_value(k, &p.scryfall_id, &o.oracle_id, p.set_code.as_deref(), p.collector_number.as_deref(), o.name.as_deref());
        if let Some(v) = d {
            m.insert(k.to_string(), Value::from(v));
        }
    }

    if let Some(ofs) = &o.faces {
        let pfs = p.faces.as_deref().unwrap_or(&[]);
        let mut arr = Vec::new();
        for (i, of) in ofs.iter().enumerate() {
            let mut f = Map::new();
            put_str(&mut f, "name", &of.name);
            put_str(&mut f, "mana_cost", &of.mana_cost);
            put_str(&mut f, "type_line", &of.type_line);
            put_str(&mut f, "oracle_text", &of.oracle_text);
            merge_extra(&mut f, &of.extra);
            if let Some(pf) = pfs.get(i) {
                put_str(&mut f, "flavor_text", &pf.flavor_text);
                put_str(&mut f, "artist", &pf.artist);
                let side = if i == 0 { "front" } else { "back" };
                if let Some(v) = images_to_json(meta, &pf.images, side, &p.scryfall_id) {
                    f.insert("image_uris".into(), v);
                }
                merge_extra(&mut f, &pf.extra);
            }
            arr.push(Value::Object(f));
        }
        m.insert("card_faces".into(), Value::Array(arr));
    }

    merge_extra(&mut m, &p.extra);
    if let Some(Value::Array(absent)) = m.remove("__absent") {
        for k in absent {
            if let Some(k) = k.as_str() {
                m.remove(k);
            }
        }
    }
    Value::Object(m)
}

/// Recursively drop `null` object entries so "absent" and "null" compare equal.
pub fn normalize(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.retain(|_, x| !x.is_null());
            for x in m.values_mut() {
                normalize(x);
            }
        }
        Value::Array(a) => {
            for x in a {
                normalize(x);
            }
        }
        _ => {}
    }
}
