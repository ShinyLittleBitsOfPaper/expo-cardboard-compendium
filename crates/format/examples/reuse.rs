use coqc_format::coqd::{read_store, write_store, write_store_reusing};
use coqc_format::codec::{zstd_exact, Reader};
use std::time::Instant;
fn main() {
    let old = std::fs::read(std::env::args().nth(1).unwrap()).unwrap();
    let mut st = read_store(&old).unwrap();
    // Simulate a daily patch on the biggest table: overwrite 553 scattered rows with other rows' content, and append 20 rows.
    let ti = 2;
    let p = std::fs::read(std::env::args().nth(2).unwrap()).unwrap();
    let rawlen = u32::from_le_bytes(p[6..10].try_into().unwrap()) as usize;
    let body = zstd_exact(&p[10..], rawlen).unwrap();
    let mut r = Reader::new(&body);
    r.take(64).unwrap();
    let ml = r.uvarint().unwrap() as usize;
    r.take(ml).unwrap();
    let mut rows_changed: Vec<u32> = vec![];
    for t in 0..3 {
        r.u8().unwrap();
        let n = r.uvarint().unwrap();
        for _ in 0..n { let row = r.u32().unwrap(); let len = r.uvarint().unwrap() as usize; r.take(len).unwrap(); if t == ti { rows_changed.push(row); } }
    }
    let src = st.tables[ti].clone();
    let n = src.rows();
    // overwrite the real patch's printing rows (same positions, other rows' content) and the real oracle/rank tables' blocks are left alone
    for (k, &i) in rows_changed.iter().enumerate() { if (i as usize) < n { st.tables[ti].set_row_from(i as usize, &src, (k * 91 + 7) % n); } }
    let t = Instant::now();
    let (full, _) = write_store(&st).unwrap();
    println!("full re-encode   {:?}", t.elapsed());
    let t = Instant::now();
    let (reuse, _) = write_store_reusing(&st, Some(&old)).unwrap();
    println!("reuse old blocks {:?}", t.elapsed());
    println!("identical: {}  (len {})", full == reuse, full.len());
    let st2 = read_store(&reuse).unwrap();
    println!("reads back: rows {}", st2.tables[ti].rows());
}
