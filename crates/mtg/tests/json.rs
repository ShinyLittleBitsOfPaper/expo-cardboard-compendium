#[test]
fn slug_matches_scryfall_style() {
    use coqc_mtg::json::slug;
    assert_eq!(slug("Forest"), "forest");
    assert_eq!(slug("Jinnie Fay, Jetmir's Second"), "jinnie-fay-jetmirs-second");
    assert_eq!(slug("Fire // Ice"), "fire-ice");
}
