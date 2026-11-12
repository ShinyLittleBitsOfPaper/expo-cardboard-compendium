use coqc_format::coqd::{read_store, write_store_reusing};
use sha2::{Digest, Sha256};
use std::time::Instant;
fn main() {
    let b = std::fs::read(std::env::args().nth(1).unwrap()).unwrap();
    let t = Instant::now();
    let st = read_store(&b).unwrap();
    println!("read_store {:?}", t.elapsed());
    for tb in &st.tables { println!("table rows {}", tb.rows()); }
    let t = Instant::now();
    let (out, stats) = write_store_reusing(&st, Some(&b)).unwrap();
    println!("write_store {:?}", t.elapsed());
    for s in stats { println!("{} raw {} stored {} blocks {}", s.name, s.raw_bytes, s.stored_bytes, s.blocks); }
    println!("identical bytes: {}  sha: {:x}", out == b, Sha256::digest(&out));
}
