use coqc::{compile, quick, Options};
use coqc_format::search::*;
use std::path::PathBuf;

fn setup() -> (coqc_format::Dataset, SearchIndex) {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample_a.jsonl");
    let (ds, _) = compile(&p, None, &Options { source_date: 20260701, ..Default::default() }).unwrap();
    let ix = SearchIndex::build(&ds);
    (ds, ix)
}

fn names(ds: &coqc_format::Dataset, q: &str, unique: Unique) -> Vec<String> {
    let (_, ix) = setup();
    let r = ix.search(&Query { expr: quick::parse(q).unwrap(), unique, ..Default::default() });
    r.rows.iter().map(|&p| ds.oracle[ds.printings[p as usize].oracle_row as usize].name.clone().unwrap()).collect()
}

#[test]
fn text_predicates_fold_accents_and_verify() {
    let (ds, _) = setup();
    assert_eq!(names(&ds, "lim-dul", Unique::Prints), vec!["Lim-Dûl's Vault"]);
    assert_eq!(names(&ds, "limdul", Unique::Prints).len(), 0);
    assert_eq!(names(&ds, "bolt", Unique::Prints).len(), 2);
    assert_eq!(names(&ds, "bolt", Unique::Cards).len(), 1);
    assert_eq!(names(&ds, "o:\"3 damage\"", Unique::Cards), vec!["Lightning Bolt"]);
    assert_eq!(names(&ds, "o:\"half mana\"", Unique::Cards), vec!["Little Girl"]);
    assert_eq!(names(&ds, "t:human", Unique::Cards).len(), 2); // Delver (face types), Little Girl
    assert_eq!(names(&ds, "t:insect", Unique::Cards).len(), 1); // second face only
    assert_eq!(names(&ds, "o:zz", Unique::Cards).len(), 0); // <3 bytes would scan; 'zz' absent
    assert_eq!(names(&ds, "t:i", Unique::Cards).len(), 7); // 2-byte scan path
}

#[test]
fn color_identity_mv_rarity_set() {
    let (ds, _) = setup();
    let n = |q: &str| names(&ds, q, Unique::Cards);
    assert_eq!(n("c:r").len(), 3); // bolt, fire//ice, bonecrusher
    assert_eq!(n("c=ru"), vec!["Fire // Ice"]);
    assert!(n("c<=ur").contains(&"Fire // Ice".to_string()));
    assert!(n("c<=ur").contains(&"Colorless Rock".to_string())); // colorless is a subset
    assert!(n("c=c").contains(&"Colorless Rock".to_string()));
    assert!(n("c:u").contains(&"Delver of Secrets // Insectile Aberration".to_string())); // face-union colors
    assert_eq!(n("id>=ub"), vec!["Lim-Dûl's Vault"]);
    assert_eq!(n("mv=0.5"), vec!["Little Girl"]); // fractional mana value
    assert_eq!(n("mv<1").len(), 1);
    assert!(n("mv>=4").contains(&"Fire // Ice".to_string()));
    assert_eq!(n("r>=rare"), vec!["Little Girl"]);
    assert_eq!(n("r:common t:instant -c:r").len(), n("r:common t:instant").len() - n("r:common t:instant c:r").len());
    assert_eq!(n("s:p23"), vec!["Lightning Bolt"]);
    assert_eq!(n("s:m11 cn:101"), vec!["Lightning Bolt"]);
    assert_eq!(n("bolt or girl").len(), 2);
}

#[test]
fn sort_paging_and_json_plan() {
    let (ds, ix) = setup();
    let r = ix.search(&Query { expr: quick::parse("bolt").unwrap(), ..Default::default() });
    // default order: released desc, then name; both bolts exist, order is stable
    assert_eq!(r.total, 2);
    let sorted = ix.search(&Query { sort: vec![SortKey { field: SortField::Mv, desc: true }], limit: Some(1), ..Default::default() });
    assert_eq!(sorted.total, 10);
    let top = &ds.oracle[ds.printings[sorted.rows[0] as usize].oracle_row as usize];
    assert_eq!(top.name.as_deref(), Some("Fire // Ice")); // mv 4
    let past = ix.search(&Query { offset: 99, ..Default::default() });
    assert!(past.rows.is_empty() && past.total == 10);
    let q: Query = serde_json::from_str(r#"{"expr":{"and":[{"color":{"op":":","colors":"r"}},{"not":{"type":"land"}}]},"unique":"cards","limit":5}"#).unwrap();
    assert!(ix.search(&q).total >= 2);
}

#[test]
fn stop_grams_fall_back_to_scan() {
    let (ds, ix) = setup();
    // gate everything: every gram is a stop-gram, so all text queries scan; results must match
    let scan = SearchIndex::build_with(&ds, 0.0, 0);
    assert_eq!(scan.stats().name_grams, 0);
    assert!(ix.stats().name_grams > 0);
    for q in ["bolt", "o:\"3 damage\"", "o:the", "lim-dul", "t:creature -o:damage", "o:zzz"] {
        let query = Query { expr: quick::parse(q).unwrap(), ..Default::default() };
        assert_eq!(ix.search(&query).rows, scan.search(&query).rows, "{q}");
    }
}
