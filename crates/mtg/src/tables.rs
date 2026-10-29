//! The MTG dataset expressed as a schema + tables (S2a). `to_store`/`from_store` convert losslessly between the legacy
//! `Dataset` (oracle/printing/ranks vectors) and the generic `Store`; S2b makes the container read and write the `Store`.
//! Nested structures (card faces, image sets, legalities) are `Bytes` columns encoded with the legacy field encoders, so sizes
//! stay comparable; they are not searchable and are not indexed.
use coqc_format::codec::{Error, Reader, Result, Writer};
use coqc_format::schema::{ColType as T, ColumnDef as C, IndexKind as I, Rel, Schema, TableDef};
use coqc_format::table::{ColData, Store};
use coqc_format::*;

const DICTS: [&str; 7] = ["layouts", "rarities", "border_colors", "langs", "promo_types", "image_keys", "formats"];

pub fn mtg_schema() -> Schema {
    let en = |d: &str| T::Enum(d.to_string());
    Schema {
        id: "mtg".into(),
        version: 1,
        dicts: DICTS.iter().map(|s| s.to_string()).collect(),
        tables: vec![
            TableDef {
                name: "oracle".into(),
                rel: Rel::None,
                key: Some("oracle_id".into()),
                columns: vec![
                    C::new("oracle_id", T::Uuid, I::Exact, &[]),
                    C::new("name", T::Str, I::Text, &["n"]),
                    C::new("mana_cost", T::Str, I::None, &["m"]),
                    C::new("type_line", T::Str, I::Text, &["t", "type"]),
                    C::new("oracle_text", T::Str, I::Text, &["o", "oracle"]),
                    C::new("colors", T::Int, I::Set, &["c", "color"]),
                    C::new("color_identity", T::Int, I::Set, &["id", "ci"]),
                    C::new("cmc", T::Float, I::Range, &["mv", "manavalue"]),
                    C::new("faces", T::Bytes, I::None, &[]),
                    C::new("extra", T::Json, I::None, &[]),
                ],
            },
            TableDef {
                name: "oracle_ranks".into(),
                rel: Rel::Sidecar("oracle".into()),
                key: None,
                columns: vec![C::new("edhrec", T::Int, I::Range, &[]), C::new("penny", T::Int, I::Range, &[])],
            },
            TableDef {
                name: "printing".into(),
                rel: Rel::Parent("oracle".into()),
                key: Some("scryfall_id".into()),
                columns: vec![
                    C::new("scryfall_id", T::Uuid, I::Exact, &["id"]),
                    C::new("oracle_row", T::Ref("oracle".into()), I::None, &[]),
                    C::new("layout", en("layouts"), I::Exact, &[]),
                    C::new("lang", en("langs"), I::Exact, &["l"]),
                    C::new("released_at", T::Int, I::Range, &["date", "year"]),
                    C::new("set_code", T::Str, I::Exact, &["s", "set", "e", "edition"]),
                    C::new("set_name", T::Str, I::Text, &[]),
                    C::new("collector_number", T::Str, I::Exact, &["cn", "number"]),
                    C::new("rarity", en("rarities"), I::Range, &["r"]),
                    C::new("border_color", en("border_colors"), I::Exact, &["border"]),
                    C::new("promo", T::Bool, I::Exact, &[]),
                    C::new("reprint", T::Bool, I::Exact, &[]),
                    C::new("digital", T::Bool, I::Exact, &[]),
                    C::new("finishes", T::Int, I::Set, &[]),
                    C::new("promo_types", T::Set("promo_types".into()), I::Set, &[]),
                    C::new("legalities", T::Bytes, I::None, &[]),
                    C::new("flavor_text", T::Str, I::Text, &["ft"]),
                    C::new("artist", T::Str, I::Text, &["a"]),
                    C::new("images", T::Bytes, I::None, &[]),
                    C::new("faces", T::Bytes, I::None, &[]),
                    C::new("extra", T::Json, I::None, &[]),
                ],
            },
        ],
    }
}

fn bytes_of(f: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer::new();
    f(&mut w);
    w.buf
}

fn opt_bytes(present: bool, f: impl FnOnce(&mut Writer)) -> Option<Vec<u8>> {
    present.then(|| bytes_of(f))
}

