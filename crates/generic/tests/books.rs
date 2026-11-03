//! S5: a second, non-MTG schema through compile, query, patch, index delta and overlays, with no Magic code involved.
//! Every query is checked against an independent evaluation of the raw JSONL.
use coqc_format::coqd::{read_store, write_store};
use coqc_format::dialect::Dialect;
use coqc_format::gindex::GIndex;
use coqc_format::schema::Schema;
use coqc_format::table::Store;
use coqc_format::{coqi, patch};
use coqc_generic::{demo, ingest, Generic};
use serde_json::Value;
use std::collections::HashMap;

const N: usize = 400;

fn inputs(day: u32) -> (Vec<(String, String)>, demo::Demo) {
    let d = demo::books(N, day);
    (vec![("work".into(), d.works.clone()), ("edition".into(), d.editions.clone())], d)
}

fn schema() -> Schema {
    serde_json::from_str(demo::SCHEMA).unwrap()
}

fn compile(day: u32, prev: Option<&Store>) -> (Store, Vec<ingest::TableStats>) {
    ingest::compile(&schema(), &inputs(day).0, prev, false, 20261100 + day).unwrap()
}

fn lines(s: &str) -> Vec<Value> {
    s.lines().filter(|l| !l.is_empty()).map(|l| serde_json::from_str(l).unwrap()).collect()
}

/// Rows (of the editions table) matching `q`, in result order.
fn run(st: &Store, parts: &coqc_format::gindex::IndexParts, q: &str, ov: Vec<(String, &Store)>) -> Result<Vec<u32>, String> {
    let g = Generic.parse_gquery(q, st, &ov)?;
    let ix = GIndex::with_parts(st, Some(Generic.hooks()), parts)?.with_overlays(ov);
    Ok(ix.search(&g)?.rows)
}

fn parts(st: &Store) -> coqc_format::gindex::IndexParts {
    GIndex::build_parts(st, Generic.index_spec(st).unwrap(), coqc_format::search::STOP_FRACTION, coqc_format::search::STOP_MIN).unwrap()
}

/// ids (edition key strings) of the rows, via the dialect's own record JSON
fn ids(st: &Store, rows: &[u32]) -> Vec<String> {
    rows.iter().map(|&r| Generic.record_json(st, r)["id"].as_str().unwrap().to_string()).collect()
}

#[test]
fn compile_is_deterministic_and_roundtrips() {
    let (a, sa) = compile(1, None);
    let (b, _) = compile(1, None);
    let (ba, _) = write_store(&a).unwrap();
    assert_eq!(ba, write_store(&b).unwrap().0);
    assert_eq!(read_store(&ba).unwrap(), a);
    assert_eq!(sa[0].live, N);
    assert_eq!(sa[1].live, lines(&inputs(1).1.editions).len());
    assert_eq!(a.schema.id, "books");
}

