use coqc_format::schema::Schema;
use coqc_format::codec::{Reader, Writer};
use coqc_mtg::tables::{from_store, mtg_schema, to_store};
use coqc_mtg::{compile, Options};
use std::path::PathBuf;

fn ds() -> coqc_format::Dataset {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../compiler/tests/fixtures/sample_a.jsonl");
    compile(&p, None, &Options { source_date: 20260701, ..Default::default() }).unwrap().0
}

#[test]
fn schema_is_valid_and_binary_and_json_roundtrip() {
    let s = mtg_schema();
    s.validate().unwrap();
    let mut w = Writer::new();
    s.encode(&mut w);
    assert_eq!(Schema::decode(&mut Reader::new(&w.buf)).unwrap(), s);
    let j = serde_json::to_string(&s).unwrap();
    assert_eq!(serde_json::from_str::<Schema>(&j).unwrap(), s);
}

#[test]
fn schema_validation_rejects_bad_schemas() {
    let mut s = mtg_schema();
    s.tables[2].key = Some("name".into());
    assert!(s.validate().is_err());
    let mut s = mtg_schema();
    s.tables[2].columns[2].ty = coqc_format::schema::ColType::Enum("nope".into());
    assert!(s.validate().is_err());
    let mut s = mtg_schema();
    s.tables[2].columns.retain(|c| c.name != "oracle_row");
    assert!(s.validate().is_err());
}

#[test]
fn dataset_survives_store_roundtrip() {
    let d = ds();
    let st = to_store(&d);
    st.check().unwrap();
    assert_eq!(st.tables[0].rows(), d.oracle.len());
    assert_eq!(st.tables[2].rows(), d.printings.len());
    let back = from_store(&st).unwrap();
    assert_eq!(back.meta, d.meta);
    assert_eq!(back.oracle, d.oracle);
    assert_eq!(back.ranks, d.ranks);
    assert_eq!(back.printings, d.printings);
}

#[test]
fn store_check_catches_corruption() {
    let mut st = to_store(&ds());
    if let coqc_format::table::ColData::Ref(v) = &mut st.tables[2].cols[1] {
        v[0] = 9999;
    }
    assert!(st.check().is_err());
}

#[test]
fn v3_file_roundtrips_tombstones_and_is_deterministic() {
    let mut d = ds();
    let n = d.printings.len();
    d.printings.push(coqc_format::PrintingRecord::tombstone([7; 16]));
    d.printings.push(coqc_format::PrintingRecord { scryfall_id: [9; 16], oracle_row: 0, ..d.printings[0].clone() });
    let (a, stats) = coqc_mtg::write(&d).unwrap();
    assert_eq!(a, coqc_mtg::write(&d).unwrap().0);
    assert!(stats.iter().any(|s| s.name == "printing_keys") && !stats.iter().any(|s| s.name == "oracle_ranks_keys"));
    let back = coqc_mtg::read(&a).unwrap();
    assert_eq!(back.printings, d.printings);
    assert!(back.printings[n].tombstone && back.printings[n].scryfall_id == [7; 16]);
    assert_eq!(&a[..4], b"COQD");
    assert_eq!(u16::from_le_bytes([a[4], a[5]]), 3);
}

#[test]
fn patch_carries_changed_tombstoned_and_appended_rows() {
    let old = ds();
    let mut new = old.clone();
    new.printings[0].artist = Some("Someone Else".into());
    new.printings[1] = coqc_format::PrintingRecord::tombstone(new.printings[1].scryfall_id);
    new.ranks[0].edhrec = Some(1);
    new.printings.push(coqc_format::PrintingRecord { scryfall_id: [9; 16], ..new.printings[0].clone() });
    let (ob, nb) = (coqc_mtg::write(&old).unwrap().0, coqc_mtg::write(&new).unwrap().0);
    let (patch, info) = coqc_format::patch::make_patch(&ob, &nb).unwrap();
    assert_eq!((info.changed("printing"), info.appended("printing"), info.changed("oracle_ranks")), (2, 1, 1));
    assert_eq!(coqc_format::patch::apply_patch(&ob, &patch).unwrap(), nb);
}
