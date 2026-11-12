use coqc_format::codec::{zstd_exact, Reader};
use std::collections::BTreeSet;
fn main() {
    let p = std::fs::read(std::env::args().nth(1).unwrap()).unwrap();
    let rawlen = u32::from_le_bytes(p[6..10].try_into().unwrap()) as usize;
    let body = zstd_exact(&p[10..], rawlen).unwrap();
    let mut r = Reader::new(&body);
    r.take(64).unwrap();
    let ml = r.uvarint().unwrap() as usize;
    r.take(ml).unwrap();
    for ti in 0..3 {
        let mode = r.u8().unwrap();
        if mode == 1 { println!("table {ti}: columnar"); continue; }
        let n = r.uvarint().unwrap();
        let mut blocks = BTreeSet::new();
        let (mut bytes, mut maxrow) = (0usize, 0u32);
        for _ in 0..n {
            let row = r.u32().unwrap();
            let len = r.uvarint().unwrap() as usize;
            r.take(len).unwrap();
            bytes += len; blocks.insert(row / 256); maxrow = maxrow.max(row);
        }
        println!("table {ti}: {n} rows in patch ({bytes} B raw), touching {} distinct 256-row blocks, max row {maxrow}", blocks.len());
    }
}
