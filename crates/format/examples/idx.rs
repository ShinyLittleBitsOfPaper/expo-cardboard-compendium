use coqc_format::coqi::{apply_delta, make_delta};
use std::time::Instant;
fn main() {
    let z = std::fs::read(std::env::args().nth(1).unwrap()).unwrap();
    let t = Instant::now();
    let idx = zstd::stream::decode_all(&z[..]).unwrap();
    println!("unzst {:?} ({} B)", t.elapsed(), idx.len());
    let (d, _) = make_delta(&idx, &idx).unwrap();
    let t = Instant::now();
    let out = apply_delta(&idx, &d).unwrap();
    println!("apply_delta (no-op delta, same decode/rebuild/sha work) {:?}, same: {}", t.elapsed(), out == idx);
    let real = std::fs::read(std::env::args().nth(2).unwrap()).unwrap();
    println!("real delta file {} B, magic {:?}", real.len(), &real[..4]);
}
