//! Record types. A card is split into an ORACLE record (rules-level data shared
//! by every printing) and a PRINTING record (one per Scryfall card id).
//!
//! Losslessness: fields the engine will index are typed; every other top-level
//! or per-face key is kept in `extra` as compact JSON text (keys sorted), so
//! nothing Scryfall sends is dropped and unknown future keys survive.

use crate::codec::*;
use std::collections::HashMap;

/// Append-only string dictionary. IDs are positions and must never be reordered
/// between releases, otherwise unchanged records would re-encode differently.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct Dict {
    values: Vec<String>,
    index: HashMap<String, u32>,
}

impl Dict {
    pub fn from_values(values: Vec<String>) -> Self {
        let index = values
            .iter()
            .enumerate()
            .map(|(i, s)| (s.clone(), i as u32))
            .collect();
        Self { values, index }
    }
    pub fn id(&self, s: &str) -> Option<u32> {
        self.index.get(s).copied()
    }
    pub fn push(&mut self, s: &str) -> u32 {
        if let Some(i) = self.index.get(s) {
            return *i;
        }
        let i = self.values.len() as u32;
        self.values.push(s.to_string());
        self.index.insert(s.to_string(), i);
        i
    }
    pub fn get(&self, id: u32) -> Option<&str> {
        self.values.get(id as usize).map(|s| s.as_str())
    }
    pub fn len(&self) -> usize {
        self.values.len()
    }
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
    pub fn values(&self) -> &[String] {
        &self.values
    }
    fn encode(&self, w: &mut Writer) {
        w.uvarint(self.values.len() as u64);
        for v in &self.values {
            w.str(v);
        }
    }
    fn decode(r: &mut Reader) -> Result<Self> {
        let n = r.uvarint()? as usize;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(r.string()?);
        }
        Ok(Self::from_values(v))
    }
}

/// Dataset-wide metadata. Dictionaries are append-only across releases.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct Meta {
    pub cards_epoch: u32,
    /// YYYYMMDD of the source bulk file (0 = unknown). Never a wall-clock time,
    /// so identical input always yields identical bytes.
    pub source_date: u32,
    pub layouts: Dict,
    pub rarities: Dict,
    pub border_colors: Dict,
    pub langs: Dict,
    pub promo_types: Dict,
    pub image_keys: Dict,
    pub formats: Dict,
    /// Top-level card keys the compiler intentionally removed (e.g. "prices").
    pub dropped_keys: Vec<String>,
}

impl Meta {
    pub fn encode(&self, w: &mut Writer) {
        w.u32(self.cards_epoch);
        w.u32(self.source_date);
        for d in self.dicts() {
            d.encode(w);
        }
        w.uvarint(self.dropped_keys.len() as u64);
        for k in &self.dropped_keys {
            w.str(k);
        }
    }
    pub fn decode(r: &mut Reader) -> Result<Self> {
        let cards_epoch = r.u32()?;
        let source_date = r.u32()?;
        let layouts = Dict::decode(r)?;
        let rarities = Dict::decode(r)?;
        let border_colors = Dict::decode(r)?;
        let langs = Dict::decode(r)?;
        let promo_types = Dict::decode(r)?;
        let image_keys = Dict::decode(r)?;
        let formats = Dict::decode(r)?;
        let n = r.uvarint()? as usize;
        let mut dropped_keys = Vec::with_capacity(n);
        for _ in 0..n {
            dropped_keys.push(r.string()?);
        }
        Ok(Self {
            cards_epoch,
            source_date,
            layouts,
            rarities,
            border_colors,
            langs,
            promo_types,
            image_keys,
            formats,
            dropped_keys,
        })
    }
    fn dicts(&self) -> [&Dict; 7] {
        [
            &self.layouts,
            &self.rarities,
            &self.border_colors,
            &self.langs,
            &self.promo_types,
            &self.image_keys,
            &self.formats,
        ]
    }
}

/// Volatile per-oracle numbers (they change for most cards every day), kept in a separate
/// small section so the big oracle blocks stay byte-identical between releases.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ranks {
    pub edhrec: Option<u32>,
    pub penny: Option<u32>,
}

pub fn encode_ranks(r: &[Ranks]) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(r.len() as u32);
    for x in r {
        w.opt_id(x.edhrec);
        w.opt_id(x.penny);
    }
    w.buf
}

