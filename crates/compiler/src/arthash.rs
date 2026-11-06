//! Art-hash overlay (docs/phash-design.md, P2): one 64-bit pHash of the inner 80% of each illustration's art crop, copied to every
//! printing that shares the illustration. A `.coqd` overlay like prices: schema `mtg-arthash`, one columnar table `art`, row i = printing row i,
//! two nullable Int columns (high/low 32 bits; null = no hash). `illustration_id` is read from the Scryfall bulk file at compile time, so the
//! base `mtg` schema is unchanged. Hashes persist across runs in a TSV cache (`illustration_id<TAB>16 hex digits`).
use coqc_format::codec::Error;
use coqc_format::coqd::{read_store, write_store};
use coqc_format::schema::{ColType, ColumnDef, IndexKind, Rel, Schema, TableDef};
use coqc_format::table::{ColData, Store};
use coqc_mtg::json::parse_uuid;
use coqc_mtg::Dataset;
use std::collections::{BTreeMap, HashMap};

pub const SCHEMA_ID: &str = "mtg-arthash";
pub const COLUMNS: [&str; 2] = ["phash_hi", "phash_lo"];
const PROP_OVERLAY_OF: &str = "overlay_of";

pub fn schema() -> Schema {
    Schema {
        id: SCHEMA_ID.into(),
        version: 1,
        dicts: vec![],
        tables: vec![TableDef {
            name: "art".into(),
            rel: Rel::None,
            key: None,
            columnar: true,
            columns: COLUMNS.iter().map(|c| ColumnDef::new(c, ColType::Int, IndexKind::Range, &[])).collect(),
        }],
    }
}

/// Per printing row of the card file of the same epoch: the art hash, if the illustration has one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArtHashTable {
    pub cards_epoch: u32,
    pub date: u32,
    pub hashes: Vec<Option<u64>>,
}

impl ArtHashTable {
    pub fn to_store(&self) -> Store {
        let mut st = Store::new(schema());
        st.epoch = self.cards_epoch;
        st.source_date = self.date;
        st.props = vec![(PROP_OVERLAY_OF.into(), vec!["printing".into()])];
        let t = &mut st.tables[0];
        t.tomb = vec![false; self.hashes.len()];
        t.cols = vec![
            ColData::Int(self.hashes.iter().map(|h| h.map(|v| (v >> 32) as i64)).collect()),
            ColData::Int(self.hashes.iter().map(|h| h.map(|v| (v & 0xffff_ffff) as i64)).collect()),
        ];
        st
    }
    pub fn from_store(st: &Store) -> Result<Self, Error> {
        if st.schema != schema() {
            return Err(Error::Corrupt("not an art-hash overlay"));
        }
        let t = &st.tables[0];
        let (ColData::Int(hi), ColData::Int(lo)) = (&t.cols[0], &t.cols[1]) else { return Err(Error::Corrupt("bad art-hash column")) };
        if hi.len() != lo.len() {
            return Err(Error::Corrupt("art-hash column length"));
        }
        let half = |x: i64| u32::try_from(x).map(u64::from).map_err(|_| Error::Corrupt("art-hash out of range"));
        let hashes = hi.iter().zip(lo).map(|(h, l)| match (h, l) {
            (Some(h), Some(l)) => Ok(Some((half(*h)? << 32) | half(*l)?)),
            (None, None) => Ok(None),
            _ => Err(Error::Corrupt("art-hash half missing")),
        }).collect::<Result<Vec<_>, _>>()?;
        Ok(Self { cards_epoch: st.epoch, date: st.source_date, hashes })
    }
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        Ok(write_store(&self.to_store())?.0)
    }
    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        Self::from_store(&read_store(b)?)
    }
}

/// One bulk-file card with an illustration: its Scryfall id, illustration id and front-face art crop URL.
#[derive(Debug, Clone, PartialEq)]
pub struct BulkArt {
    pub scryfall_id: [u8; 16],
    pub illustration_id: String,
    pub url: String,
}

fn bulk_entry(v: &serde_json::Value) -> Option<BulkArt> {
    let face0 = v.get("card_faces").and_then(|f| f.get(0));
    let pick = |k: &str| v.get(k).or_else(|| face0.and_then(|f| f.get(k)));
    let ill = pick("illustration_id")?.as_str()?.to_string();
    let url = pick("image_uris")?.get("art_crop")?.as_str()?.to_string();
    Some(BulkArt { scryfall_id: parse_uuid(v.get("id")?.as_str()?)?, illustration_id: ill, url })
}