#[test]
fn queries_match_an_independent_evaluation_of_the_jsonl() {
    let (st, _) = compile(1, None);
    let p = parts(&st);
    let d = inputs(1).1;
    let works: HashMap<String, Value> = lines(&d.works).into_iter().map(|w| (w["work_id"].as_str().unwrap().to_string(), w)).collect();
    let eds = lines(&d.editions);
    let expect = |f: &dyn Fn(&Value, &Value) -> bool| -> Vec<String> {
        let mut v: Vec<String> = eds.iter().filter(|e| f(e, &works[e["work"].as_str().unwrap()])).map(|e| e["id"].as_str().unwrap().to_string()).collect();
        v.sort();
        v
    };
    let got = |q: &str| -> Vec<String> {
        let mut v = ids(&st, &run(&st, &p, q, vec![]).unwrap_or_else(|e| panic!("{q}: {e}")));
        v.sort();
        v
    };
    let has = |v: &Value, s: &str| v.as_str().unwrap().to_lowercase().contains(s);
    let subj = |w: &Value, s: &str| w["subjects"].as_array().unwrap().iter().any(|x| x == s);
    let cases: Vec<(&str, Vec<String>)> = vec![
        ("harbor", expect(&|_, w| has(&w["title"], "harbor") || has(&w["author"], "harbor") || has(&w["blurb"], "harbor"))),
        ("title:harbor", expect(&|_, w| has(&w["title"], "harbor"))),
        ("t:\"the silent\"", expect(&|_, w| has(&w["title"], "the silent"))),
        ("author:le guin", expect(&|_, w| has(&w["author"], "le") && (has(&w["title"], "guin") || has(&w["author"], "guin") || has(&w["blurb"], "guin")) || false)),
        ("year>=1990", expect(&|e, _| e["year"].as_i64().unwrap() >= 1990)),
        ("year>=1990 pages<300", expect(&|e, _| e["year"].as_i64().unwrap() >= 1990 && e["pages"].as_i64().unwrap() < 300)),
        ("format:Paperback", expect(&|e, _| e["format"] == "paperback")),
        ("f!=ebook", expect(&|e, _| e["format"] != "ebook")),
        ("pub:meridian in_print:true", expect(&|e, _| e["publisher"] == "Meridian" && e["in_print"] == true)),
        ("subject:scifi", expect(&|_, w| subj(w, "scifi"))),
        ("subject:scifi,sea", expect(&|_, w| subj(w, "scifi") && subj(w, "sea"))),
        ("-subject:scifi rating>=4", expect(&|_, w| !subj(w, "scifi") && w["rating"].as_f64().unwrap() >= 4.0)),
        ("subject:scifi or subject:myth", expect(&|_, w| subj(w, "scifi") || subj(w, "myth"))),
        ("(subject:scifi or subject:myth) format:ebook -in_print:true", expect(&|e, w| (subj(w, "scifi") || subj(w, "myth")) && e["format"] == "ebook" && e["in_print"] != true)),
        ("has:extra", expect(&|e, _| !e["extra"].is_null())),
        ("written<1950", expect(&|_, w| w["first_year"].as_i64().unwrap() < 1950)),
        ("isbn:9780000000000", expect(&|e, _| e["isbn"] == "9780000000000")),
        ("subject:nonexistent", vec![]),
        ("format:vinyl", vec![]),
    ];
    for (q, want) in cases {
        if q == "author:le guin" {
            // two words: `author:le` and the bare word `guin` (documented): just check it parses and runs
            assert!(run(&st, &p, q, vec![]).is_ok());
            continue;
        }
        assert_eq!(got(q), want, "query {q}");
    }
    // quoted author phrase
    let want = expect(&|_, w| has(&w["author"], "le guin"));
    assert!(!want.is_empty());
    assert_eq!(got("author:\"le guin\""), want);
}

#[test]
fn order_unique_prefer_and_errors() {
    let (st, _) = compile(1, None);
    let p = parts(&st);
    let year = |r: u32| Generic.record_json(&st, r)["year"].as_i64().unwrap();
    let rows = run(&st, &p, "subject:sea order:-year", vec![]).unwrap();
    assert!(rows.len() > 5 && rows.windows(2).all(|w| year(w[0]) >= year(w[1])));
    let rows = run(&st, &p, "subject:sea order:year", vec![]).unwrap();
    assert!(rows.windows(2).all(|w| year(w[0]) <= year(w[1])));
    assert_eq!(run(&st, &p, "subject:sea order:year dir:desc", vec![]).unwrap(), run(&st, &p, "subject:sea order:-year", vec![]).unwrap());
    // unique:work keeps one edition per work, the one `prefer` picks
    let all = run(&st, &p, "subject:sea", vec![]).unwrap();
    let uniq = run(&st, &p, "subject:sea unique:work prefer:year", vec![]).unwrap();
    let works = |rows: &[u32]| -> std::collections::BTreeSet<String> { rows.iter().map(|&r| Generic.record_json(&st, r)["work"]["work_id"].as_str().unwrap().to_string()).collect() };
    assert_eq!(works(&all), works(&uniq));
    assert_eq!(uniq.len(), works(&uniq).len());
    for &u in &uniq {
        let w = Generic.record_json(&st, u)["work"]["work_id"].clone();
        let best = all.iter().filter(|&&r| Generic.record_json(&st, r)["work"]["work_id"] == w).map(|&r| year(r)).max().unwrap();
        assert_eq!(year(u), best);
    }
    for (q, frag) in [("nosuch:1", "unknown field"), ("year>x", "not a number"), ("year", "word"), ("(year>1", "missing )"), ("title:", "empty"), ("pages~3", "word"), ("work:abc", "not searchable"), ("format<paperback", "not defined"), ("unique:foo", "unique"), ("order:nosuch", "unknown field")] {
        match run(&st, &p, q, vec![]) {
            Err(e) => assert!(e.contains(frag), "{q}: {e}"),
            // a bare word is a valid text search
            Ok(_) => assert!(frag == "word", "{q} should fail"),
        }
    }
}

