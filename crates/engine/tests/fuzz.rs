//! Milestone 3.5: malformed-input tests. Every reader (COQD, COQI, COQP patch, COQI delta, price overlay, idmap) and the engine's
//! C-ABI entry point must return an error, never panic or hang, on truncated, bit-flipped and random input. Deterministic
//! (seeded) mutation fuzzing over real fixtures; no extra dependencies.
use coqc::{build_index_file, compile, patch::make_patch, Options};
use coqc_format::{coqd, coqi, patch};
use coqc_mtg::{prices::{load_idmap, PriceTable}, write};
use coqe::{ffi::call_json, Engine};
use serde_json::json;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Mutations of `seed_bytes`: every truncation length class, single/multi byte flips (biased to the header), splices, zero/0xFF runs, garbage.
fn mutants(seed_bytes: &[u8], n: usize, salt: u64) -> Vec<Vec<u8>> {
    let mut r = Rng(0x9E3779B97F4A7C15 ^ salt);
    let len = seed_bytes.len();
    let mut out: Vec<Vec<u8>> = vec![vec![], vec![0], seed_bytes[..len.min(3)].to_vec(), seed_bytes[..len.min(8)].to_vec()];
    for k in 0..64.min(len) {
        out.push(seed_bytes[..k * len / 64].to_vec()); // truncations
    }
    for _ in 0..n {
        let mut m = seed_bytes.to_vec();
        match r.below(6) {
            0 => {
                let i = r.below(m.len());
                m[i] ^= 1 << r.below(8);
            }
            1 => {
                for _ in 0..1 + r.below(8) {
                    let i = r.below(m.len().min(96)); // header / section table
                    m[i] = r.next() as u8;
                }
            }
            2 => {
                let (i, l) = (r.below(m.len()), 1 + r.below(64));
                let end = (i + l).min(m.len());
                m[i..end].fill(if r.below(2) == 0 { 0 } else { 0xFF });
            }
            3 => {
                let cut = r.below(m.len());
                m.truncate(cut);
                m.extend((0..r.below(32)).map(|_| r.next() as u8));
            }
            4 => {
                let (a, b) = (r.below(m.len()), r.below(m.len()));
                let (a, b) = (a.min(b), a.max(b));
                let chunk = m[a..b].to_vec();
                let at = r.below(m.len());
                m.splice(at..at, chunk);
            }
            _ => m = (0..r.below(512)).map(|_| r.next() as u8).collect(),
        }
        out.push(m);
    }
    out
}

/// Run `f` on every mutant; panics are collected and reported with the mutant index. Also fails when a single call takes > 5 s.
fn fuzz(name: &str, seed_bytes: &[u8], n: usize, mut f: impl FnMut(&[u8])) {
    let mut bad = vec![];
    for (i, m) in mutants(seed_bytes, n, name.len() as u64 * 7919).iter().enumerate() {
        if std::env::var_os("FUZZ_TRACE").is_some() {
            eprintln!("{name} #{i} len {}", m.len());
        }
        let t = std::time::Instant::now();
        if catch_unwind(AssertUnwindSafe(|| f(m))).is_err() {
            bad.push(format!("#{i} ({} bytes)", m.len()));
        }
        assert!(t.elapsed().as_secs() < 5, "{name}: mutant #{i} took {:?}", t.elapsed());
    }
    assert!(bad.is_empty(), "{name}: {} panics, e.g. {:?}", bad.len(), &bad[..bad.len().min(5)]);
}

fn fx(n: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../compiler/tests/fixtures").join(n)
}

struct World {
    a: Vec<u8>,
    b: Vec<u8>,
    ia: Vec<u8>,
    ib: Vec<u8>,
    patch: Vec<u8>,
    delta: Vec<u8>,
    prices: Vec<u8>,
    prices2: Vec<u8>,
    pprice: Vec<u8>,
}

fn world() -> World {
    let o = Options { source_date: 20260701, ..Default::default() };
    let (a, _) = compile(&fx("sample_a.jsonl"), None, &o).unwrap();
    let (b, _) = compile(&fx("sample_b.jsonl"), Some(&a), &o).unwrap();
    let ((ab, _), (bb, _)) = (write(&a).unwrap(), write(&b).unwrap());
    let (ia, ib) = (build_index_file(&ab).unwrap(), build_index_file(&bb).unwrap());
    let (p, _) = make_patch(&ab, &bb).unwrap();
    let (d, _) = coqi::make_delta(&ia, &ib).unwrap();
    let st = coqd::read_store(&ab).unwrap();
    let rows = st.table("printing").unwrap().1.rows();
    let mut t1 = PriceTable::new(st.epoch, 20261101, rows);
    let mut t2 = t1.clone();
    for i in 0..rows {
        t1.cols[0][i] = 1 + 100 * i as u32;
        t2.cols[0][i] = 1 + 100 * i as u32 + 7;
        t2.cols[3][i] = 1 + 5;
    }
    let (p1, p2) = (t1.encode().unwrap(), t2.encode().unwrap());
    let (pp, _) = make_patch(&p1, &p2).unwrap();
    World { a: ab, b: bb, ia, ib, patch: p, delta: d, prices: p1, prices2: p2, pprice: pp }
}