pub fn decode_ranks(b: &[u8]) -> Result<Vec<Ranks>> {
    let mut r = Reader::new(b);
    let n = r.u32()? as usize;
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        v.push(Ranks { edhrec: r.opt_id()?, penny: r.opt_id()? });
    }
    Ok(v)
}

// ---------------------------------------------------------------- oracle ---

#[derive(Clone, Debug, PartialEq, Default)]
pub struct OracleFace {
    pub name: Option<String>,
    pub mana_cost: Option<String>,
    pub type_line: Option<String>,
    pub oracle_text: Option<String>,
    /// JSON object text of remaining oracle-level face keys (power, colors...).
    pub extra: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OracleRecord {
    pub tombstone: bool,
    pub oracle_id: [u8; 16],
    pub name: Option<String>,
    pub mana_cost: Option<String>,
    pub type_line: Option<String>,
    pub oracle_text: Option<String>,
    /// WUBRG bitmask; None = key absent (Scryfall omits it on multi-faced cards).
    pub colors: Option<u8>,
    pub color_identity: Option<u8>,
    pub cmc: Option<f64>,
    /// None = no `card_faces` key.
    pub faces: Option<Vec<OracleFace>>,
    /// JSON object text of remaining oracle-level keys (power, keywords, ...).
    pub extra: Option<String>,
}

impl OracleRecord {
    pub fn tombstone(oracle_id: [u8; 16]) -> Self {
        Self {
            tombstone: true,
            oracle_id,
            name: None,
            mana_cost: None,
            type_line: None,
            oracle_text: None,
            colors: None,
            color_identity: None,
            cmc: None,
            faces: None,
            extra: None,
        }
    }

    pub fn encode(&self, w: &mut Writer) {
        w.u8(self.tombstone as u8);
        w.bytes(&self.oracle_id);
        if self.tombstone {
            return;
        }
        w.opt_str(self.name.as_deref());
        w.opt_str(self.mana_cost.as_deref());
        w.opt_str(self.type_line.as_deref());
        w.opt_str(self.oracle_text.as_deref());
        w.opt_id(self.colors.map(|c| c as u32));
        w.opt_id(self.color_identity.map(|c| c as u32));
        match self.cmc {
            None => w.u8(0),
            Some(v) => {
                w.u8(1);
                w.u64(v.to_bits());
            }
        }
        encode_oracle_faces(w, &self.faces);
        w.opt_str(self.extra.as_deref());
    }

    pub fn decode(r: &mut Reader) -> Result<Self> {
        let tombstone = r.u8()? != 0;
        let oracle_id = r.uuid()?;
        if tombstone {
            return Ok(Self::tombstone(oracle_id));
        }
        let name = r.opt_string()?;
        let mana_cost = r.opt_string()?;
        let type_line = r.opt_string()?;
        let oracle_text = r.opt_string()?;
        let colors = r.opt_id()?.map(|c| c as u8);
        let color_identity = r.opt_id()?.map(|c| c as u8);
        let cmc = match r.u8()? {
            0 => None,
            _ => Some(f64::from_bits(r.u64()?)),
        };
        let faces = decode_oracle_faces(r)?;
        let extra = r.opt_string()?;
        Ok(Self {
            tombstone,
            oracle_id,
            name,
            mana_cost,
            type_line,
            oracle_text,
            colors,
            color_identity,
            cmc,
            faces,
            extra,
        })
    }
}

/// Field encoders shared with the table model: a dialect stores nested structures as opaque `Bytes` columns using these.
pub fn encode_oracle_faces(w: &mut Writer, faces: &Option<Vec<OracleFace>>) {
    match faces {
        None => w.uvarint(0),
        Some(fs) => {
            w.uvarint(fs.len() as u64 + 1);
            for f in fs {
                w.opt_str(f.name.as_deref());
                w.opt_str(f.mana_cost.as_deref());
                w.opt_str(f.type_line.as_deref());
                w.opt_str(f.oracle_text.as_deref());
                w.opt_str(f.extra.as_deref());
            }
        }
    }
}

pub fn decode_oracle_faces(r: &mut Reader) -> Result<Option<Vec<OracleFace>>> {
    let n = r.uvarint()? as usize;
    if n == 0 {
        return Ok(None);
    }
    let mut fs = Vec::with_capacity(n - 1);
    for _ in 0..n - 1 {
        fs.push(OracleFace {
            name: r.opt_string()?,
            mana_cost: r.opt_string()?,
            type_line: r.opt_string()?,
            oracle_text: r.opt_string()?,
            extra: r.opt_string()?,
        });
    }
    Ok(Some(fs))
}

pub fn encode_printing_faces(w: &mut Writer, faces: &Option<Vec<PrintingFace>>) {
    match faces {
        None => w.uvarint(0),
        Some(fs) => {
            w.uvarint(fs.len() as u64 + 1);
            for f in fs {
                w.opt_str(f.flavor_text.as_deref());
                w.opt_str(f.artist.as_deref());
                f.images.encode(w);
                w.opt_str(f.extra.as_deref());
            }
        }
    }
}

pub fn decode_printing_faces(r: &mut Reader) -> Result<Option<Vec<PrintingFace>>> {
    let n = r.uvarint()? as usize;
    if n == 0 {
        return Ok(None);
    }
    let mut fs = Vec::with_capacity(n - 1);
    for _ in 0..n - 1 {
        fs.push(PrintingFace { flavor_text: r.opt_string()?, artist: r.opt_string()?, images: Images::decode(r)?, extra: r.opt_string()? });
    }
    Ok(Some(fs))
}

// -------------------------------------------------------------- printing ---

/// Image URLs of one face. `Canonical` means every URL equals
/// `https://cards.scryfall.io/{key}/{side}/{id[0]}/{id[1]}/{id}.{ext}?{stamp}`
/// (checked by the compiler), so only the set of `key=segment.ext` dictionary
/// entries (e.g. `normal=normal.jpg`, `jpeg=normal.jpg`, `art=art.webp`) and one stamp are stored.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Images {
    #[default]
    None,
    Canonical { keys: Vec<u32>, stamp: u32 },
    /// JSON object text, for anything that does not match the canonical shape.
    Raw(String),
}

