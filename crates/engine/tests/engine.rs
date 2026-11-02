use coqc::{compile, Options};
use coqc_format::coqi;
use coqc_mtg::write;
use coqe::Engine;
use serde_json::{json, Value};
use std::path::PathBuf;

fn fx(n: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../compiler/tests/fixtures").join(n)
}

fn opts() -> Options {
    Options { source_date: 20260701, ..Default::default() }
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("coqe-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn open_search_cards_scope_and_ffi() {
    let dir = tmp("a");
    let (a, _) = compile(&fx("sample_a.jsonl"), None, &opts()).unwrap();
    let (bytes, _) = write(&a).unwrap();
    let (coqd, coqi_p) = (dir.join("c.coqd"), dir.join("c.coqi"));
    std::fs::write(&coqd, &bytes).unwrap();

    let mut e = Engine::open(&coqd, Some(&coqi_p)).unwrap();
    assert_eq!(e.info()["index"], "built");
    assert!(coqi_p.exists(), "index persisted");
    let e2 = Engine::open(&coqd, Some(&coqi_p)).unwrap();
    assert_eq!(e2.info()["index"], "coqi");
    // a .coqs for a different dataset is ignored and rebuilt
    {
        // a .coqi built for a different dataset (other hash in the header) is ignored and rebuilt
        let mut other = coqc::build_index_file(&bytes).unwrap();
        other[6..38].copy_from_slice(&[1u8; 32]);
        std::fs::write(&coqi_p, other).unwrap();
    }
    assert_eq!(Engine::open(&coqd, Some(&coqi_p)).unwrap().info()["index"], "built");

    let r = e.call("search", &json!({"query": "bolt", "cards": true, "limit": 5})).unwrap();
    let n = r["total"].as_u64().unwrap();
    assert!(n >= 1);
    let first = &r["cards"][0];
    assert_eq!(first["name"], "Lightning Bolt");
    let id = first["id"].as_str().unwrap().to_string();
    assert_eq!(e.call("getByScryfallId", &json!({"id": id})).unwrap()["name"], "Lightning Bolt");
    assert_eq!(e.call("getByScryfallId", &json!({"id": "00000000-0000-0000-0000-000000000000"})).unwrap(), Value::Null);
    assert_eq!(e.call("getCards", &json!({"rows": r["rows"]})).unwrap()[0]["id"], first["id"]);

    // scope: only registered printings can match; unknown ids are counted, unknown sets rejected
    let s = e.call("registerSet", &json!({"name": "mine", "ids": [id, "nope"]})).unwrap();
    assert_eq!((s["resolved"].as_u64(), s["unknown"].as_u64()), (Some(1), Some(1)));
    let all = e.call("search", &json!({"query": "t:instant"})).unwrap()["total"].as_u64().unwrap();
    let scoped = e.call("search", &json!({"query": "t:instant", "scope": "mine"})).unwrap();
    assert_eq!(scoped["total"], 1);
    assert!(all > 1);
    assert!(e.call("search", &json!({"query": "x", "scope": "nope"})).is_err());
    assert!(e.call("search", &json!({"query": "frobnicate:1"})).is_err());

    // C ABI wrapper returns {ok}/{error}
    let ok: Value = serde_json::from_str(&coqe::ffi::call_json("open", &json!({"coqd": coqd, "coqi": coqi_p}).to_string())).unwrap();
    assert!(ok["ok"]["printings"].as_u64().unwrap() > 0);
    let s: Value = serde_json::from_str(&coqe::ffi::call_json("search", r#"{"query":"bolt","limit":1}"#)).unwrap();
    assert_eq!(s["ok"]["rows"].as_array().unwrap().len(), 1);
    let bad: Value = serde_json::from_str(&coqe::ffi::call_json("search", "{")).unwrap();
    assert!(bad["error"].is_string());
    coqe::ffi::call_json("close", "");
    let closed: Value = serde_json::from_str(&coqe::ffi::call_json("info", "")).unwrap();
    assert_eq!(closed["error"], "engine not open");
}

#[test]
fn patch_then_index_delta_matches_fresh_build() {
    let dir = tmp("b");
    let (a, _) = compile(&fx("sample_a.jsonl"), None, &opts()).unwrap();
    let (b, _) = compile(&fx("sample_b.jsonl"), Some(&a), &opts()).unwrap();
    let ((ab, _), (bb, _)) = (write(&a).unwrap(), write(&b).unwrap());
    let (coqd, coqi_p) = (dir.join("c.coqd"), dir.join("c.coqi"));
    std::fs::write(&coqd, &ab).unwrap();
    let mut e = Engine::open(&coqd, Some(&coqi_p)).unwrap();
    let before = e.call("search", &json!({"query": "include:extras"})).unwrap()["total"].as_u64().unwrap();

    let (patch, _) = coqc::patch::make_patch(&ab, &bb).unwrap();
    let pp = dir.join("p.coqp");
    std::fs::write(&pp, patch).unwrap();
    let fa = coqc::build_index_file(&ab).unwrap();
    let fb = coqc::build_index_file(&bb).unwrap();
    let (delta, _) = coqi::make_delta(&fa, &fb).unwrap();
    let dp = dir.join("d.coqid");
    std::fs::write(&dp, delta).unwrap();

    // index delta before the dataset patch is refused
    assert!(e.call("applyIndexDelta", &json!({"path": dp})).is_err());
    assert_eq!(e.call("applyPatch", &json!({"path": pp})).unwrap()["index"], "stale");
    assert_eq!(std::fs::read(&coqd).unwrap(), bb, "dataset replays byte-identically");
    assert_eq!(e.call("applyIndexDelta", &json!({"path": dp})).unwrap()["index"], "delta");
    assert_eq!(std::fs::read(&coqi_p).unwrap(), fb, "index replays byte-identically");
    let after = e.call("search", &json!({"query": "include:extras"})).unwrap()["total"].as_u64().unwrap();
    assert!(after > before);

    // skipping the delta is safe: a stale index is rebuilt on the next search
    std::fs::write(&coqd, &ab).unwrap();
    std::fs::write(&coqi_p, &fa).unwrap();
    let mut e = Engine::open(&coqd, Some(&coqi_p)).unwrap();
    e.call("applyPatch", &json!({"path": pp})).unwrap();
    assert_eq!(e.call("search", &json!({"query": "include:extras"})).unwrap()["total"].as_u64().unwrap(), after);
    assert_eq!(e.info()["index"], "built");
}

#[test]
fn file_helpers_hash_and_unzstd() {
    use sha2::{Digest, Sha256};
    let dir = tmp("files");
    std::fs::create_dir_all(&dir).unwrap();
    let raw: Vec<u8> = (0..300_000u32).flat_map(|i| (i % 251).to_le_bytes()).collect();
    let want: String = Sha256::digest(&raw).iter().map(|b| format!("{b:02x}")).collect();
    let (plain, z, out) = (dir.join("a.bin"), dir.join("a.bin.zst"), dir.join("b.bin"));
    std::fs::write(&plain, &raw).unwrap();
    std::fs::write(&z, zstd::encode_all(&raw[..], 3).unwrap()).unwrap();
    // through the JSON ABI, with no engine open
    let r: serde_json::Value = serde_json::from_str(&coqe::ffi::call_json("sha256File", &format!(r#"{{"path":{:?}}}"#, plain))).unwrap();
    assert_eq!((r["ok"]["sha256"].as_str().unwrap(), r["ok"]["size"].as_u64().unwrap()), (want.as_str(), raw.len() as u64));
    let r: serde_json::Value =
        serde_json::from_str(&coqe::ffi::call_json("unzstFile", &format!(r#"{{"path":{:?},"out":{:?}}}"#, z, out))).unwrap();
    assert_eq!(r["ok"]["sha256"].as_str().unwrap(), want);
    assert_eq!(std::fs::read(&out).unwrap(), raw);
    assert!(!dir.join("b.bin.part").exists());
    // garbage in: an error, no output file, no leftover .part
    let bad = dir.join("bad.zst");
    std::fs::write(&bad, b"not zstd at all").unwrap();
    let out2 = dir.join("c.bin");
    let r: serde_json::Value =
        serde_json::from_str(&coqe::ffi::call_json("unzstFile", &format!(r#"{{"path":{:?},"out":{:?}}}"#, bad, out2))).unwrap();
    assert!(r["error"].is_string() && !out2.exists() && !dir.join("c.bin.part").exists());
    let r: serde_json::Value = serde_json::from_str(&coqe::ffi::call_json("sha256File", r#"{"path":"/nonexistent/x"}"#)).unwrap();
    assert!(r["error"].is_string());
    // moveFile replaces an existing target; removeFile of a missing file is not an error
    let (a, b) = (dir.join("m1"), dir.join("m2"));
    std::fs::write(&a, b"new").unwrap();
    std::fs::write(&b, b"old").unwrap();
    let r: serde_json::Value = serde_json::from_str(&coqe::ffi::call_json("moveFile", &format!(r#"{{"from":{:?},"to":{:?}}}"#, a, b))).unwrap();
    assert!(r["ok"].is_null() && std::fs::read(&b).unwrap() == b"new" && !a.exists());
    for _ in 0..2 {
        let r: serde_json::Value = serde_json::from_str(&coqe::ffi::call_json("removeFile", &format!(r#"{{"path":{:?}}}"#, b))).unwrap();
        assert!(r["ok"].is_null());
    }
}

#[test]
fn price_overlay_attach_search_and_patch_file() {
    use coqc_mtg::prices::PriceTable;
    let dir = tmp("p");
    let (a, _) = compile(&fx("sample_a.jsonl"), None, &opts()).unwrap();
    let (bytes, _) = write(&a).unwrap();
    let (coqd, coqi_p, pr) = (dir.join("c.coqd"), dir.join("c.coqi"), dir.join("prices.coqd"));
    std::fs::write(&coqd, &bytes).unwrap();
    let st = coqc_format::coqd::read_store(&bytes).unwrap();
    let rows = st.table("printing").unwrap().1.rows();
    let mk = |bump: u32| {
        let mut t = PriceTable::new(st.epoch, 20261101, rows);
        for i in 0..rows {
            if i % 2 == 0 {
                t.cols[0][i] = 1 + 50 + (i as u32) * 100 + bump; // tcg_normal cents + 1 (0 = no price)
            }
            if i % 3 == 0 {
                t.cols[3][i] = 1 + 80; // cm_normal EUR 0.80
            }
        }
        t
    };
    let (p1, p2) = (mk(0), mk(500));
    let (b1, b2) = (p1.encode().unwrap(), p2.encode().unwrap());
    std::fs::write(&pr, &b1).unwrap();

    let mut e = Engine::open(&coqd, Some(&coqi_p)).unwrap();
    // without the overlay a price query says so instead of matching nothing
    assert!(e.call("search", &json!({"query": "usd>1"})).unwrap_err().contains("prices overlay"));
    let a = e.call("attachOverlay", &json!({"id": "prices", "path": pr})).unwrap();
    assert_eq!(a["overlays"]["prices"]["rows"], rows);
    let count = |t: &PriceTable, col: usize, f: &dyn Fn(u32) -> bool| (0..rows).filter(|&i| t.cents(i, col).is_some_and(f)).count() as u64;
    let q = |e: &mut Engine, s: &str| e.call("search", &json!({"query": format!("{s} include:extras")})).unwrap()["total"].as_u64().unwrap();
    assert_eq!(q(&mut e, "usd>1"), count(&p1, 0, &|c| c > 100));
    assert_eq!(q(&mut e, "usd<=0.5"), count(&p1, 0, &|c| c <= 50));
    assert_eq!(q(&mut e, "eur=0.8"), count(&p1, 3, &|c| c == 80));
    assert_eq!(q(&mut e, "usdfoil>0"), 0, "no foil prices at all");
    assert_eq!(q(&mut e, "usd>1 usd<=0.5"), 0);

    // patch the overlay file without an engine, re-attach: new prices, byte-identical to a fresh encode
    let (patch, _) = coqc::patch::make_patch(&b1, &b2).unwrap();
    let pp = dir.join("p.coqp");
    std::fs::write(&pp, patch).unwrap();
    let out = dir.join("prices.next");
    let r = coqe::files::call("patchFile", &json!({"path": pr, "patch": pp, "out": out})).unwrap().unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), b2);
    assert_eq!(r["size"], b2.len());
    std::fs::rename(&out, &pr).unwrap();
    e.call("attachOverlay", &json!({"id": "prices", "path": pr})).unwrap();
    assert_eq!(q(&mut e, "usd>1"), count(&p2, 0, &|c| c > 100));

    // MTGJSON prices come back Scryfall-shaped, from the overlay only (compiled card data never carries Scryfall prices)
    let gp = e.call("getPrices", &json!({"rows": [0, 1, 99999]})).unwrap();
    let want0 = p2.cents(0, 0).unwrap();
    assert_eq!(gp[0]["usd"], format!("{}.{:02}", want0 / 100, want0 % 100));
    assert_eq!(gp[0]["eur"], "0.80");
    assert!(gp[0]["usd_foil"].is_null() && gp[1]["usd"].is_null() && gp[2].is_null());
    let with = e.call("getRecords", &json!({"rows": [0], "prices": true})).unwrap();
    assert_eq!(with[0]["prices"]["usd"], gp[0]["usd"]);
    assert!(e.call("getRecords", &json!({"rows": [0]})).unwrap()[0].get("prices").is_none());
    let s = e.call("search", &json!({"query": "include:extras", "limit": 2, "cards": true, "prices": true})).unwrap();
    assert!(s["cards"][0]["prices"].is_object());

    // an overlay of another epoch is refused; detaching restores the error
    let mut other = mk(0);
    other.cards_epoch += 1;
    std::fs::write(dir.join("o.coqd"), other.encode().unwrap()).unwrap();
    assert!(e.call("attachOverlay", &json!({"id": "x", "path": dir.join("o.coqd")})).unwrap_err().contains("epoch"));
    e.call("detachOverlay", &json!({"id": "prices"})).unwrap();
    assert!(e.call("search", &json!({"query": "usd>1"})).is_err());
    // generic aliases
    assert!(e.call("getRecords", &json!({"rows": [0]})).unwrap()[0].is_object());
}
