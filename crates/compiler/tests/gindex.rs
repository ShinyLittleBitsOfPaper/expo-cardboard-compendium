//! S3a cross-check: the generic index (`coqc_format::gindex`, driven by the schema + MTG derived columns) must return exactly the same
//! rows, in the same order, as the old `SearchIndex` for every query, sort, unique and prefer combination.
use coqc::{compile, Options};
use coqc_format::gindex::GIndex;
use coqc_format::search::*;
use coqc_mtg::gindex::{derive, translate, MtgHooks};
use std::path::PathBuf;

const QUERIES: &[&str] = &[
    "bolt", "\"lightning bolt\"", "!\"lightning bolt\"", "!\"fire // ice\"", "!fire", "lim-dul", "delver", "insectile", "fire ice", "stomp", "krrik son", "a", "ab",
    "o:damage", "o:\"damage to any\"", "o:mana", "o:\"top five\"", "o:~", "-o:damage", "o:\"~ enters tapped\"", "o:\"~ deals\"", "fo:~", "fo:\"(\"", "fo:damage",
    "t:instant", "t:creature", "t:human", "t:giant", "t:artifact", "t:creature t:human", "t:instant or t:artifact", "-t:instant", "t:land", "t:legendary t:creature",
    "c:r", "c:u", "c:ur", "c>=ur", "c<=ur", "c<ur", "c>ur", "c=ur", "c!=ur", "c:c", "c:m", "c:w", "-c:r", "c>=2", "c=0", "c<3", "id:r", "id<=ub", "id=gw", "id:c", "id>=2", "id=3",
    "mv=1", "mv>=3", "mv<1", "cmc=0.5", "mv=4", "mv!=2", "mv:even", "mv:odd", "mv>=5",
    "r:common", "r:rare", "r>=rare", "r<rare", "r!=common", "r:mythic", "r:special", "r:bonus", "r:zzz", "r!=zzz", "r>common",
    "s:m11", "e:p23", "s:p23 or s:zzz", "s:lea", "cn:101", "cn:102", "cn:999", "cn>50", "cn<=100", "cn:1",
    "(t:instant or t:creature) -c:r", "t:instant (c:r or c:u)", "bolt -s:p23", "-bolt", "t:legendary t:creature c>=gu",
    "bolt order:name", "bolt order:released direction:desc", "bolt unique:prints", "c:r unique:prints order:cmc", "c:r order:rarity",
    "c:r order:rarity direction:desc", "bolt prefer:newest", "bolt prefer:oldest", "t:instant order:cmc direction:asc", "t:creature unique:cards order:name", "include:extras", "t:plane include:extras",
];

fn check(ds: &coqc_format::Dataset, label: &str) {
    let old = SearchIndex::build(ds);
    let store = coqc_mtg::tables::to_store(ds);
    let new = GIndex::build(&store, derive(ds), Some(&MtgHooks)).unwrap();
    let (mut checked, mut nonempty) = (0, 0);
    for q in QUERIES {
        let Ok(base) = coqc_mtg::parse::parse(q) else { continue };
        for unique in [Unique::Prints, Unique::Cards] {
            for prefer in [Prefer::Newest, Prefer::Oldest] {
                for extras in [false, true] {
                    let mut query = base.clone();
                    query.unique = if base.unique == Unique::Cards { Unique::Cards } else { unique };
                    query.prefer = prefer;
                    query.include_extras = extras || base.include_extras;
                    let want = old.search(&query);
                    let got = new.search(&translate(&query)).unwrap_or_else(|e| panic!("{label}: {q}: {e}"));
                    assert_eq!(want.total, got.total, "{label}: {q} (unique {:?}, extras {extras}): total", query.unique);
                    assert_eq!(want.rows, got.rows, "{label}: {q} (unique {:?}, prefer {prefer:?}, extras {extras}): rows", query.unique);
                    checked += 1;
                    nonempty += (got.total > 0) as usize;
                }
            }
        }
    }
    assert!(checked > 300, "{label}: only {checked} comparisons");
    assert!(nonempty * 4 > checked, "{label}: only {nonempty}/{checked} comparisons returned rows");
}

