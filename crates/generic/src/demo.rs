//! A synthetic "books" dataset for the S5 proof: works (root table) and editions (child table, the search rows) with text,
//! integer, float, bool, enum, set and JSON columns, plus a price overlay (one row per edition). Deterministic: `books(n, day)`
//! is the same bytes every time, later days add works, retire editions, change ratings, pages and prices.
use serde_json::{json, Map, Value};

pub const SCHEMA: &str = r#"{
  "id": "books", "version": 1,
  "dicts": ["subject", "publisher", "format"],
  "tables": [
    {"name": "work", "rel": "None", "key": "work_id", "columnar": false, "columns": [
      {"name": "work_id", "ty": "Uuid", "index": "Exact", "aliases": []},
      {"name": "title", "ty": "Str", "index": "Text", "aliases": ["t"]},
      {"name": "author", "ty": "Str", "index": "Text", "aliases": ["a"]},
      {"name": "blurb", "ty": "Str", "index": "Text", "aliases": []},
      {"name": "subjects", "ty": {"Set": "subject"}, "index": "Set", "aliases": ["subject", "s"]},
      {"name": "first_year", "ty": "Int", "index": "Range", "aliases": ["written"]},
      {"name": "rating", "ty": "Float", "index": "Range", "aliases": []}]},
    {"name": "edition", "rel": {"Parent": "work"}, "key": "id", "columnar": false, "columns": [
      {"name": "id", "ty": "Uuid", "index": "Exact", "aliases": []},
      {"name": "work", "ty": {"Ref": "work"}, "index": "None", "aliases": []},
      {"name": "publisher", "ty": {"Enum": "publisher"}, "index": "Exact", "aliases": ["pub"]},
      {"name": "format", "ty": {"Enum": "format"}, "index": "Exact", "aliases": ["f"]},
      {"name": "year", "ty": "Int", "index": "Range", "aliases": ["y"]},
      {"name": "pages", "ty": "Int", "index": "Range", "aliases": []},
      {"name": "isbn", "ty": "Str", "index": "Exact", "aliases": []},
      {"name": "in_print", "ty": "Bool", "index": "Exact", "aliases": []},
      {"name": "extra", "ty": "Json", "index": "None", "aliases": []}]}
  ]}"#;

pub const OVERLAY_SCHEMA: &str = r#"{
  "id": "books-prices", "version": 1, "dicts": [],
  "tables": [
    {"name": "price", "rel": "None", "key": null, "columnar": true, "columns": [
      {"name": "price_cents", "ty": "Int", "index": "Range", "aliases": ["price"]},
      {"name": "used_cents", "ty": "Int", "index": "Range", "aliases": ["used"]}]}
  ]}"#;

pub struct Demo {
    pub works: String,
    pub editions: String,
    pub prices: String,
}

