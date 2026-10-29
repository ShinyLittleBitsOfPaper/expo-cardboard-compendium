use coqc::{compile, verify, Id, Options};
use coqc_mtg::json::{card_json, parse_uuid};
use coqc_format::{read, write, Dataset, Images};
use std::path::PathBuf;

fn fx(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}
fn opts() -> Options {
    Options { source_date: 20260701, skip_digital: true, skip_art_series: true, ..Default::default() }
}
fn id(n: u32) -> Id {
    parse_uuid(&format!("00000000-0000-4000-8000-{n:012}")).unwrap()
}
fn row(ds: &Dataset, n: u32) -> usize {
    ds.printings.iter().position(|p| p.scryfall_id == id(n)).unwrap()
}
fn card(ds: &Dataset, n: u32) -> serde_json::Value {
    let p = &ds.printings[row(ds, n)];
    card_json(&ds.meta, &ds.oracle[p.oracle_row as usize], &ds.ranks[p.oracle_row as usize], p)
}

#[test]
fn filters_and_lossless_roundtrip() {
    let (ds, st) = compile(&fx("sample_a.jsonl"), None, &opts()).unwrap();
    assert_eq!(
        (st.skipped_digital, st.skipped_art_series, st.skipped_no_oracle_id, st.skipped_bad_id),
        (1, 1, 1, 1)
    );
    assert_eq!(st.printings, 10);
    assert_eq!(st.oracle_cards, 9);
    assert_eq!(verify(&ds, &fx("sample_a.jsonl")).unwrap(), (10, 0));
    // survives a trip through bytes too
    let (bytes, _) = write(&ds).unwrap();
    assert_eq!(verify(&read(&bytes).unwrap(), &fx("sample_a.jsonl")).unwrap(), (10, 0));
}

#[test]
fn jsonl_and_array_input_are_identical_and_deterministic() {
    let a = write(&compile(&fx("sample_a.jsonl"), None, &opts()).unwrap().0).unwrap().0;
    let b = write(&compile(&fx("sample_a.jsonl"), None, &opts()).unwrap().0).unwrap().0;
    let c = write(&compile(&fx("sample_a_array.json"), None, &opts()).unwrap().0).unwrap().0;
    assert_eq!(a, b);
    assert_eq!(a, c);
}

#[test]
fn tricky_cards_keep_every_field() {
    let (ds, _) = compile(&fx("sample_a.jsonl"), None, &opts()).unwrap();
    // half mana value
    assert_eq!(card(&ds, 11)["cmc"].as_f64(), Some(0.5));
    // reversible card: no top-level oracle_id in the source, none rebuilt
    assert!(card(&ds, 6).get("oracle_id").is_none());
    // unknown future key and nested nulls: key kept, null-valued key dropped
    let weird = card(&ds, 14);
    assert!(weird.get("future_field").is_some());
    assert!(weird.get("watermark").is_none());
    // unknown finish and non-WUBRG color order survive verbatim
    assert_eq!(weird["finishes"], serde_json::json!(["glossy", "nonfoil"]));
    assert_eq!(weird["colors"], serde_json::json!(["G", "W"]));
    assert_eq!(weird["legalities"]["newformat"], "legal");
    // non-canonical image host stays raw; Scryfall-shaped ones are compacted
    assert!(matches!(ds.printings[row(&ds, 12)].images, Images::Raw(_)));
    assert!(matches!(ds.printings[row(&ds, 1)].images, Images::Canonical { .. }));
    // Scryfall's volatile `prices` object is dropped by default
    assert!(card(&ds, 1).get("prices").is_none());
    assert_eq!(ds.meta.dropped_keys, vec!["prices".to_string()]);
}

#[test]
fn keep_prices_flag_preserves_prices() {
    let o = Options { keep_prices: true, ..opts() };
    let (ds, _) = compile(&fx("sample_a.jsonl"), None, &o).unwrap();
    assert_eq!(card(&ds, 1)["prices"]["usd"], "0.10");
    assert_eq!(verify(&ds, &fx("sample_a.jsonl")).unwrap().1, 0);
}