fn load(name: &str, skip: bool) -> coqc_format::Dataset {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    compile(&p, None, &Options { source_date: 20260701, skip_digital: skip, skip_art_series: skip, ..Default::default() }).unwrap().0
}

#[test]
fn matches_old_index_on_fixtures() {
    for (n, skip) in [("sample_a.jsonl", true), ("sample_a.jsonl", false), ("sample_b.jsonl", true), ("sample_b.jsonl", false)] {
        check(&load(n, skip), &format!("{n} skip={skip}"));
    }
}

#[test]
fn matches_old_index_on_synthetic_data() {
    let dir = std::env::temp_dir().join(format!("coqc-gindex-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("synth.jsonl");
    coqc_mtg::synth::generate(&p, 4000, 1).unwrap();
    let ds = compile(&p, None, &Options { source_date: 20260701, ..Default::default() }).unwrap().0;
    check(&ds, "synthetic");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn generic_query_json_roundtrip_and_errors() {
    let ds = load("sample_a.jsonl", true);
    let store = coqc_mtg::tables::to_store(&ds);
    let ix = GIndex::build(&store, derive(&ds), Some(&MtgHooks)).unwrap();
    let q = translate(&coqc_mtg::parse::parse("t:creature c:r").unwrap());
    let back: coqc_format::gindex::GQuery = serde_json::from_str(&serde_json::to_string(&q).unwrap()).unwrap();
    assert_eq!(ix.search(&q).unwrap().rows, ix.search(&back).unwrap().rows);
    // a schema column with no dialect help works generically (alias `artist` is a Text column on `printing`)
    assert!(ix.search(&coqc_format::gindex::GQuery { expr: Some(coqc_format::gindex::GExpr::Text { field: "artist".into(), needle: "zzzz".into() }), ..Default::default() }).unwrap().rows.is_empty());
    assert!(ix.search(&coqc_format::gindex::GQuery { expr: Some(coqc_format::gindex::GExpr::Text { field: "nope".into(), needle: "x".into() }), ..Default::default() }).is_err());
}

/// `cargo test -p coqc --release --test gindex -- --ignored --nocapture`: build and query timings on 120k synthetic printings.
#[test]
#[ignore]
fn timings_old_vs_generic() {
    use std::time::Instant;
    let dir = std::env::temp_dir().join(format!("coqc-gindex-t-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("synth.jsonl");
    coqc_mtg::synth::generate(&p, 120_000, 1).unwrap();
    let ds = compile(&p, None, &Options { source_date: 20260701, ..Default::default() }).unwrap().0;
    let t = Instant::now();
    let old = SearchIndex::build(&ds);
    println!("old build {:?}", t.elapsed());
    let store = coqc_mtg::tables::to_store(&ds);
    let t = Instant::now();
    let d = derive(&ds);
    let new = GIndex::build(&store, d, Some(&MtgHooks)).unwrap();
    println!("generic derive+build {:?} (includes auto text columns for every Text column in the schema)", t.elapsed());
    for q in ["bolt", "t:creature", "c:rg", "o:flying", "mv>=5", "r:mythic", "s:lea", "-t:land", "t:legendary t:creature c>=gu"] {
        let query = coqc_mtg::parse::parse(q).unwrap();
        let gq = translate(&query);
        let (mut a, mut b) = (0.0, 0.0);
        for _ in 0..5 {
            let t = Instant::now();
            let x = old.search(&query);
            a += t.elapsed().as_secs_f64() * 200.0;
            let t = Instant::now();
            let y = new.search(&gq).unwrap();
            b += t.elapsed().as_secs_f64() * 200.0;
            assert_eq!(x.rows, y.rows);
        }
        println!("{q:35} old {a:7.2} ms   generic {b:7.2} ms");
    }
    std::fs::remove_dir_all(&dir).ok();
}