/// splitmix64 over a few integers: all variation in the dataset comes from here.
fn h(a: u64, b: u64, c: u64) -> u64 {
    let mut z = a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ c.wrapping_mul(0x1656_67B1_9E37_79F9) ^ 0x2545_F491_4F6C_DD1D;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn uuid(kind: u64, a: u64, b: u64) -> String {
    let (x, y) = (h(kind, a, b), h(kind + 100, a, b));
    let s = format!("{x:016x}{y:016x}");
    format!("{}-{}-4{}-a{}-{}", &s[0..8], &s[8..12], &s[13..16], &s[17..20], &s[20..32])
}

const ADJ: [&str; 16] = ["Silent", "Broken", "Hidden", "Last", "Golden", "Distant", "Burning", "Quiet", "Forgotten", "Crooked", "Winter", "Paper", "Iron", "Salt", "Hollow", "Bright"];
const NOUN: [&str; 16] = ["Harbor", "Orchard", "Empire", "Garden", "Library", "River", "Engine", "Compass", "Lantern", "Cartographer", "Tide", "Archive", "Kingdom", "Station", "Forest", "Clockwork"];
const FIRST: [&str; 10] = ["Ursula", "Octavia", "Italo", "Chinua", "Marguerite", "Stanisław", "Jorge", "Toni", "Haruki", "Zadie"];
const LAST: [&str; 10] = ["Le Guin", "Butler", "Calvino", "Achebe", "Yourcenar", "Lem", "Borges", "Morrison", "Murakami", "Smith"];
const SUBJECTS: [&str; 24] = ["scifi", "fantasy", "history", "mystery", "romance", "poetry", "travel", "science", "philosophy", "biography", "horror", "satire", "children", "essays", "war", "sea", "politics", "myth", "crime", "family", "nature", "technology", "art", "religion"];
const PUBLISHERS: [&str; 12] = ["Meridian", "Northgate", "Ashfield", "Orbit Press", "Lantern House", "Saltmarsh", "Quire", "Penhallow", "Redwater", "Highfold", "Tern", "Bellwether"];
const FORMATS: [&str; 5] = ["hardcover", "paperback", "ebook", "audiobook", "mass market"];
const WORDS: [&str; 12] = ["a quiet", "an unlikely", "a sweeping", "the final", "a forgotten", "a fierce", "an intimate", "a restless", "the first", "a strange", "a patient", "a luminous"];

/// Works at `day` (>= 1): `n` from day 1 plus 3 per later day. Editions of work i: 1-4, some retired from a later day on.
pub fn books(n: usize, day: u32) -> Demo {
    let (mut works, mut editions, mut prices) = (String::new(), String::new(), String::new());
    let total = n as u64 + 3 * (day as u64 - 1);
    for i in 0..total {
        let w = h(1, i, 0);
        let title = format!("The {} {}", ADJ[(w % 16) as usize], NOUN[((w >> 8) % 16) as usize]);
        let author = format!("{} {}", FIRST[((w >> 16) % 10) as usize], LAST[((w >> 24) % 10) as usize]);
        let blurb = format!("{} story of {} and {}.", WORDS[((w >> 32) % 12) as usize], NOUN[((w >> 36) % 16) as usize].to_lowercase(), NOUN[((w >> 40) % 16) as usize].to_lowercase());
        let mut subjects: Vec<&str> = (0..1 + (w >> 44) % 3).map(|k| SUBJECTS[(h(2, i, k) % 24) as usize]).collect();
        subjects.sort_unstable();
        subjects.dedup();
        let first_year = 1900 + h(3, i, 0) % 125;
        let mut rating = 100 + (h(4, i, 0) % 400) as i64;
        for d in 2..=day as u64 {
            if h(5, i, d) % 7 == 0 {
                rating = (rating + [-10, 0, 10][(h(6, i, d) % 3) as usize]).clamp(100, 500);
            }
        }
        let wid = uuid(1, i, 0);
        works.push_str(&json!({"work_id": wid, "title": title, "author": author, "blurb": blurb, "subjects": subjects, "first_year": first_year, "rating": rating as f64 / 100.0}).to_string());
        works.push('\n');
        for j in 0..1 + h(7, i, 0) % 4 {
            let retired = (h(8, i, j) % 11 == 0).then(|| 2 + h(9, i, j) % 4);
            if retired.is_some_and(|r| day as u64 >= r) {
                continue;
            }
            let e = h(10, i, j);
            let mut pages = 80 + (e % 820) as i64;
            if (1..day as u64).any(|d| h(11, i * 8 + j, d + 1) % 40 == 0) {
                pages += 12;
            }
            let id = uuid(2, i, j);
            let mut m = Map::new();
            m.insert("id".into(), json!(id));
            m.insert("work".into(), json!(wid));
            m.insert("publisher".into(), json!(PUBLISHERS[((e >> 8) % 12) as usize]));
            m.insert("format".into(), json!(FORMATS[((e >> 16) % 5) as usize]));
            m.insert("year".into(), json!((first_year + (e >> 24) % 40).min(2026)));
            m.insert("pages".into(), json!(pages));
            m.insert("isbn".into(), json!(format!("978{:010}", e % 10_000_000_000)));
            m.insert("in_print".into(), json!(retired.is_none() && (e >> 40) % 6 != 0));
            if (e >> 44) % 5 == 0 {
                m.insert("extra".into(), json!({"series": format!("{} Cycle", NOUN[((e >> 48) % 16) as usize]), "volume": 1 + (e >> 52) % 6}));
            }
            editions.push_str(&Value::Object(m).to_string());
            editions.push('\n');
            if e % 9 != 0 {
                let mut price = 499 + (e % 5000) as i64;
                for d in 2..=day as u64 {
                    if h(12, i * 8 + j, d) % 4 == 0 {
                        price = (price + (h(13, i * 8 + j, d) % 400) as i64 - 200).max(99);
                    }
                }
                let mut p = json!({"key": id, "price_cents": price});
                if e % 3 != 0 {
                    p["used_cents"] = json!(price * 6 / 10);
                }
                prices.push_str(&p.to_string());
                prices.push('\n');
            }
        }
    }
    Demo { works, editions, prices }
}
