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
