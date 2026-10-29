use coqc_format::codec::{Reader, Writer};
use coqc_format::*;

#[test]
fn varints_and_strings_roundtrip() {
    let mut w = Writer::new();
    for v in [0u64, 1, 127, 128, 300, u32::MAX as u64, u64::MAX] {
        w.uvarint(v);
    }
    w.opt_str(None);
    w.opt_str(Some(""));
    w.opt_str(Some("Lim-Dûl's Vault"));
    let mut r = Reader::new(&w.buf);
    for v in [0u64, 1, 127, 128, 300, u32::MAX as u64, u64::MAX] {
        assert_eq!(r.uvarint().unwrap(), v);
    }
    assert_eq!(r.opt_string().unwrap(), None);
    assert_eq!(r.opt_string().unwrap(), Some(String::new()));
    assert_eq!(r.opt_string().unwrap().unwrap(), "Lim-Dûl's Vault");
    assert!(r.is_empty());
}

fn sample_oracle() -> OracleRecord {
    OracleRecord {
        tombstone: false,
        oracle_id: [7; 16],
        name: Some("Fire // Ice".into()),
        mana_cost: None,
        type_line: Some("Instant // Instant".into()),
        oracle_text: None,
        colors: Some(0b01010),
        color_identity: Some(0),
        cmc: Some(0.5),
        faces: Some(vec![OracleFace { name: Some("Fire".into()), extra: Some("{\"power\":\"1\"}".into()), ..Default::default() }]),
        extra: Some("{\"keywords\":[]}".into()),
    }
}

fn sample_printing() -> PrintingRecord {
    PrintingRecord {
        tombstone: false,
        scryfall_id: [9; 16],
        oracle_row: 3,
        layout: Some(0),
        lang: Some(1),
        released_at: Some(20210723),
        set_code: Some("m11".into()),
        set_name: None,
        collector_number: Some("228★".into()),
        rarity: Some(2),
        border_color: None,
        promo: Some(false),
        reprint: Some(true),
        digital: None,
        finishes: Some(0b101),
        promo_types: Some(vec![0, 2]),
        legalities: Some(vec![1, 2, 3, 4, 0]),
        flavor_text: None,
        artist: Some("A".into()),
        images: Images::Canonical { keys: vec![0, 1], stamp: 1700000000 },
        faces: Some(vec![PrintingFace { images: Images::Raw("{}".into()), ..Default::default() }]),
        extra: None,
    }
}

#[test]
fn records_roundtrip() {
    let (o, p) = (sample_oracle(), sample_printing());
    let mut w = Writer::new();
    o.encode(&mut w);
    assert_eq!(OracleRecord::decode(&mut Reader::new(&w.buf)).unwrap(), o);
    let mut w = Writer::new();
    p.encode(&mut w);
    assert_eq!(PrintingRecord::decode(&mut Reader::new(&w.buf)).unwrap(), p);
    for t in [OracleRecord::tombstone([1; 16])] {
        let mut w = Writer::new();
        t.encode(&mut w);
        assert_eq!(OracleRecord::decode(&mut Reader::new(&w.buf)).unwrap(), t);
    }
}

#[test]
fn container_roundtrip_and_determinism() {
    let ds = Dataset {
        meta: Meta { cards_epoch: 2, source_date: 20260701, ..Default::default() },
        oracle: (0..600u32).map(|i| OracleRecord { oracle_id: [(i % 251) as u8; 16], ..sample_oracle() }).collect(),
        ranks: (0..600u32).map(|i| Ranks { edhrec: Some(i * 3), penny: (i % 2 == 0).then_some(i) }).collect(),
        printings: (0..600u32).map(|i| PrintingRecord { oracle_row: i, ..sample_printing() }).collect(),
    };
    let (a, _) = write(&ds).unwrap();
    let (b, _) = write(&ds).unwrap();
    assert_eq!(a, b, "same input must give identical bytes");
    let back = read(&a).unwrap();
    assert_eq!(back.oracle, ds.oracle);
    assert_eq!(back.ranks, ds.ranks);
    assert_eq!(back.printings, ds.printings);
    assert_eq!(back.meta, ds.meta);
}

#[test]
fn rejects_garbage_and_future_versions() {
    assert!(read(b"nope").is_err());
    let ds = Dataset { meta: Meta::default(), oracle: vec![], ranks: vec![], printings: vec![] };
    let (mut bytes, _) = write(&ds).unwrap();
    bytes[6] = 9; // min_reader > supported
    assert!(read(&bytes).is_err());
    bytes[6] = 2;
    bytes[4] = 1; // a version-1 file lacks the ranks section
    bytes[5] = 0;
    assert!(read(&bytes).is_err());
}
