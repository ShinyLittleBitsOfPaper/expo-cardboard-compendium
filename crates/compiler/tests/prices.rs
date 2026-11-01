use coqc::prices::{build_idmap, compile_prices, load_idmap, PriceTable};
use coqc::{compile, Options};
use coqc_format::patch::{apply_patch, make_patch};
use std::path::PathBuf;

const BOLT1: &str = "00000000-0000-4000-8000-000000000001";
const BOLT2: &str = "00000000-0000-4000-8000-000000000002";

fn cards() -> coqc_mtg::Dataset {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample_a.jsonl");
    compile(&p, None, &Options { source_date: 20260701, ..Default::default() }).unwrap().0
}

fn idjson() -> String {
    format!(
        r#"{{"meta":{{}},"data":{{
 "aaaaaaaa-0000-5000-8000-000000000001":{{"name":"x","identifiers":{{"scryfallId":"{BOLT1}"}}}},
 "aaaaaaaa-0000-5000-8000-000000000002":{{"identifiers":{{"scryfallId":"{BOLT2}","mcmId":"1"}}}},
 "aaaaaaaa-0000-5000-8000-000000000003":{{"identifiers":{{}}}},
 "aaaaaaaa-0000-5000-8000-000000000004":{{"identifiers":{{"scryfallId":"99999999-0000-4000-8000-000000000009"}}}}}}}}"#
    )
}

fn prices(day: &str, bolt: f64) -> String {
    format!(
        r#"{{"meta":{{"date":"{day}"}},"data":{{
 "aaaaaaaa-0000-5000-8000-000000000001":{{"paper":{{
   "tcgplayer":{{"buylist":{{}},"retail":{{"normal":{{"{day}":{bolt}}},"foil":{{"{day}":2.5}}}},"currency":"USD"}},
   "cardmarket":{{"retail":{{"etched":{{"{day}":1.05}}}},"currency":"EUR"}},
   "cardkingdom":{{"retail":{{"normal":{{"{day}":9.99}}}}}}}},"mtgo":{{"cardhoarder":{{"retail":{{"normal":{{"{day}":0.03}}}}}}}}}},
 "aaaaaaaa-0000-5000-8000-000000000002":{{"paper":{{"tcgplayer":{{"retail":{{"normal":{{"{day}":0.1}}}}}}}}}},
 "aaaaaaaa-0000-5000-8000-000000000003":{{"paper":{{"tcgplayer":{{"retail":{{"normal":{{"{day}":1.0}}}}}}}}}},
 "aaaaaaaa-0000-5000-8000-000000000004":{{"paper":{{"tcgplayer":{{"retail":{{"normal":{{"{day}":1.0}}}}}}}}}},
 "bbbbbbbb-0000-5000-8000-000000000000":{{"paper":{{"tcgplayer":{{"retail":{{"normal":{{"{day}":1.0}}}}}}}}}}}}}}"#
    )
}

fn table(day: &str, bolt: f64) -> (PriceTable, coqc::prices::PriceReport) {
    let ds = cards();
    let (b, n) = build_idmap(idjson().as_bytes()).unwrap();
    assert_eq!(n, 3); // the entry without scryfallId is skipped
    compile_prices(&ds, &load_idmap(&b).unwrap(), prices(day, bolt).as_bytes()).unwrap()
}

#[test]
fn maps_uuids_and_keeps_only_tcg_and_cardmarket_retail() {
    let (t, r) = table("2026-10-24", 0.5);
    assert_eq!((r.date, r.entries, r.unmapped, r.not_in_cards, r.conflicts), (20261024, 5, 2, 1, 0));
    let ds = cards();
    let row = |id: &str| ds.printings.iter().position(|p| p.scryfall_id == coqc_mtg::json::parse_uuid(id).unwrap()).unwrap();
    assert_eq!(t.cents(row(BOLT1), 0), Some(50));
    assert_eq!(t.cents(row(BOLT1), 1), Some(250));
    assert_eq!(t.cents(row(BOLT1), 2), None);
    assert_eq!(t.cents(row(BOLT1), 5), Some(105)); // cardmarket etched
    assert_eq!(t.cents(row(BOLT2), 0), Some(10));
    assert_eq!(r.rows_priced, 2);
    assert_eq!(t.rows(), ds.printings.len());
    assert_eq!(PriceTable::decode(&t.encode().unwrap()).unwrap(), t);
}