#[test]
fn row_ids_are_stable_across_releases() {
    let (a, _) = compile(&fx("sample_a.jsonl"), None, &opts()).unwrap();
    let (b, st) = compile(&fx("sample_b.jsonl"), Some(&a), &opts()).unwrap();
    assert_eq!(b.meta.cards_epoch, a.meta.cards_epoch);
    // every earlier row keeps its id
    for (i, p) in a.printings.iter().enumerate() {
        assert_eq!(b.printings[i].scryfall_id, p.scryfall_id);
    }
    for (i, o) in a.oracle.iter().enumerate() {
        assert_eq!(b.oracle[i].oracle_id, o.oracle_id);
    }
    // removed printing is a tombstone, new ones are appended
    assert!(b.printings[row(&a, 2)].tombstone);
    assert_eq!(st.new_printings, 2);
    assert_eq!(st.tombstoned_printings, 1);
    assert_eq!(b.printings.len(), a.printings.len() + 2);
    assert!(row(&b, 15) >= a.printings.len() && row(&b, 16) >= a.printings.len());
    // dictionaries are append-only
    assert_eq!(&b.meta.formats.values()[..a.meta.formats.len()], a.meta.formats.values());
    assert_eq!(verify(&b, &fx("sample_b.jsonl")).unwrap(), (11, 0));
    // an edit is reflected
    assert_eq!(card(&b, 11)["oracle_text"], "Half mana, errata.");
}

#[test]
fn removed_card_can_return_with_same_row() {
    let (a, _) = compile(&fx("sample_a.jsonl"), None, &opts()).unwrap();
    let (b, _) = compile(&fx("sample_b.jsonl"), Some(&a), &opts()).unwrap();
    let (c, _) = compile(&fx("sample_a.jsonl"), Some(&b), &opts()).unwrap();
    assert!(!c.printings[row(&a, 2)].tombstone);
    assert_eq!(row(&c, 2), row(&a, 2));
}

#[test]
fn rebase_starts_a_new_epoch() {
    let (a, _) = compile(&fx("sample_a.jsonl"), None, &opts()).unwrap();
    let o = Options { rebase: true, ..opts() };
    let (b, _) = compile(&fx("sample_b.jsonl"), Some(&a), &o).unwrap();
    assert_eq!(b.meta.cards_epoch, a.meta.cards_epoch + 1);
    assert!(b.printings.iter().all(|p| !p.tombstone));
}

#[test]
fn oracle_record_follows_the_majority_and_webp_art_images_are_canonical() {
    let first = include_str!("fixtures/sample_a.jsonl").lines().next().unwrap();
    let old = "00000000-0000-4000-8000-000000000001";
    let mut lines = Vec::new();
    for (n, rank) in [(101u32, 5), (102, 5), (103, 9)] {
        let new = format!("00000000-0000-4000-8000-{n:012}");
        let mut c: serde_json::Value = serde_json::from_str(&first.replace(old, &new)).unwrap();
        c["edhrec_rank"] = rank.into();
        // a size Scryfall added later, served as webp, sharing the others' stamp
        c["image_uris"]["art"] = format!("https://cards.scryfall.io/art/front/0/0/{new}.webp?1700000001").into();
        // a JSON key whose URL path segment differs from the key (`jpeg` -> `/normal/`)
        c["image_uris"]["jpeg"] = format!("https://cards.scryfall.io/normal/front/0/0/{new}.jpg?1700000001").into();
        lines.push(c.to_string());
    }
    let path = std::env::temp_dir().join("coqc_majority_test.jsonl");
    std::fs::write(&path, lines.join("\n")).unwrap();
    let (ds, st) = compile(&path, None, &opts()).unwrap();
    assert_eq!(st.healed_printings, 1, "only the printing that disagrees with the majority is healed");
    assert_eq!(st.images_raw, 0, "art.webp must be canonical: {:?}", st.image_mismatches);
    assert_eq!(verify(&ds, &path).unwrap(), (3, 0));
    assert_eq!(card(&ds, 103)["edhrec_rank"], 9);
    assert_eq!(card(&ds, 101)["edhrec_rank"], 5);
    let art = card(&ds, 102)["image_uris"]["art"].as_str().unwrap().to_string();
    assert!(art.ends_with("/00000000-0000-4000-8000-000000000102.webp?1700000001"), "{art}");
    let jpeg = card(&ds, 102)["image_uris"]["jpeg"].as_str().unwrap().to_string();
    assert!(jpeg.contains("/normal/front/"), "{jpeg}");
}

#[test]
fn legalities_are_kept_per_printing() {
    let (ds, st) = compile(&fx("sample_b.jsonl"), None, &opts()).unwrap();
    // card 1 differs from card 15 (same oracle) only by legalities: nothing needs healing for it
    assert_eq!(card(&ds, 1)["legalities"]["pauper"], "legal");
    assert_eq!(card(&ds, 15)["legalities"]["pauper"], "not_legal");
    assert!(!st.healed_keys.contains_key("legalities"));
}

#[test]
fn breakdown_reports_residual_keys() {
    let (ds, _) = compile(&fx("sample_a.jsonl"), None, &opts()).unwrap();
    let b = coqc::breakdown(&ds).unwrap();
    // `uri` is derived rather than stored, so check a key that is stored
    assert!(b["purchase_uris"].raw > 0 && b["purchase_uris"].stored > 0);
    assert!(!b.contains_key("uri"));
}