#[test]
fn readers_never_panic_on_malformed_input() {
    let w = world();
    let schema = coqd::read_store(&w.a).unwrap().schema;
    let (m, _) = (&w.a, ());
    fuzz("coqd", m, 3000, |b| {
        let _ = coqd::read_store(b);
    });
    fuzz("coqi", &w.ia, 1500, |b| {
        let _ = coqi::deserialize(b, &schema);
    });
    fuzz("price overlay", &w.prices, 3000, |b| {
        let _ = PriceTable::decode(b);
        let _ = coqd::read_store(b);
    });
    fuzz("idmap", b"COQM\x01\x00\x00\x00\x00\x00\x00\x00", 500, |b| {
        let _ = load_idmap(b);
    });
}

#[test]
fn patches_and_deltas_never_panic_on_malformed_input() {
    let w = world();
    // a damaged patch against the right base, and the right patch against damaged bases
    fuzz("coqp patch", &w.patch, 3000, |p| {
        let _ = patch::apply_patch(&w.a, p);
    });
    fuzz("coqp base", &w.a, 1500, |base| {
        let _ = patch::apply_patch(base, &w.patch);
    });
    fuzz("price patch", &w.pprice, 3000, |p| {
        let _ = patch::apply_patch(&w.prices, p);
    });
    fuzz("price patch base", &w.prices, 1000, |base| {
        let _ = patch::apply_patch(base, &w.pprice);
    });
    fuzz("coqid delta", &w.delta, 3000, |d| {
        let _ = coqi::apply_delta(&w.ia, d);
    });
    fuzz("coqid base", &w.ia, 1000, |base| {
        let _ = coqi::apply_delta(base, &w.delta);
    });
    // sanity: the unmutated inputs do work (the fuzzers above are not just feeding errors)
    assert_eq!(patch::apply_patch(&w.a, &w.patch).unwrap(), w.b);
    assert_eq!(coqi::apply_delta(&w.ia, &w.delta).unwrap(), w.ib);
    assert_eq!(patch::apply_patch(&w.prices, &w.pprice).unwrap(), w.prices2);
}

/// Opening, attaching, patching and searching with damaged files through the engine and the C-ABI entry point returns errors only.
#[test]
fn engine_and_c_abi_survive_damaged_files_and_garbage_calls() {
    let w = world();
    let d = std::env::temp_dir().join(format!("coqe-fuzz-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let (coqd_p, coqi_p, ov, pp, dp) = (d.join("c.coqd"), d.join("c.coqi"), d.join("prices.coqd"), d.join("x.coqp"), d.join("x.coqid"));
    std::fs::write(&coqd_p, &w.a).unwrap();
    std::fs::write(&coqi_p, &w.ia).unwrap();
    fuzz("engine open (damaged coqd)", &w.a, 300, |b| {
        std::fs::write(&coqd_p, b).unwrap();
        let _ = Engine::open(&coqd_p, Some(&coqi_p));
    });
    std::fs::write(&coqd_p, &w.a).unwrap();
    fuzz("engine open (damaged coqi)", &w.ia, 300, |b| {
        std::fs::write(&coqi_p, b).unwrap();
        if let Ok(mut e) = Engine::open(&coqd_p, Some(&coqi_p)) {
            // a damaged index must be rebuilt, not trusted: searching still works
            e.call("search", &json!({"query": "t:creature"})).expect("search after rebuild");
        }
    });
    std::fs::write(&coqi_p, &w.ia).unwrap();
    let mut e = Engine::open(&coqd_p, Some(&coqi_p)).unwrap();
    fuzz("attachOverlay (damaged)", &w.prices, 300, |b| {
        std::fs::write(&ov, b).unwrap();
        let _ = e.call("attachOverlay", &json!({"id": "prices", "path": ov}));
    });
    fuzz("applyIndexDelta (damaged)", &w.delta, 200, |b| {
        std::fs::write(&dp, b).unwrap();
        let _ = e.call("applyIndexDelta", &json!({"path": dp}));
    });
    fuzz("patchFile (damaged)", &w.pprice, 200, |b| {
        std::fs::write(&pp, b).unwrap();
        std::fs::write(&ov, &w.prices).unwrap();
        let _ = coqe::files::call("patchFile", &json!({"path": ov, "patch": pp, "out": d.join("out.coqd")}));
    });
    // the engine is still usable after all that
    assert!(e.call("search", &json!({"query": "bolt"})).is_ok());

    // C ABI: unknown methods, wrong argument types and junk JSON come back as {"error":...}, never a panic
    let mut r = Rng(42);
    for m in ["", "nope", "open", "search", "getRecords", "getByKeys", "registerRows", "attachOverlay", "patchFile", "unzstFile", "saveSets", "loadSets", "getPrices", "rowsOf", "applyPatch", "hashImage", "matchArtHash"] {
        for a in ["", "{}", "[]", "null", "{\"query\":5}", "{\"rows\":\"x\"}", "{\"rows\":[-1,1e99,null,\"a\"]}", "{\"path\":\"/nonexistent/\\u0000\"}", "{\"ids\":[1,2]}", "{", "\u{0}"] {
            let out = call_json(m, a);
            let v: serde_json::Value = serde_json::from_str(&out).unwrap_or_else(|_| panic!("{m} {a}: not JSON: {out}"));
            assert!(v.get("ok").is_some() || v.get("error").is_some(), "{m} {a}: {out}");
        }
    }
    for _ in 0..300 {
        let q: String = (0..r.below(40)).map(|_| char::from_u32(32 + r.below(95) as u32).unwrap()).collect();
        let _ = e.call("search", &json!({"query": q, "limit": r.below(5), "offset": r.below(10)}));
    }
}
