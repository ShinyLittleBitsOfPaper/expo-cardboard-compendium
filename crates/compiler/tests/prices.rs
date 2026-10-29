use coqc::prices::{build_idmap, compile_prices, load_idmap};
use coqc::{compile, Options};
use coqc_format::prices::{apply_delta, make_delta, PriceTable};
use std::path::PathBuf;

const BOLT1: &str = "00000000-0000-4000-8000-000000000001";
const BOLT2: &str = "00000000-0000-4000-8000-000000000002";

fn cards() -> coqc_format::Dataset {
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
fn delta_replays_exactly_and_is_small() {
    let (a, _) = table("2026-10-24", 0.5);
    let (b, _) = table("2026-10-25", 0.49);
    let (fa, fb) = (a.encode().unwrap(), b.encode().unwrap());
    let d = make_delta(&fa, &fb).unwrap();
    assert_eq!(apply_delta(&fa, &d).unwrap(), fb);
    assert!(apply_delta(&fb, &d).is_err()); // wrong base
}
