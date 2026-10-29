//! Deterministic synthetic bulk data shaped like Scryfall's `default_cards`.
//!
//! Exists so the pipeline can be exercised at realistic scale (and "daily
//! churn" modelled) without network access. It is NOT real data: sizes and
//! timings from it are indicative only; confirm with the real bulk file.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn uuid(kind: u64, n: u64) -> String {
    let mut s = kind.wrapping_mul(0x1234_5678_9ABC_DEF1) ^ n.wrapping_mul(0xD6E8_FEB8_6659_FD93);
    let (a, b) = (splitmix(&mut s), splitmix(&mut s));
    format!(
        "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
        (a >> 32) as u32,
        (a >> 16) as u16,
        a & 0xfff,
        (b >> 48) & 0xfff,
        b & 0xffff_ffff_ffff
    )
}

const WORDS: [&str; 24] = [
    "creature", "target", "damage", "draw", "card", "graveyard", "battlefield", "exile", "counter", "spell",
    "tap", "untap", "library", "hand", "player", "opponent", "token", "enchant", "sacrifice", "flying",
    "trample", "destroy", "return", "owner",
];
const FORMATS: [&str; 23] = [
    "standard", "future", "historic", "timeless", "gladiator", "pioneer", "modern", "legacy", "pauper",
    "vintage", "penny", "commander", "oathbreaker", "standardbrawl", "brawl", "competitivebrawl", "alchemy",
    "paupercommander", "duel", "oldschool", "premodern", "predh", "tlr",
];

fn text(seed: u64, words: usize) -> String {
    let mut s = seed;
    (0..words).map(|_| WORDS[(splitmix(&mut s) % 24) as usize]).collect::<Vec<_>>().join(" ")
}

fn img(size: &str, side: &str, id: &str, stamp: u64) -> String {
    let ext = if size == "png" { "png" } else { "jpg" };
    format!("https://cards.scryfall.io/{size}/{side}/{}/{}/{id}.{ext}?{stamp}", &id[0..1], &id[1..2])
}

fn images_json(id: &str, side: &str, stamp: u64) -> String {
    let parts: Vec<String> = ["small", "normal", "large", "png", "art_crop", "border_crop"]
        .iter()
        .map(|s| format!("\"{s}\":\"{}\"", img(s, side, id, stamp)))
        .collect();
    format!("{{{}}}", parts.join(","))
}

