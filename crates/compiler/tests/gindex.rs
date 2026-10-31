//! S3a cross-check: the generic index (`coqc_format::gindex`, driven by the schema + MTG derived columns) must return exactly the same
//! rows, in the same order, as the old `SearchIndex` for every query, sort, unique and prefer combination.
use coqc::{compile, Options};
use coqc_format::coqi;
use coqc_format::gindex::{GIndex, IndexParts};
use coqc_format::search::*;
use coqc_mtg::gindex::{derive, derive_store, index_spec, translate, MtgHooks};
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
    // S3b: derived from the Store (auto text columns trimmed), then serialised to a .coqi and reloaded
    let from_store = GIndex::build_spec(&store, index_spec(&store).unwrap(), Some(&MtgHooks)).unwrap();
    let file = coqi::serialize(from_store.parts(), &store.schema, &[9u8; 32]);
    let (reloaded, sha) = coqi::deserialize(&file, &store.schema).unwrap();
    assert_eq!(sha, [9u8; 32]);
    assert_eq!(coqi::serialize(&reloaded, &store.schema, &sha), file, "{label}: serialise/deserialise round trip");
    let again = GIndex::with_parts(&store, Some(&MtgHooks), &reloaded).unwrap();
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
                    let g = translate(&query);
                    for (name, ix) in [("store-derived", &from_store), ("reloaded .coqi", &again)] {
                        let r = ix.search(&g).unwrap_or_else(|e| panic!("{label}: {name}: {q}: {e}"));
                        assert_eq!((want.total, &want.rows), (r.total, &r.rows), "{label}: {name}: {q} (unique {:?}, prefer {prefer:?}, extras {extras})", query.unique);
                    }
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
    let new = GIndex::build_spec(&store, index_spec(&store).unwrap(), Some(&MtgHooks)).unwrap();
    println!("generic derive-from-store+build {:?} (S3b: unsearched auto text columns skipped)", t.elapsed());
    let f = coqi::serialize(new.parts(), &store.schema, &[0u8; 32]);
    println!(".coqi {} bytes raw, {} zstd", f.len(), coqi::compress(&f).len());
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

fn parts_of(ds: &coqc_format::Dataset) -> (coqc_format::table::Store, IndexParts) {
    let store = coqc_mtg::tables::to_store(ds);
    let parts = GIndex::build_parts(&store, index_spec(&store).unwrap(), coqc_format::search::STOP_FRACTION, coqc_format::search::STOP_MIN).unwrap();
    (store, parts)
}

#[test]
fn derive_from_store_equals_derive_from_dataset() {
    for (n, skip) in [("sample_a.jsonl", true), ("sample_b.jsonl", false)] {
        let ds = load(n, skip);
        let store = coqc_mtg::tables::to_store(&ds);
        let (a, b) = (derive(&ds), derive_store(&store).unwrap());
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!((&x.table, &x.name, &x.tri_group), (&y.table, &y.name, &y.tri_group));
            use coqc_format::gindex::DData::*;
            match (&x.data, &y.data) {
                (Text(p), Text(q)) => assert_eq!(p, q, "{}", x.name),
                (Mask(p), Mask(q)) => assert_eq!(p, q, "{}", x.name),
                (Num(p), Num(q)) => assert_eq!(p, q, "{}", x.name),
                (Flag(p), Flag(q)) => assert_eq!(p, q, "{}", x.name),
                _ => panic!("kind differs for {}", x.name),
            }
        }
    }
}

#[test]
fn coqi_rejects_garbage_and_wrong_dataset() {
    let ds = load("sample_a.jsonl", true);
    let (store, parts) = parts_of(&ds);
    let f = coqi::serialize(&parts, &store.schema, &[7u8; 32]);
    assert!(coqi::deserialize(&f[..f.len() / 2], &store.schema).is_err());
    assert!(coqi::deserialize(b"nope", &store.schema).is_err());
    assert!(coqi::deserialize(b"COQS\x01\x00", &store.schema).is_err());
    // columns built for another dataset do not attach
    let other = load("sample_b.jsonl", true);
    let ostore = coqc_mtg::tables::to_store(&other);
    assert!(GIndex::with_parts(&ostore, Some(&MtgHooks), &parts).is_err());
}

#[test]
fn coqi_delta_replays_byte_identically() {
    let a = load("sample_a.jsonl", true);
    let b = compile(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample_b.jsonl"), Some(&a), &Options { source_date: 20260702, skip_digital: true, skip_art_series: true, ..Default::default() }).unwrap().0;
    let (sa, pa) = parts_of(&a);
    let (sb, pb) = parts_of(&b);
    let (fa, fb) = (coqi::serialize(&pa, &sa.schema, &[1u8; 32]), coqi::serialize(&pb, &sb.schema, &[2u8; 32]));
    let (delta, info) = coqi::make_delta(&fa, &fb).unwrap();
    assert!(info.tables.iter().any(|t| t.changed > 0));
    assert!(delta.len() < fb.len());
    assert_eq!(coqi::apply_delta(&fa, &delta).unwrap(), fb);
    assert!(coqi::apply_delta(&fb, &delta).is_err(), "delta only applies to its base");
    let mut bad = delta.clone();
    let n = bad.len();
    bad[n - 1] ^= 0xff;
    assert!(coqi::apply_delta(&fa, &bad).is_err());
    // the replayed file loads and answers like a fresh build
    let (back, _) = coqi::deserialize(&coqi::apply_delta(&fa, &delta).unwrap(), &sb.schema).unwrap();
    let ix = GIndex::with_parts(&sb, Some(&MtgHooks), &back).unwrap();
    let fresh = GIndex::with_parts(&sb, Some(&MtgHooks), &pb).unwrap();
    for q in ["bolt", "t:creature", "o:~", "c:r", "include:extras"] {
        let g = translate(&coqc_mtg::parse::parse(q).unwrap());
        assert_eq!(ix.search(&g).unwrap().rows, fresh.search(&g).unwrap().rows, "{q}");
    }
}
