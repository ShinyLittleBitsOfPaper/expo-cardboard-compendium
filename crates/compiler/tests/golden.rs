//! S0 golden tests: lock today's MTG search behaviour and container size before the schema-driven rewrite.
//! Regenerate on purpose with `UPDATE_GOLDEN=1 cargo test -p coqc --test golden`; any other diff is a regression.
use coqc::{compile, quick, Options};
use coqc_format::search::*;
use std::path::PathBuf;

const QUERIES: &[&str] = &[
    "bolt", "\"lightning bolt\"", "!\"lightning bolt\"", "lim-dul", "delver", "insectile", "fire ice", "stomp",
    "o:damage", "o:\"damage to any\"", "o:mana", "o:\"top five\"", "o:~", "-o:damage",
    "t:instant", "t:creature", "t:human", "t:giant", "t:artifact", "t:creature t:human", "t:instant or t:artifact", "-t:instant",
    "c:r", "c:u", "c:ur", "c>=ur", "c<=ur", "c:c", "c:m", "c:w", "-c:r", "id:r", "id<=ub", "id=gw", "id:c",
    "mv=1", "mv>=3", "mv<1", "cmc=0.5", "mv=4",
    "r:common", "r:rare", "r>=rare", "r<rare", "s:m11", "e:p23", "s:p23 or s:zzz", "cn:101", "cn:102", "cn:999",
    "(t:instant or t:creature) -c:r", "t:instant (c:r or c:u)", "bolt -s:p23", "-bolt",
    "bolt order:name", "bolt order:released direction:desc", "bolt unique:prints", "c:r unique:prints order:cmc",
    "bolt prefer:newest", "bolt prefer:oldest", "t:instant order:cmc direction:asc",
];

fn run() -> (String, usize) {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample_a.jsonl");
    let (ds, _) = compile(&p, None, &Options { source_date: 20260701, skip_digital: true, skip_art_series: true, ..Default::default() }).unwrap();
    let size = coqc_format::container::write(&ds).unwrap().0.len();
    let ix = SearchIndex::build(&ds);
    let mut out = String::new();
    for q in QUERIES {
        for (label, unique) in [("cards", Unique::Cards), ("prints", Unique::Prints)] {
            match quick::parse(q) {
                Err(e) => out.push_str(&format!("{q} [{label}] => PARSE ERROR {e}\n")),
                Ok(expr) => {
                    let r = ix.search(&Query { expr, unique, ..Default::default() });
                    let names: Vec<String> = r.rows.iter().map(|&p| {
                        let pr = &ds.printings[p as usize];
                        format!("{}#{}", ds.oracle[pr.oracle_row as usize].name.clone().unwrap_or_default(), p)
                    }).collect();
                    out.push_str(&format!("{q} [{label}] => {} | {}\n", names.len(), names.join(", ")));
                }
            }
        }
    }
    (out, size)
}

#[test]
fn queries_and_size_match_golden() {
    let (out, size) = run();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let qf = dir.join("queries.txt");
    let sf = dir.join("size.txt");
    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::write(&qf, &out).unwrap();
        std::fs::write(&sf, size.to_string()).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&qf).expect("golden missing: run with UPDATE_GOLDEN=1");
    assert_eq!(out, want, "search results changed");
    let want_size: f64 = std::fs::read_to_string(&sf).unwrap().trim().parse().unwrap();
    let ratio = size as f64 / want_size;
    assert!((0.95..=1.05).contains(&ratio), "container size {size} vs golden {want_size} (outside +-5%)");
}