fn dicts_of(m: &Meta) -> Vec<Dict> {
    vec![m.layouts.clone(), m.rarities.clone(), m.border_colors.clone(), m.langs.clone(), m.promo_types.clone(), m.image_keys.clone(), m.formats.clone()]
}

pub fn to_store(ds: &Dataset) -> Store {
    let mut st = Store::new(mtg_schema());
    st.epoch = ds.meta.cards_epoch;
    st.source_date = ds.meta.source_date;
    st.dicts = dicts_of(&ds.meta);
    st.props = vec![("dropped_keys".into(), ds.meta.dropped_keys.clone())];

    let o = &mut st.tables[0];
    o.tomb = ds.oracle.iter().map(|r| r.tombstone).collect();
    o.cols = vec![
        ColData::Uuid(ds.oracle.iter().map(|r| r.oracle_id).collect()),
        ColData::Str(ds.oracle.iter().map(|r| r.name.clone()).collect()),
        ColData::Str(ds.oracle.iter().map(|r| r.mana_cost.clone()).collect()),
        ColData::Str(ds.oracle.iter().map(|r| r.type_line.clone()).collect()),
        ColData::Str(ds.oracle.iter().map(|r| r.oracle_text.clone()).collect()),
        ColData::Int(ds.oracle.iter().map(|r| r.colors.map(|c| c as i64)).collect()),
        ColData::Int(ds.oracle.iter().map(|r| r.color_identity.map(|c| c as i64)).collect()),
        ColData::Float(ds.oracle.iter().map(|r| r.cmc).collect()),
        ColData::Bytes(ds.oracle.iter().map(|r| opt_bytes(r.faces.is_some(), |w| encode_oracle_faces(w, &r.faces))).collect()),
        ColData::Json(ds.oracle.iter().map(|r| r.extra.clone()).collect()),
    ];

    let k = &mut st.tables[1];
    k.tomb = ds.oracle.iter().map(|r| r.tombstone).collect();
    k.cols = vec![
        ColData::Int(ds.ranks.iter().map(|r| r.edhrec.map(|x| x as i64)).collect()),
        ColData::Int(ds.ranks.iter().map(|r| r.penny.map(|x| x as i64)).collect()),
    ];

    let p = &mut st.tables[2];
    let pr = &ds.printings;
    p.tomb = pr.iter().map(|r| r.tombstone).collect();
    p.cols = vec![
        ColData::Uuid(pr.iter().map(|r| r.scryfall_id).collect()),
        ColData::Ref(pr.iter().map(|r| r.oracle_row).collect()),
        ColData::Enum(pr.iter().map(|r| r.layout).collect()),
        ColData::Enum(pr.iter().map(|r| r.lang).collect()),
        ColData::Int(pr.iter().map(|r| r.released_at.map(|x| x as i64)).collect()),
        ColData::Str(pr.iter().map(|r| r.set_code.clone()).collect()),
        ColData::Str(pr.iter().map(|r| r.set_name.clone()).collect()),
        ColData::Str(pr.iter().map(|r| r.collector_number.clone()).collect()),
        ColData::Enum(pr.iter().map(|r| r.rarity).collect()),
        ColData::Enum(pr.iter().map(|r| r.border_color).collect()),
        ColData::Bool(pr.iter().map(|r| r.promo).collect()),
        ColData::Bool(pr.iter().map(|r| r.reprint).collect()),
        ColData::Bool(pr.iter().map(|r| r.digital).collect()),
        ColData::Int(pr.iter().map(|r| r.finishes.map(|x| x as i64)).collect()),
        ColData::Set(pr.iter().map(|r| r.promo_types.clone()).collect()),
        ColData::Bytes(pr.iter().map(|r| opt_bytes(r.legalities.is_some(), |w| encode_nibbles(w, &r.legalities))).collect()),
        ColData::Str(pr.iter().map(|r| r.flavor_text.clone()).collect()),
        ColData::Str(pr.iter().map(|r| r.artist.clone()).collect()),
        ColData::Bytes(pr.iter().map(|r| opt_bytes(r.images != Images::None, |w| r.images.encode(w))).collect()),
        ColData::Bytes(pr.iter().map(|r| opt_bytes(r.faces.is_some(), |w| encode_printing_faces(w, &r.faces))).collect()),
        ColData::Json(pr.iter().map(|r| r.extra.clone()).collect()),
    ];
    st
}