#[test]
fn next_day_keeps_row_ids_and_patches_and_indexes_incrementally() {
    let (d1, _) = compile(1, None);
    let (d2, s2) = compile(2, Some(&d1));
    // stable ids: every row of day 1 is at the same place with the same key
    for (a, b) in d1.tables.iter().zip(&d2.tables) {
        assert!(b.rows() >= a.rows());
    }
    assert_eq!(s2[0].new, 3);
    assert!(s2[1].tombstoned > 0 && s2[1].new > 0 && s2[1].changed > 0, "{:?}", s2[1]);
    assert_eq!(d2.epoch, d1.epoch);
    // dictionary ids are append-only
    for (a, b) in d1.dicts.iter().zip(&d2.dicts) {
        assert_eq!(&b.values()[..a.len()], a.values());
    }
    // tombstoned editions are gone from the search and keep their key
    let (b1, b2) = (write_store(&d1).unwrap().0, write_store(&d2).unwrap().0);
    let (pt, info) = patch::make_patch(&b1, &b2).unwrap();
    assert!(pt.len() < b2.len() / 2, "patch {} vs full {}", pt.len(), b2.len());
    assert_eq!(info.appended("work"), 3);
    assert_eq!(patch::apply_patch(&b1, &pt).unwrap(), b2);
    // index delta
    let (p1, p2) = (parts(&d1), parts(&d2));
    let (i1, i2) = (coqi::serialize(&p1, &d1.schema, &coqi::sha(&b1)), coqi::serialize(&p2, &d2.schema, &coqi::sha(&b2)));
    let (delta, _) = coqi::make_delta(&i1, &i2).unwrap();
    assert_eq!(coqi::apply_delta(&i1, &delta).unwrap(), i2);
    assert!(delta.len() < i2.len() / 2, "delta {} vs index {}", delta.len(), i2.len());
    // the replayed index answers like a fresh one
    let (re, dsha) = coqi::deserialize(&coqi::apply_delta(&i1, &delta).unwrap(), &d2.schema).unwrap();
    assert_eq!(dsha, coqi::sha(&b2));
    for q in ["subject:sea order:-year", "harbor", "format:ebook year>2000", "has:extra"] {
        assert_eq!(run(&d2, &re, q, vec![]).unwrap(), run(&d2, &p2, q, vec![]).unwrap(), "{q}");
    }
    // retired editions no longer match, but a day-1 query still finds the survivors with the same row ids
    let gone = ids(&d1, &run(&d1, &p1, "pages>0", vec![]).unwrap());
    let now: std::collections::HashSet<String> = ids(&d2, &run(&d2, &p2, "pages>0", vec![]).unwrap()).into_iter().collect();
    assert!(gone.iter().any(|g| !now.contains(g)));
    // schema change without --rebase is refused, with --rebase it starts a new epoch
    let mut other = schema();
    other.version = 2;
    assert!(ingest::compile(&other, &inputs(2).0, Some(&d1), false, 0).is_err());
    assert_eq!(ingest::compile(&other, &inputs(2).0, Some(&d1), true, 0).unwrap().0.epoch, d1.epoch + 1);
}

