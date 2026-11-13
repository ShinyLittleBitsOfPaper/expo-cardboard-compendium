use coqc_format::{coqd, coqi, patch};
use sha2::{Digest, Sha256};
use std::time::Instant;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let old = std::fs::read(&a[0]).unwrap();
    let p = std::fs::read(&a[1]).unwrap();
    coqd::block_counts_reset();
    let t = Instant::now();
    let new = patch::apply_patch(&old, &p).unwrap();
    let (re, en) = coqd::block_counts();
    println!("apply_patch {:?}: reused {re}, recompressed {en}, sha {:x}", t.elapsed(), Sha256::digest(&new));
    let want = std::fs::read(&a[2]).unwrap();
    println!("matches published today's .coqd: {}", new == want);
    // index delta: yesterday's index + today's delta
    let oz = std::fs::read(&a[3]).unwrap();
    let oi = zstd::stream::decode_all(&oz[..]).unwrap();
    let d = std::fs::read(&a[4]).unwrap();
    let t = Instant::now();
    let ni = coqi::apply_delta(&oi, &d);
    match ni {
        Ok(ni) => {
            let nz = std::fs::read(&a[5]).unwrap();
            let want = zstd::stream::decode_all(&nz[..]).unwrap();
            println!("apply_delta {:?}: ok, matches today's index: {}", t.elapsed(), ni == want);
        }
        Err(e) => println!("apply_delta {:?}: ERR {:?}", t.elapsed(), e),
    }
}