/// Entries of a Scryfall bulk file (JSON array or JSONL).
pub fn scan_bulk(bytes: &[u8]) -> Result<Vec<BulkArt>, String> {
    let first = bytes.iter().find(|b| !b.is_ascii_whitespace());
    if first == Some(&b'[') {
        let all: Vec<serde_json::Value> = serde_json::from_slice(bytes).map_err(|e| format!("bulk: {e}"))?;
        return Ok(all.iter().filter_map(bulk_entry).collect());
    }
    let mut out = Vec::new();
    for line in bytes.split(|b| *b == b'\n').filter(|l| !l.iter().all(|b| b.is_ascii_whitespace())) {
        let v: serde_json::Value = serde_json::from_slice(line).map_err(|e| format!("bulk line: {e}"))?;
        out.extend(bulk_entry(&v));
    }
    Ok(out)
}

pub fn parse_cache(text: &str) -> HashMap<String, u64> {
    text.lines().filter_map(|l| {
        let (id, h) = l.split_once('\t')?;
        Some((id.to_string(), u64::from_str_radix(h.trim(), 16).ok()?))
    }).collect()
}

pub fn render_cache(c: &HashMap<String, u64>) -> String {
    let sorted: BTreeMap<_, _> = c.iter().collect();
    sorted.iter().map(|(k, v)| format!("{k}\t{v:016x}\n")).collect()
}

#[derive(Debug, Default)]
pub struct ArtReport {
    pub printings: usize,
    pub printings_with_illustration: usize,
    pub illustrations: usize,
    pub illustrations_hashed: usize,
    pub rows_hashed: usize,
    /// (illustration id, art_crop url) of illustrations still without a hash, one per illustration, sorted.
    pub todo: Vec<(String, String)>,
}

/// Build the overlay for a card file: every live printing whose illustration is in `cache` gets that hash.
pub fn build(ds: &Dataset, bulk: &[BulkArt], cache: &HashMap<String, u64>) -> (ArtHashTable, ArtReport) {
    let by_sid: HashMap<[u8; 16], &BulkArt> = bulk.iter().map(|b| (b.scryfall_id, b)).collect();
    let mut t = ArtHashTable { cards_epoch: ds.meta.cards_epoch, date: ds.meta.source_date, hashes: vec![None; ds.printings.len()] };
    let mut rep = ArtReport::default();
    let mut illus: BTreeMap<&str, &str> = BTreeMap::new();
    for (i, p) in ds.printings.iter().enumerate() {
        if p.tombstone {
            continue;
        }
        rep.printings += 1;
        let Some(b) = by_sid.get(&p.scryfall_id) else { continue };
        rep.printings_with_illustration += 1;
        illus.entry(&b.illustration_id).or_insert(&b.url);
        if let Some(h) = cache.get(&b.illustration_id) {
            t.hashes[i] = Some(*h);
            rep.rows_hashed += 1;
        }
    }
    rep.illustrations = illus.len();
    rep.illustrations_hashed = illus.keys().filter(|k| cache.contains_key(**k)).count();
    rep.todo = illus.iter().filter(|(k, _)| !cache.contains_key(**k)).map(|(k, u)| (k.to_string(), u.to_string())).collect();
    (t, rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_roundtrip_and_cache() {
        let t = ArtHashTable { cards_epoch: 3, date: 20261106, hashes: vec![Some(u64::MAX), None, Some(0x0123_4567_89ab_cdef), Some(0)] };
        assert_eq!(ArtHashTable::decode(&t.encode().unwrap()).unwrap(), t);
        let mut c = HashMap::new();
        c.insert("b".to_string(), 0xdeadbeefu64);
        c.insert("a".to_string(), u64::MAX);
        let text = render_cache(&c);
        assert!(text.starts_with("a\t"));
        assert_eq!(parse_cache(&text), c);
    }

    #[test]
    fn bulk_scan_front_face_and_jsonl() {
        let one = r#"{"id":"11111111-1111-1111-1111-111111111111","illustration_id":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa","image_uris":{"art_crop":"https://x/a.jpg"}}"#;
        let dfc = r#"{"id":"22222222-2222-2222-2222-222222222222","card_faces":[{"illustration_id":"bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb","image_uris":{"art_crop":"https://x/b.jpg"}},{"illustration_id":"cccccccc-cccc-cccc-cccc-cccccccccccc","image_uris":{"art_crop":"https://x/c.jpg"}}]}"#;
        let none = r#"{"id":"33333333-3333-3333-3333-333333333333","name":"no art"}"#;
        let jl = scan_bulk(format!("{one}\n{dfc}\n{none}\n").as_bytes()).unwrap();
        assert_eq!(jl.len(), 2);
        assert_eq!(jl[1].illustration_id, "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb");
        let arr = scan_bulk(format!("[{one},{dfc},{none}]").as_bytes()).unwrap();
        assert_eq!(arr, jl);
    }

    #[test]
    fn corrupt_half_is_rejected() {
        let mut st = ArtHashTable { cards_epoch: 1, date: 1, hashes: vec![Some(5)] }.to_store();
        st.tables[0].cols[1] = ColData::Int(vec![None]);
        assert!(ArtHashTable::from_store(&st).is_err());
    }
}