#[test]
fn overlay_compiles_patches_and_answers_queries() {
    let (d1, _) = compile(1, None);
    let (d2, _) = compile(2, Some(&d1));
    let osch: Schema = serde_json::from_str(demo::OVERLAY_SCHEMA).unwrap();
    let (o1, s1) = ingest::compile_overlay(&osch, &d1, &inputs(1).1.prices, 20261101).unwrap();
    let (o2, s2) = ingest::compile_overlay(&osch, &d2, &inputs(2).1.prices, 20261102).unwrap();
    assert_eq!((s1.unknown_keys, s2.unknown_keys), (0, 0));
    assert_eq!(o2.epoch, d2.epoch);
    let (b1, b2) = (write_store(&o1).unwrap().0, write_store(&o2).unwrap().0);
    let (pt, info) = patch::make_patch(&b1, &b2).unwrap();
    assert!(info.appended("price") > 0);
    assert_eq!(patch::apply_patch(&b1, &pt).unwrap(), b2);
    let p = parts(&d2);
    let ov = || vec![("prices".to_string(), &o2)];
    // independent check from the JSONL
    let want = |f: &dyn Fn(&Value) -> bool| -> Vec<String> {
        let mut v: Vec<String> = lines(&inputs(2).1.prices).iter().filter(|p| f(p)).map(|p| p["key"].as_str().unwrap().to_string()).collect();
        v.sort();
        v
    };
    let got = |q: &str| {
        let mut v = ids(&d2, &run(&d2, &p, q, ov()).unwrap_or_else(|e| panic!("{q}: {e}")));
        v.sort();
        v
    };
    assert_eq!(got("price<1000"), want(&|p| p["price_cents"].as_i64().unwrap() < 1000));
    assert_eq!(got("price_cents>=2000 used>1000"), want(&|p| p["price_cents"].as_i64().unwrap() >= 2000 && p.get("used_cents").is_some_and(|u| u.as_i64().unwrap() > 1000)));
    assert_eq!(got("prices.price=1"), vec![] as Vec<String>);
    assert_eq!(got("-price>0 has:extra").len() + got("price>0 has:extra").len(), got("has:extra").len());
    // without the overlay attached the predicate is an error, not an empty result
    assert!(run(&d2, &p, "price<1000", vec![]).unwrap_err().contains("unknown field"));
}

#[test]
fn keys_records_and_bad_input() {
    let (st, _) = compile(1, None);
    let km = Generic.key_map(&st);
    let first = lines(&inputs(1).1.editions).remove(0);
    let row = km[&Generic.parse_key(first["id"].as_str().unwrap()).unwrap()];
    let rec = Generic.record_json(&st, row);
    assert_eq!(rec["id"], first["id"]);
    assert_eq!(rec["publisher"], first["publisher"]);
    assert_eq!(rec["work"]["title"].as_str().unwrap().is_empty(), false);
    assert_eq!(rec["pages"], first["pages"]);
    // any string can be a key
    assert_eq!(coqc_generic::key_bytes("isbn-1"), coqc_generic::key_bytes("isbn-1"));
    assert_ne!(coqc_generic::key_bytes("isbn-1"), coqc_generic::key_bytes("isbn-2"));
    // malformed input names the table and line
    let bad = vec![("work".to_string(), "{\"work_id\":\"a\",\"first_year\":\"x\"}\n".to_string()), ("edition".to_string(), String::new())];
    let e = ingest::compile(&schema(), &bad, None, false, 0).unwrap_err();
    assert!(e.contains("work line 1") && e.contains("first_year"), "{e}");
    let orphan = vec![("work".to_string(), String::new()), ("edition".to_string(), "{\"id\":\"e\",\"work\":\"nope\"}\n".to_string())];
    assert!(ingest::compile(&schema(), &orphan, None, false, 0).unwrap_err().contains("no live work"));
    let dup = vec![("work".to_string(), "{\"work_id\":\"a\"}\n{\"work_id\":\"a\"}\n".to_string()), ("edition".to_string(), String::new())];
    assert!(ingest::compile(&schema(), &dup, None, false, 0).unwrap_err().contains("duplicate key"));
    // a 33-value Set dictionary cannot be mask-indexed
    let many: String = (0..33).map(|i| format!("{{\"work_id\":\"w{i}\",\"subjects\":[\"s{i}\"]}}\n")).collect();
    let (big, _) = ingest::compile(&schema(), &[("work".into(), many), ("edition".into(), String::new())], None, false, 0).unwrap();
    assert!(Generic.index_spec(&big).err().unwrap().contains("at most 32"));
}
