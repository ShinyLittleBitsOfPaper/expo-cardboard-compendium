//! S3d: regex, pow/tou/loy, m:, is:/not:, f:/banned:/restricted:, kw:, a:/ft: on the fixture.
use coqc::{compile, Options};
use coqc_mtg::query::*;
use std::path::PathBuf;

mod common;
use common::Ix;

fn names(q: &str) -> Vec<String> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample_a.jsonl");
    let (ds, _) = compile(&p, None, &Options { source_date: 20260701, skip_digital: true, skip_art_series: true, ..Default::default() }).unwrap();
    let ix = Ix::build(&ds);
    let mut parsed = coqc_mtg::parse::parse(q).unwrap();
    parsed.unique = Unique::Cards;
    let r = ix.search(&parsed);
    let mut v: Vec<String> = r.rows.iter().map(|&p| ds.oracle[ds.printings[p as usize].oracle_row as usize].name.clone().unwrap()).collect();
    v.sort();
    v
}

fn n(q: &str) -> Vec<String> {
    names(q)
}

#[test]
fn regex() {
    assert_eq!(n("name:/bolt|ice/"), ["Fire // Ice", "Lightning Bolt"]);
    assert_eq!(n("o:/\\d damage/"), ["Fire // Ice", "Lightning Bolt"]);
    assert_eq!(n("t:/^instant$/"), ["Lightning Bolt", "Lim-Dûl's Vault", "Weird One"]);
    assert_eq!(n("name:/BOLT/"), n("name:/bolt/")); // case-insensitive
    assert_eq!(n("name:/lightning bolt/"), ["Lightning Bolt"]); // literal phrase with a space
    assert_eq!(n("-name:/bolt|ice/").len() + 2, n("-name:/zzzz/").len());
    assert!(coqc_mtg::parse::parse("c:/r/").is_err());
    assert!(coqc_mtg::parse::parse("name:/abc").is_err());
}

#[test]
fn bad_regex_is_an_error_not_a_panic() {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample_a.jsonl");
    let (ds, _) = compile(&p, None, &Options { source_date: 20260701, skip_digital: true, skip_art_series: true, ..Default::default() }).unwrap();
    let ix = Ix::build(&ds);
    let q = coqc_mtg::gindex::translate(&coqc_mtg::parse::parse("name:/(/").unwrap());
    let e = ix.index().search(&q).unwrap_err();
    assert!(e.contains("bad regex"), "{e}");
    // a pattern that would need a huge automaton is refused, not run
    let q = coqc_mtg::gindex::translate(&coqc_mtg::parse::parse("name:/(a{100}){100}{100}/").unwrap());
    assert!(ix.index().search(&q).is_err());
}

#[test]
fn power_toughness_loyalty() {
    assert_eq!(n("pow>=3"), ["Bonecrusher Giant // Stomp", "Delver of Secrets // Insectile Aberration"]); // any face
    assert_eq!(n("tou<2"), ["Delver of Secrets // Insectile Aberration"]);
    assert_eq!(n("pow>tou"), ["Bonecrusher Giant // Stomp", "Delver of Secrets // Insectile Aberration"]);
    assert_eq!(n("pow>mv"), n("pow>cmc"));
    assert!(n("loy>0").is_empty());
    assert!(n("power=3").len() <= n("pow>=3").len());
}

#[test]
fn mana_cost() {
    assert_eq!(n("m:{1}{R}"), ["Bonecrusher Giant // Stomp", "Fire // Ice"]); // Stomp {1}{R}, Fire {1}{R}
    assert_eq!(n("m:{R}"), n("m:r"));
    assert_eq!(n("m=r"), ["Colorless Rock", "Lightning Bolt", "Lim-Dûl's Vault", "Weird One"]);
    assert_eq!(n("m:1R"), n("m:{1}{R}")); // shorthand
    assert!(coqc_mtg::parse::parse("m:{").is_err());
    assert!(coqc_mtg::parse::parse("m:!").is_err());
}

#[test]
fn is_flags() {
    assert_eq!(n("is:dfc"), ["Delver of Secrets // Insectile Aberration", "Reversible // Card"]);
    assert_eq!(n("is:split"), ["Fire // Ice"]);
    assert_eq!(n("is:transform"), ["Delver of Secrets // Insectile Aberration"]);
    assert_eq!(n("is:adventure"), ["Bonecrusher Giant // Stomp"]);
    assert_eq!(n("is:promo"), ["Lightning Bolt"]);
    assert_eq!(n("is:gold"), ["Fire // Ice", "Lim-Dûl's Vault", "Weird One"]);
    assert_eq!(n("is:hybrid"), ["Little Girl"]);
    assert_eq!(n("is:historic"), ["Colorless Rock"]);
    assert_eq!(n("not:foil"), ["Weird One"]);
    assert_eq!(n("-is:foil"), n("not:foil"));
    assert!(coqc_mtg::parse::parse("is:nonsense").is_err());
    assert!(coqc_mtg::parse::parse("not:nonsense").is_err());
}

#[test]
fn formats_keywords_artist() {
    assert_eq!(n("banned:brawl").len(), 9);
    assert_eq!(n("restricted:oldschool").len(), 9);
    assert!(n("legal:pauper").is_empty());
    assert_eq!(n("f:edh"), n("f:commander"));
    assert_eq!(n("kw:transform"), ["Delver of Secrets // Insectile Aberration"]);
    assert_eq!(n("kw:Adventure"), ["Bonecrusher Giant // Stomp"]);
    assert!(n("kw:flying").is_empty());
    assert_eq!(n("a:SOME").len(), 9);
    assert!(n("a:nobody").is_empty());
    assert!(n("ft:zzzzz").is_empty());
}

#[test]
fn unknown_format_is_an_error() {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample_a.jsonl");
    let (ds, _) = compile(&p, None, &Options { source_date: 20260701, skip_digital: true, skip_art_series: true, ..Default::default() }).unwrap();
    let ix = Ix::build(&ds);
    let q = coqc_mtg::gindex::translate(&coqc_mtg::parse::parse("f:notaformat").unwrap());
    assert!(ix.index().search(&q).unwrap_err().contains("unknown format"));
}

#[test]
fn index_without_s3d_columns_is_refused() {
    use coqc_format::gindex::{GIndex, IndexSpec};
    use coqc_mtg::gindex::{index_spec, MtgHooks};
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample_a.jsonl");
    let (ds, _) = compile(&p, None, &Options { source_date: 20260701, skip_digital: true, skip_art_series: true, ..Default::default() }).unwrap();
    let store = coqc_mtg::tables::to_store(&ds);
    let full = index_spec(&store).unwrap();
    let old = IndexSpec {
        derived: full.derived.into_iter().filter(|d| !matches!(d.name.as_str(), "mana_f" | "pt_f" | "is_o" | "is_p")).collect(),
        skip_auto: full.skip_auto,
    };
    let parts = GIndex::build_parts(&store, old, 0.25, 8).unwrap();
    let e = GIndex::with_parts(&store, Some(&MtgHooks), &parts).err().expect("must refuse");
    assert!(e.contains("older version"), "{e}");
}
