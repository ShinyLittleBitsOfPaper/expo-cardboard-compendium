use coqc::{compile, quick, Options};
use coqc_format::search::*;
use std::path::PathBuf;

fn setup() -> (coqc_format::Dataset, SearchIndex) {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample_a.jsonl");
    let (ds, _) = compile(&p, None, &Options { source_date: 20260701, skip_digital: true, skip_art_series: true, ..Default::default() }).unwrap();
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

#[test]
fn scryfall_semantics_leaves() {
    let (ds, ix) = setup();
    let run = |j: &str| -> Vec<String> {
        let q: Query = serde_json::from_str(&format!("{{\"expr\":{j},\"unique\":\"cards\"}}")).unwrap();
        ix.search(&q).rows.iter().map(|&p| ds.oracle[ds.printings[p as usize].oracle_row as usize].name.clone().unwrap()).collect()
    };
    // id: bare `:` is subset (id<=); colorless fits anywhere
    assert!(run(r#"{"identity":{"op":":","colors":"ur"}}"#).contains(&"Colorless Rock".to_string()));
    assert!(!run(r#"{"identity":{"op":":","colors":"u"}}"#).contains(&"Lim-Dûl's Vault".to_string()));
    // color counts
    assert!(run(r#"{"colorcount":{"op":">=","value":2}}"#).contains(&"Fire // Ice".to_string()));
    assert_eq!(run(r#"{"identitycount":{"op":"=","value":0}}"#).iter().filter(|n| *n == "Colorless Rock").count(), 1);
    // !"name": full name or one face, folded
    assert_eq!(run(r#"{"nameexact":"lightning bolt"}"#), vec!["Lightning Bolt"]);
    assert_eq!(run(r#"{"nameexact":"fire"}"#), vec!["Fire // Ice"]);
    assert_eq!(run(r#"{"nameexact":"bolt"}"#).len(), 0);
    // rarity order: ... rare < special < mythic < bonus
    assert!(run(r#"{"rarity":{"op":">","value":"rare"}}"#).len() >= run(r#"{"rarity":{"op":">=","value":"mythic"}}"#).len());
}

#[test]
fn name_punctuation_tilde_reminder_and_prefer() {
    let (ds, _) = setup();
    assert_eq!(names(&ds, "\"lim-duls vault\"", Unique::Cards), vec!["Lim-Dûl's Vault"]);
    assert_eq!(names(&ds, "\"lim-dul's, vault\"", Unique::Cards), vec!["Lim-Dûl's Vault"]);
    assert_eq!(names(&ds, "o:\"~ deals 2 damage\"", Unique::Cards), vec!["Fire // Ice"]);
    assert_eq!(names(&ds, "o:\"~ deals 3\"", Unique::Cards), vec!["Lightning Bolt"]);
    assert_eq!(coqc_format::search::strip_reminder("Flying (It can't be blocked.)\nHaste (Go.) now"), "Flying\nHaste now");
    // unique:cards keeps the newest printing before sorting
    let (_, ix) = setup();
    let q = coqc_mtg::parse::parse("!\"lightning bolt\"").unwrap();
    let r = ix.search(&q);
    assert_eq!(r.rows.len(), 1);
    let all = ix.search(&Query { expr: q.expr.clone(), unique: Unique::Prints, ..Default::default() });
    assert_eq!(r.rows[0], all.rows[0]); // default sort released desc => newest first
}

#[test]
fn extras_hidden_unless_revealed() {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample_a.jsonl");
    let (ds, _) = compile(&p, None, &Options { source_date: 20260701, ..Default::default() }).unwrap();
    let ix = SearchIndex::build(&ds);
    let count = |q: &str| ix.search(&coqc_mtg::parse::parse(q).unwrap()).total;
    let hidden = count("");
    let shown = count("include:extras");
    assert!(shown > hidden, "art_series fixture card must be hidden by default ({hidden} vs {shown})");
}

#[test]
fn coqs_round_trip_and_delta() {
    use coqc_format::search::coqs;
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let opts = Options { source_date: 20260701, ..Default::default() };
    let (a, _) = compile(&dir.join("sample_a.jsonl"), None, &opts).unwrap();
    let ia = SearchIndex::build(&a);
    let fa = coqs::serialize(&ia, &[7u8; 32]);
    let (back, sha) = coqs::deserialize(&fa).unwrap();
    assert_eq!(sha, [7u8; 32]);
    assert_eq!(coqs::serialize(&back, &sha), fa);
    for q in ["bolt", "o:\"3 damage\"", "t:creature -o:damage", "c>=ur", "mv<=1", "s:m11", "include:extras", "!\"fire // ice\""] {
        let query = coqc_mtg::parse::parse(q).unwrap();
        assert_eq!(ia.search(&query).rows, back.search(&query).rows, "{q}");
    }
    // corrupt input is rejected, not a panic
    assert!(coqs::deserialize(&fa[..fa.len() / 2]).is_err());
    assert!(coqs::deserialize(b"nope").is_err());

    // delta: next-day dataset built on top of the first (stable rows, appended cards)
    let b_path = dir.join("sample_b.jsonl");
    if !b_path.exists() {
        return;
    }
    let (b, _) = compile(&b_path, Some(&a), &opts).unwrap();
    let fb = coqs::serialize(&SearchIndex::build(&b), &[8u8; 32]);
    let (delta, info) = coqs::make_delta(&fa, &fb).unwrap();
    assert!(!info.tables.is_empty());
    assert_eq!(coqs::apply_delta(&fa, &delta).unwrap(), fb);
    // wrong base is refused
    assert!(coqs::apply_delta(&fb, &delta).is_err());
}