/// Writes `n` base printings plus `day * n/200` new ones; on `day >= 2` some
/// printings disappear, some change, and a third of oracle `edhrec_rank`s move
/// (modelling real volatility). Returns the number of cards written.
pub fn generate(path: &Path, n: u64, day: u64) -> std::io::Result<u64> {
    let mut w = BufWriter::new(File::create(path)?);
    let total = n + day * (n / 200).max(1);
    let oracles = (n / 4).max(1);
    let mut written = 0;
    for i in 0..total {
        if day >= 2 && i % 1000 == 7 {
            continue; // removed
        }
        let o = i % oracles;
        let id = uuid(1, i);
        let oid = uuid(2, o);
        let set = format!("s{:02}", i / 2000 % 100);
        let layout = if o % 40 == 0 { "transform" } else if o % 55 == 0 { "split" } else { "normal" };
        let mut s = o * 7919 + 11;
        let cmc = (splitmix(&mut s) % 9) as f64;
        let rank = if day >= 2 && o % 3 == 0 { o * 13 % 20000 + day } else { o * 13 % 20000 };
        let leg: Vec<String> = FORMATS
            .iter()
            .enumerate()
            .map(|(k, f)| format!("\"{f}\":\"{}\"", if (o + k as u64) % 5 == 0 { "not_legal" } else { "legal" }))
            .collect();
        let stamp = 1_700_000_000 + (i % 400) * 86400;
        let flavor = if day >= 2 && i % 500 == 3 { format!("changed {day}") } else { text(i, 6) };
        let name = format!("Card {o} of {}", text(o, 2));
        let day_str = format!("20{:02}-{:02}-{:02}", 10 + (i / 20000) % 15, 1 + i % 12, 1 + i % 28);
        let faces = if layout == "transform" {
            format!(
                ",\"card_faces\":[{{\"object\":\"card_face\",\"name\":\"{name} front\",\"mana_cost\":\"{{1}}{{G}}\",\"type_line\":\"Creature\",\"oracle_text\":\"{}\",\"power\":\"2\",\"toughness\":\"2\",\"colors\":[\"G\"],\"artist\":\"Artist {}\",\"image_uris\":{}}},{{\"object\":\"card_face\",\"name\":\"{name} back\",\"type_line\":\"Creature\",\"oracle_text\":\"{}\",\"power\":\"3\",\"toughness\":\"3\",\"colors\":[\"G\"],\"artist\":\"Artist {}\",\"image_uris\":{}}}]",
                text(o + 1, 20), i % 300, images_json(&id, "front", stamp),
                text(o + 2, 20), i % 300, images_json(&id, "back", stamp),
            )
        } else {
            String::new()
        };
        let top_image = if layout == "transform" {
            String::new()
        } else {
            format!(",\"image_uris\":{}", images_json(&id, "front", stamp))
        };
        let top_text = if layout == "transform" {
            String::new()
        } else {
            format!(",\"mana_cost\":\"{{2}}{{R}}\",\"oracle_text\":\"{}\",\"power\":\"2\",\"toughness\":\"3\",\"colors\":[\"R\"]", text(o + 5, 25))
        };
        let line = format!(
            "{{\"object\":\"card\",\"id\":\"{id}\",\"oracle_id\":\"{oid}\",\"multiverse_ids\":[{}],\"mtgo_id\":{},\"tcgplayer_id\":{},\"cardmarket_id\":{},\"name\":\"{name}\",\"lang\":\"en\",\"released_at\":\"{day_str}\",\"uri\":\"https://api.scryfall.com/cards/{id}\",\"scryfall_uri\":\"https://scryfall.com/card/{set}/{}/card-{o}?utm_source=api\",\"layout\":\"{layout}\",\"highres_image\":true,\"image_status\":\"highres_scan\",\"cmc\":{cmc:.1},\"type_line\":\"Creature\",\"color_identity\":[\"R\"],\"keywords\":[\"Flying\"],\"legalities\":{{{}}},\"games\":[\"paper\",\"mtgo\"],\"reserved\":false,\"foil\":true,\"nonfoil\":true,\"finishes\":[\"nonfoil\",\"foil\"],\"oversized\":false,\"promo\":false,\"reprint\":{},\"variation\":false,\"set_id\":\"{}\",\"set\":\"{set}\",\"set_name\":\"Set {set}\",\"set_type\":\"expansion\",\"set_uri\":\"https://api.scryfall.com/sets/{}\",\"set_search_uri\":\"https://api.scryfall.com/cards/search?order=set&q=e%3A{set}&unique=prints\",\"scryfall_set_uri\":\"https://scryfall.com/sets/{set}?utm_source=api\",\"rulings_uri\":\"https://api.scryfall.com/cards/{id}/rulings\",\"prints_search_uri\":\"https://api.scryfall.com/cards/search?order=released&q=oracleid%3A{oid}&unique=prints\",\"collector_number\":\"{}\",\"digital\":false,\"rarity\":\"common\",\"flavor_text\":\"{flavor}\",\"card_back_id\":\"0aeebaf5-8c7d-4636-9e82-8c27447861f7\",\"artist\":\"Artist {}\",\"artist_ids\":[\"{}\"],\"illustration_id\":\"{}\",\"border_color\":\"black\",\"frame\":\"2015\",\"full_art\":false,\"textless\":false,\"booster\":true,\"story_spotlight\":false,\"edhrec_rank\":{rank},\"prices\":{{\"usd\":\"{}.{:02}\",\"usd_foil\":null,\"eur\":\"1.00\"}},\"related_uris\":{{\"gatherer\":\"https://gatherer.wizards.com/Pages/Card/Details.aspx?multiverseid={}\",\"tcgplayer_infinite_articles\":\"https://tcgplayer.com/search/articles?q={o}\",\"edhrec\":\"https://edhrec.com/route/?cc={o}\"}},\"purchase_uris\":{{\"tcgplayer\":\"https://www.tcgplayer.com/product/{}?utm_campaign=affiliate\",\"cardmarket\":\"https://www.cardmarket.com/en/Magic/Products/Search?referrer=scryfall&searchString={o}\",\"cardhoarder\":\"https://www.cardhoarder.com/cards/{}?affiliate_id=scryfall\"}}{top_text}{top_image}{faces}}}",
            1000 + i,
            50000 + i,
            60000 + i,
            70000 + i,
            i % 400,
            leg.join(","),
            i % 3 == 0,
            uuid(4, i / 2000),
            uuid(4, i / 2000),
            i % 400,
            i % 300,
            uuid(5, i % 300),
            uuid(6, i),
            (splitmix(&mut s) % 20) + (day % 3),
            i % 100,
            1000 + i,
            60000 + i,
            50000 + i,
        );
        w.write_all(line.as_bytes())?;
        w.write_all(b"\n")?;
        written += 1;
    }
    w.flush()?;
    Ok(written)
}
