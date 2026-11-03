//! S5 through the engine: a non-MTG dataset (the synthetic books) opened, searched, looked up, overlaid, patched and
//! index-delta'd exactly like the app would, through the JSON dispatcher. The dialect is picked from the schema id.
use coqc_format::coqd::write_store;
use coqc_format::{coqi, patch};
use coqc_generic::{demo, ingest};
use coqe::Engine;
use serde_json::{json, Value};
use std::path::PathBuf;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("coqe-books-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn books_end_to_end_through_the_engine() {
    let dir = tmp("e2e");
    let schema = serde_json::from_str(demo::SCHEMA).unwrap();
    let osch = serde_json::from_str(demo::OVERLAY_SCHEMA).unwrap();
    let day = |n: u32, prev: Option<&coqc_format::table::Store>| {
        let d = demo::books(300, n);
        ingest::compile(&schema, &[("work".into(), d.works), ("edition".into(), d.editions)], prev, false, 20261100 + n).unwrap().0
    };
    let (s1, ) = (day(1, None),);
    let s2 = day(2, Some(&s1));
    let (b1, b2) = (write_store(&s1).unwrap().0, write_store(&s2).unwrap().0);
    let (i1, i2) = (coqc::build_index_file(&b1).unwrap(), coqc::build_index_file(&b2).unwrap());
    let (p12, _) = patch::make_patch(&b1, &b2).unwrap();
    let (d12, _) = coqi::make_delta(&i1, &i2).unwrap();
    let pr = |n: u32, base: &coqc_format::table::Store| write_store(&ingest::compile_overlay(&osch, base, &demo::books(300, n).prices, 20261100 + n).unwrap().0).unwrap().0;
    let (o1, o2) = (pr(1, &s1), pr(2, &s2));
    let (coqd, coqi_p, ov, pp, dp) = (dir.join("b.coqd"), dir.join("b.coqi"), dir.join("prices.coqd"), dir.join("p.coqp"), dir.join("d.coqid"));
    std::fs::write(&coqd, &b1).unwrap();
    std::fs::write(&coqi_p, &i1).unwrap();
    std::fs::write(&ov, &o1).unwrap();
    std::fs::write(&pp, &p12).unwrap();
    std::fs::write(&dp, &d12).unwrap();

    let mut e = Engine::open(&coqd, Some(&coqi_p)).unwrap();
    let info = e.info();
    assert_eq!(info["index"], "coqi");
    let search = |e: &mut Engine, q: &str| -> Value { e.call("search", &json!({"query": q, "limit": 5, "cards": true})).unwrap_or_else(|er| panic!("{q}: {er}")) };
    let r = search(&mut e, "subject:scifi format:paperback order:-year");
    let total1 = r["total"].as_u64().unwrap();
    assert!(total1 > 0);
    let first = &r["cards"][0];
    assert_eq!(first["format"], "paperback");
    assert!(first["work"]["subjects"].as_array().unwrap().iter().any(|s| s == "scifi"));
    // lookup by natural key (any string a hash or a uuid)
    let id = first["id"].as_str().unwrap();
    assert_eq!(e.call("getByKey", &json!({"id": id})).unwrap()["isbn"], first["isbn"]);
    assert_eq!(e.call("getByKey", &json!({"id": "no-such-book"})).unwrap(), Value::Null);
    // errors are messages, not panics
    assert!(e.call("search", &json!({"query": "nosuch:1"})).unwrap_err().contains("unknown field"));
    // overlay predicates need the overlay
    assert!(e.call("search", &json!({"query": "price<1000"})).is_err());
    e.call("attachOverlay", &json!({"id": "prices", "path": ov.to_str().unwrap()})).unwrap();
    let cheap = search(&mut e, "price<1000 format:ebook");
    assert!(cheap["total"].as_u64().unwrap() > 0);
    // day 2: patch + index delta, then the new rows are searchable and the old overlay is for the same epoch
    e.call("applyPatch", &json!({"path": pp.to_str().unwrap()})).unwrap();
    e.call("applyIndexDelta", &json!({"path": dp.to_str().unwrap()})).unwrap();
    assert_eq!(e.info()["index"], "delta");
    assert_eq!(std::fs::read(&coqd).unwrap(), b2, "patched file on disk equals the day-2 file");
    let total2 = search(&mut e, "subject:scifi format:paperback")["total"].as_u64().unwrap();
    let fresh = {
        std::fs::write(&coqd, &b2).unwrap();
        let mut f = Engine::open(&coqd, None).unwrap();
        search(&mut f, "subject:scifi format:paperback")["total"].as_u64().unwrap()
    };
    assert_eq!(total2, fresh);
    // a replacement overlay for day 2 (full file) attaches and answers
    std::fs::write(&ov, &o2).unwrap();
    e.call("attachOverlay", &json!({"id": "prices", "path": ov.to_str().unwrap()})).unwrap();
    assert!(search(&mut e, "price>=0")["total"].as_u64().unwrap() > 0);
    let _ = std::fs::remove_dir_all(&dir);
}