fn opt_dec<T>(b: &Option<Vec<u8>>, f: impl FnOnce(&mut Reader) -> Result<T>) -> Result<Option<T>> {
    b.as_ref().map(|b| f(&mut Reader::new(b))).transpose()
}

pub fn from_store(st: &Store) -> Result<Dataset> {
    if st.schema != mtg_schema() {
        return Err(Error::Corrupt("not the MTG schema"));
    }
    st.check().map_err(|_| Error::Corrupt("inconsistent store"))?;
    macro_rules! col {
        ($t:expr, $i:expr, $v:ident) => {
            match &st.tables[$t].cols[$i] {
                ColData::$v(v) => v,
                _ => return Err(Error::Corrupt("column type")),
            }
        };
    }
    let mut meta = Meta { cards_epoch: st.epoch, source_date: st.source_date, ..Default::default() };
    let mut d = st.dicts.iter().cloned();
    meta.layouts = d.next().unwrap();
    meta.rarities = d.next().unwrap();
    meta.border_colors = d.next().unwrap();
    meta.langs = d.next().unwrap();
    meta.promo_types = d.next().unwrap();
    meta.image_keys = d.next().unwrap();
    meta.formats = d.next().unwrap();
    meta.dropped_keys = st.prop("dropped_keys").map(|v| v.to_vec()).unwrap_or_default();

    let ot = &st.tables[0];
    let mut oracle = Vec::with_capacity(ot.rows());
    for i in 0..ot.rows() {
        let id = col!(0, 0, Uuid)[i];
        if ot.tomb[i] {
            oracle.push(OracleRecord::tombstone(id));
            continue;
        }
        oracle.push(OracleRecord {
            tombstone: false,
            oracle_id: id,
            name: col!(0, 1, Str)[i].clone(),
            mana_cost: col!(0, 2, Str)[i].clone(),
            type_line: col!(0, 3, Str)[i].clone(),
            oracle_text: col!(0, 4, Str)[i].clone(),
            colors: col!(0, 5, Int)[i].map(|x| x as u8),
            color_identity: col!(0, 6, Int)[i].map(|x| x as u8),
            cmc: col!(0, 7, Float)[i],
            faces: opt_dec(&col!(0, 8, Bytes)[i], decode_oracle_faces)?.flatten(),
            extra: col!(0, 9, Json)[i].clone(),
        });
    }
    let (edhrec, penny) = (col!(1, 0, Int), col!(1, 1, Int));
    let ranks = (0..st.tables[1].rows()).map(|i| Ranks { edhrec: edhrec[i].map(|x| x as u32), penny: penny[i].map(|x| x as u32) }).collect();

    let pt = &st.tables[2];
    let mut printings = Vec::with_capacity(pt.rows());
    for i in 0..pt.rows() {
        let id = col!(2, 0, Uuid)[i];
        if pt.tomb[i] {
            printings.push(PrintingRecord::tombstone(id));
            continue;
        }
        printings.push(PrintingRecord {
            tombstone: false,
            scryfall_id: id,
            oracle_row: col!(2, 1, Ref)[i],
            layout: col!(2, 2, Enum)[i],
            lang: col!(2, 3, Enum)[i],
            released_at: col!(2, 4, Int)[i].map(|x| x as u32),
            set_code: col!(2, 5, Str)[i].clone(),
            set_name: col!(2, 6, Str)[i].clone(),
            collector_number: col!(2, 7, Str)[i].clone(),
            rarity: col!(2, 8, Enum)[i],
            border_color: col!(2, 9, Enum)[i],
            promo: col!(2, 10, Bool)[i],
            reprint: col!(2, 11, Bool)[i],
            digital: col!(2, 12, Bool)[i],
            finishes: col!(2, 13, Int)[i].map(|x| x as u8),
            promo_types: col!(2, 14, Set)[i].clone(),
            legalities: opt_dec(&col!(2, 15, Bytes)[i], decode_nibbles)?.flatten(),
            flavor_text: col!(2, 16, Str)[i].clone(),
            artist: col!(2, 17, Str)[i].clone(),
            images: opt_dec(&col!(2, 18, Bytes)[i], Images::decode)?.unwrap_or(Images::None),
            faces: opt_dec(&col!(2, 19, Bytes)[i], decode_printing_faces)?.flatten(),
            extra: col!(2, 20, Json)[i].clone(),
        });
    }
    Ok(Dataset { meta, oracle, ranks, printings })
}