impl Images {
    pub fn encode(&self, w: &mut Writer) {
        match self {
            Images::None => w.u8(0),
            Images::Canonical { keys, stamp } => {
                w.u8(1);
                w.uvarint(keys.len() as u64);
                for k in keys {
                    w.uvarint(*k as u64);
                }
                w.uvarint(*stamp as u64);
            }
            Images::Raw(s) => {
                w.u8(2);
                w.str(s);
            }
        }
    }
    pub fn decode(r: &mut Reader) -> Result<Self> {
        Ok(match r.u8()? {
            0 => Images::None,
            1 => {
                let n = r.uvarint()? as usize;
                let mut keys = Vec::with_capacity(n);
                for _ in 0..n {
                    keys.push(r.uvarint32()?);
                }
                Images::Canonical { keys, stamp: r.uvarint32()? }
            }
            2 => Images::Raw(r.string()?),
            _ => return Err(Error::Corrupt("bad image tag")),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct PrintingFace {
    pub flavor_text: Option<String>,
    pub artist: Option<String>,
    pub images: Images,
    /// JSON object text of remaining printing-level face keys.
    pub extra: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PrintingRecord {
    pub tombstone: bool,
    pub scryfall_id: [u8; 16],
    /// Row of the oracle record (stable, append-only).
    pub oracle_row: u32,
    pub layout: Option<u32>,
    pub lang: Option<u32>,
    /// YYYYMMDD, None = key absent.
    pub released_at: Option<u32>,
    pub set_code: Option<String>,
    pub set_name: Option<String>,
    pub collector_number: Option<String>,
    pub rarity: Option<u32>,
    pub border_color: Option<u32>,
    pub promo: Option<bool>,
    pub reprint: Option<bool>,
    pub digital: Option<bool>,
    /// nonfoil=1 foil=2 etched=4
    pub finishes: Option<u8>,
    pub promo_types: Option<Vec<u32>>,
    /// One nibble per format id: 0 = absent, 1 legal, 2 not_legal, 3 restricted, 4 banned.
    /// Per printing: Scryfall's legalities are not identical across printings of a card.
    pub legalities: Option<Vec<u8>>,
    pub flavor_text: Option<String>,
    pub artist: Option<String>,
    pub images: Images,
    pub faces: Option<Vec<PrintingFace>>,
    /// JSON object text: every remaining key, plus overrides written by the
    /// compiler's self-check (a key whose typed reconstruction would differ).
    /// The reserved key `__absent` lists keys that must be removed.
    pub extra: Option<String>,
}

pub fn encode_nibbles(w: &mut Writer, v: &Option<Vec<u8>>) {
    match v {
        None => w.uvarint(0),
        Some(l) => {
            w.uvarint(l.len() as u64 + 1);
            for pair in l.chunks(2) {
                w.u8(pair[0] | pair.get(1).copied().unwrap_or(0) << 4);
            }
        }
    }
}

pub fn decode_nibbles(r: &mut Reader) -> Result<Option<Vec<u8>>> {
    let n = r.uvarint()? as usize;
    if n == 0 {
        return Ok(None);
    }
    let n = n - 1;
    let bytes = r.take(n.div_ceil(2))?;
    Ok(Some((0..n).map(|i| if i % 2 == 0 { bytes[i / 2] & 0xf } else { bytes[i / 2] >> 4 }).collect()))
}

fn tri(v: Option<bool>) -> u16 {
    match v {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    }
}
fn untri(v: u16) -> Option<bool> {
    match v {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    }
}

impl PrintingRecord {
    pub fn tombstone(scryfall_id: [u8; 16]) -> Self {
        Self {
            tombstone: true,
            scryfall_id,
            oracle_row: 0,
            layout: None,
            lang: None,
            released_at: None,
            set_code: None,
            set_name: None,
            collector_number: None,
            rarity: None,
            border_color: None,
            promo: None,
            reprint: None,
            digital: None,
            finishes: None,
            promo_types: None,
            legalities: None,
            flavor_text: None,
            artist: None,
            images: Images::None,
            faces: None,
            extra: None,
        }
    }

    pub fn encode(&self, w: &mut Writer) {
        let flags: u16 = self.tombstone as u16
            | tri(self.promo) << 1
            | tri(self.reprint) << 3
            | tri(self.digital) << 5;
        w.u16(flags);
        w.bytes(&self.scryfall_id);
        if self.tombstone {
            return;
        }
        w.uvarint(self.oracle_row as u64);
        w.opt_id(self.layout);
        w.opt_id(self.lang);
        w.opt_id(self.released_at);
        w.opt_str(self.set_code.as_deref());
        w.opt_str(self.set_name.as_deref());
        w.opt_str(self.collector_number.as_deref());
        w.opt_id(self.rarity);
        w.opt_id(self.border_color);
        w.opt_id(self.finishes.map(|f| f as u32));
        match &self.promo_types {
            None => w.uvarint(0),
            Some(p) => {
                w.uvarint(p.len() as u64 + 1);
                for id in p {
                    w.uvarint(*id as u64);
                }
            }
        }
        encode_nibbles(w, &self.legalities);
        w.opt_str(self.flavor_text.as_deref());
        w.opt_str(self.artist.as_deref());
        self.images.encode(w);
        encode_printing_faces(w, &self.faces);
        w.opt_str(self.extra.as_deref());
    }

    pub fn decode(r: &mut Reader) -> Result<Self> {
        let flags = r.u16()?;
        let scryfall_id = r.uuid()?;
        if flags & 1 != 0 {
            return Ok(Self::tombstone(scryfall_id));
        }
        let oracle_row = r.uvarint32()?;
        let layout = r.opt_id()?;
        let lang = r.opt_id()?;
        let released_at = r.opt_id()?;
        let set_code = r.opt_string()?;
        let set_name = r.opt_string()?;
        let collector_number = r.opt_string()?;
        let rarity = r.opt_id()?;
        let border_color = r.opt_id()?;
        let finishes = r.opt_id()?.map(|f| f as u8);
        let n = r.uvarint()? as usize;
        let promo_types = if n == 0 {
            None
        } else {
            let mut v = Vec::with_capacity(n - 1);
            for _ in 0..n - 1 {
                v.push(r.uvarint32()?);
            }
            Some(v)
        };
        let legalities = decode_nibbles(r)?;
        let flavor_text = r.opt_string()?;
        let artist = r.opt_string()?;
        let images = Images::decode(r)?;
        let faces = decode_printing_faces(r)?;
        let extra = r.opt_string()?;
        Ok(Self {
            tombstone: false,
            scryfall_id,
            oracle_row,
            layout,
            lang,
            released_at,
            set_code,
            set_name,
            collector_number,
            rarity,
            border_color,
            promo: untri(flags >> 1 & 3),
            reprint: untri(flags >> 3 & 3),
            digital: untri(flags >> 5 & 3),
            finishes,
            promo_types,
            legalities,
            flavor_text,
            artist,
            images,
            faces,
            extra,
        })
    }
}