#[test]
fn new_rows_are_grouped_by_set_then_collector_number() {
    let (ds, _) = compile(&fx("sample_a.jsonl"), None, &opts()).unwrap();
    let keys: Vec<(String, u32)> = ds
        .printings
        .iter()
        .map(|p| {
            let cn = p.collector_number.clone().unwrap_or_default();
            (p.set_code.clone().unwrap_or_default(), cn.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(u32::MAX))
        })
        .collect();
    assert!(keys.windows(2).all(|w| w[0] <= w[1]), "{keys:?}");
    // uuid order is available and gives different rows
    let o = Options { row_order: coqc::RowOrder::Uuid, ..opts() };
    let (u, _) = compile(&fx("sample_a.jsonl"), None, &o).unwrap();
    assert_eq!(u.printings.len(), ds.printings.len());
    assert!(u.printings.windows(2).all(|w| w[0].scryfall_id <= w[1].scryfall_id));
}

#[test]
fn diff_keys_names_what_changed() {
    let (a, _) = compile(&fx("sample_a.jsonl"), None, &opts()).unwrap();
    let (b, _) = compile(&fx("sample_b.jsonl"), Some(&a), &opts()).unwrap();
    let (o, p, r) = coqc::diff_keys(&a, &b);
    assert_eq!(r, 1); // Bolt's representative printing changed, so its rank row changed
    assert!(o.by_key.get("edhrec_rank").is_none(), "ranks are not in the oracle blocks");
    assert_eq!(o.by_key.get("oracle_text"), Some(&1)); // Little Girl errata
    assert_eq!(o.rows_new - o.rows_old, 1); // Brand New Card
    assert_eq!(p.became_tombstone, 1); // card 2 removed
    assert_eq!(p.rows_new - p.rows_old, 2);
    assert_eq!(p.by_key.get("legalities"), Some(&1)); // card 1 pauper legality
}

#[test]
fn derived_urls_are_not_stored_but_come_back() {
    let (ds, st) = compile(&fx("sample_a.jsonl"), None, &opts()).unwrap();
    let (hit, _) = st.derived["uri"];
    assert!(hit > 0);
    let p = &ds.printings[row(&ds, 1)];
    assert!(!p.extra.as_deref().unwrap_or("").contains("\"uri\""), "uri must not be stored");
    assert_eq!(card(&ds, 1)["uri"], "https://api.scryfall.com/cards/00000000-0000-4000-8000-000000000001");
    assert!(card(&ds, 1)["rulings_uri"].as_str().unwrap().ends_with("/rulings"));
}

#[test]
fn patch_apply_reproduces_the_new_file_exactly() {
    let (a, _) = compile(&fx("sample_a.jsonl"), None, &opts()).unwrap();
    let (b, _) = compile(&fx("sample_b.jsonl"), Some(&a), &opts()).unwrap();
    let (ab, bb) = (write(&a).unwrap().0, write(&b).unwrap().0);
    let (patch, info) = coqc::patch::make_patch(&ab, &bb).unwrap();
    assert_eq!(info.printing_appended, 2);
    assert_eq!(info.oracle_appended, 1);
    let out = coqc::patch::apply_patch(&ab, &patch).unwrap();
    assert_eq!(out, bb);
    // wrong base and tampered patches are rejected
    assert!(coqc::patch::apply_patch(&bb, &patch).is_err());
    let mut bad = patch.clone();
    let n = bad.len();
    bad[n - 3] ^= 0xff;
    assert!(coqc::patch::apply_patch(&ab, &bad).is_err());
    // a different epoch cannot be patched
    let (c, _) = compile(&fx("sample_b.jsonl"), Some(&a), &Options { rebase: true, ..opts() }).unwrap();
    assert!(coqc::patch::make_patch(&ab, &write(&c).unwrap().0).is_err());
}

#[test]
fn digital_and_art_series_included_by_default() {
    let o = Options { source_date: 20260701, ..Default::default() };
    let (ds, st) = compile(&fx("sample_a.jsonl"), None, &o).unwrap();
    assert_eq!((st.skipped_digital, st.skipped_art_series), (0, 0));
    assert_eq!(st.printings, 12);
    assert_eq!(verify(&ds, &fx("sample_a.jsonl")).unwrap(), (12, 0));
    // a dataset compiled with the skips still verifies against the full input
    let (skipped, _) = compile(&fx("sample_a.jsonl"), None, &opts()).unwrap();
    assert_eq!(verify(&skipped, &fx("sample_a.jsonl")).unwrap(), (10, 0));
}