#[test]
fn patch_replays_exactly_and_is_small() {
    let (a, _) = table("2026-10-24", 0.5);
    let (b, _) = table("2026-10-25", 0.49);
    let (fa, fb) = (a.encode().unwrap(), b.encode().unwrap());
    let (d, info) = make_patch(&fa, &fb).unwrap();
    assert!(info.changed("price") >= 1);
    assert_eq!(apply_patch(&fa, &d).unwrap(), fb);
    assert!(apply_patch(&fb, &d).is_err()); // wrong base
}

/// Size check on a synthetic 120k-row table (cargo test -p coqc --test prices -- --ignored --nocapture).
/// Prices are skewed (most cards cheap), ~60% of printings priced, ~35% of priced cells change by a few cents per day.
#[test]
#[ignore]
fn overlay_sizes_synthetic_120k() {
    let mut x = 12345u64;
    let mut rnd = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let n = 120_000;
    let mut a = PriceTable::new(1, 20261024, n);
    for c in 0..6 {
        for i in 0..n {
            if rnd() % 100 < if c == 0 || c == 3 { 60 } else { 12 } {
                let r = rnd() % 1000;
                a.cols[c][i] = 1 + if r < 700 { 5 + (rnd() % 60) as u32 } else if r < 950 { 60 + (rnd() % 600) as u32 } else { 600 + (rnd() % 20000) as u32 };
            }
        }
    }
    let mut b = a.clone();
    b.date = 20261025;
    for c in 0..6 {
        for i in 0..n {
            if b.cols[c][i] != 0 && rnd() % 100 < 35 {
                b.cols[c][i] = (b.cols[c][i] as i64 + (rnd() % 7) as i64 - 3).max(2) as u32;
            }
        }
    }
    // old COQR/COQX layout for comparison: zstd(6 varint columns) and zstd(zigzag deltas)
    let old_full = {
        let mut w = coqc_format::codec::Writer::new();
        a.cols.iter().for_each(|c| c.iter().for_each(|v| w.uvarint(*v as u64)));
        zstd::bulk::compress(&w.buf, 15).unwrap().len()
    };
    let old_delta = {
        let mut w = coqc_format::codec::Writer::new();
        for c in 0..6 {
            for i in 0..n {
                let d = b.cols[c][i] as i64 - a.cols[c][i] as i64;
                w.uvarint(((d << 1) ^ (d >> 63)) as u64);
            }
        }
        zstd::bulk::compress(&w.buf, 19).unwrap().len()
    };
    let (fa, fb) = (a.encode().unwrap(), b.encode().unwrap());
    let (p, info) = make_patch(&fa, &fb).unwrap();
    println!("old full {old_full} B, old delta {old_delta} B");
    println!("new full {} B, new patch {} B (changed rows {})", fa.len(), p.len(), info.changed("price"));
}

#[test]
fn patch_handles_appended_rows_and_refuses_shrinking() {
    let (a, _) = table("2026-10-24", 0.5);
    let mut b = a.clone();
    b.date = 20261025;
    for c in 0..6 {
        b.cols[c].extend([0, 7, 0]);
    }
    b.cols[0][0] = 99;
    let (fa, fb) = (a.encode().unwrap(), b.encode().unwrap());
    let (p, info) = make_patch(&fa, &fb).unwrap();
    assert_eq!(info.appended("price"), 3);
    assert_eq!(apply_patch(&fa, &p).unwrap(), fb);
    assert_eq!(PriceTable::decode(&fb).unwrap(), b);
    assert!(make_patch(&fb, &fa).is_err());
}
