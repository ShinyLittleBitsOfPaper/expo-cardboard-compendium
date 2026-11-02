//! Audit of the query shapes slbop's own parser (src/utils/searchParser.ts) issues, run through the engine (Milestone 3.4).
use coqc::{compile, Options};
use coqc_mtg::write;
use coqe::Engine;
use serde_json::json;
use std::path::PathBuf;

fn engine(name: &str) -> Engine {
    let fx = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../compiler/tests/fixtures/sample_a.jsonl");
    let (a, _) = compile(&fx, None, &Options { source_date: 20260701, ..Default::default() }).unwrap();
    let (bytes, _) = write(&a).unwrap();
    let d = std::env::temp_dir().join(format!("coqe-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("c.coqd"), &bytes).unwrap();
    Engine::open(&d.join("c.coqd"), Some(&d.join("c.coqi"))).unwrap()
}

fn total(e: &mut Engine, q: &str) -> u64 {
    e.call("search", &json!({"query": q})).unwrap_or_else(|m| panic!("{q}: {m}"))["total"].as_u64().unwrap()
}

/// Every grammar form of searchParser.ts that maps 1:1 onto Scryfall syntax parses, is case-insensitive and agrees with its aliases.
#[test]
fn slbop_grammar_forms_parse_and_aliases_agree() {
    let mut e = engine("q1");
    for q in [
        "bolt", "-bolt", "t:creature", "t=\"legendary creature\"", "o:flying", "o=\"flying\"", "c:wu", "c=wu", "c>wu", "c<=wu", "c<wu", "c:colorless", "c:c", "id:wu", "id<=wu",
        "id:azorius", "ci:bant", "c:jeskai", "id>=3", "c=2", "r:rare", "r>uncommon", "r<mythic", "r=mythic", "s:m11", "cn:108", "mv:3", "mv=3", "mv>=4", "cmc<2", "-t:vampire",
        "-c:wu", "-r:mythic", "-(s:m11 cn:108)",
    ] {
        e.call("search", &json!({"query": q})).unwrap_or_else(|m| panic!("{q}: {m}"));
    }
    for (a, b) in [("T:CREATURE", "t:creature"), ("C:WU", "c:wu"), ("ID<=WU", "id<=wu"), ("MV>=2", "mv>=2"), ("S:M11", "s:m11"), ("commander:wu", "id:wu"), ("identity:wu", "ci:wu"), ("type:creature", "t:creature"), ("colors:wu", "c:wu"), ("cmc>=2", "mv>=2"), ("number:108 include:extras", "cn:108 include:extras"), ("set:m11", "s:m11")] {
        assert_eq!(total(&mut e, a), total(&mut e, b), "{a} vs {b}");
    }
}

/// slbop lists EVERY printing (tokens and other extras included), newest first, then name, then a stable row order.
#[test]
fn app_listing_is_all_printings_newest_first() {
    let mut e = engine("q2");
    let all = e.call("info", &json!({})).unwrap()["printings"].as_u64().unwrap();
    assert_eq!(total(&mut e, "unique:prints include:extras"), all, "unique:prints include:extras lists every live printing");
    assert!(total(&mut e, "") <= all);
    let r = e.call("search", &json!({"query": "unique:prints include:extras order:released direction:desc", "cards": true})).unwrap();
    let cards = r["cards"].as_array().unwrap();
    assert_eq!(cards.len() as u64, all);
    let key = |c: &serde_json::Value| (c["released_at"].as_str().unwrap_or("").to_string(), c["name"].as_str().unwrap_or("").to_string());
    for w in cards.windows(2) {
        let (a, b) = (key(&w[0]), key(&w[1]));
        assert!(a.0 > b.0 || (a.0 == b.0 && a.1.to_lowercase() <= b.1.to_lowercase()), "order broken between {a:?} and {b:?}");
    }
    // the same query is deterministic page by page
    let p1 = e.call("search", &json!({"query": "unique:prints include:extras order:released direction:desc", "limit": 4, "offset": 0})).unwrap();
    let p2 = e.call("search", &json!({"query": "unique:prints include:extras order:released direction:desc", "limit": 4, "offset": 4})).unwrap();
    let mut joined = p1["rows"].as_array().unwrap().clone();
    joined.extend(p2["rows"].as_array().unwrap().clone());
    assert_eq!(joined[..], r["rows"].as_array().unwrap()[..joined.len()]);
}

/// Not Scryfall syntax: these stay in the app (documented in docs/search.md), and the engine says so instead of guessing.
#[test]
fn app_only_terms_are_errors_or_text() {
    let mut e = engine("q3");
    assert!(e.call("search", &json!({"query": "tag:foo"})).unwrap_err().contains("tag"));
    assert_eq!(total(&mut e, "m11#108 include:extras"), 0, "set#number is plain text to the engine");
}
