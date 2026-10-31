//! `manifest.json`: the one file clients fetch (fixed URL on the rolling `data-latest` release) to learn what is
//! current and how to get there. It lists the current full files, the format versions they need, and the chain of
//! patches from older releases (independent chains for cards and prices, since prices can change without cards).
//!
//! Pure logic: `build` takes already-measured assets and the previous manifest, so it is unit-testable without files.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MANIFEST_VERSION: u32 = 1;
/// Tag of the rolling release that holds the manifest and the current full files.
pub const LATEST_TAG: &str = "data-latest";

pub fn sha_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

/// The search index ships zstd-compressed (GitHub does not compress release assets); the raw fields describe the
/// decompressed `.coqi`, which is what an index delta's base/target hashes refer to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexAsset {
    #[serde(flatten)]
    pub file: Asset,
    pub encoding: String,
    pub raw_size: u64,
    pub raw_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardPatch {
    pub from: String,
    pub to: String,
    /// When this patch was published (ISO 8601 UTC); used to expire old links of the chain.
    pub at: String,
    pub patch: Asset,
    pub index_delta: Option<Asset>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PricePatch {
    pub from: String,
    pub to: String,
    pub at: String,
    pub patch: Asset,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cards {
    pub stamp: String,
    pub format: u16,
    pub min_reader: u16,
    pub file: Asset,
    pub index: IndexAsset,
    pub patches: Vec<CardPatch>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prices {
    pub stamp: String,
    pub format: u16,
    pub min_reader: u16,
    pub file: Asset,
    pub patches: Vec<PricePatch>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub manifest_version: u32,
    pub generated_at: String,
    /// Tag of the immutable release that holds this run's patches.
    pub run: String,
    /// Cards epoch (row-id space). A client on another epoch cannot patch: it downloads the full files.
    pub epoch: u32,
    /// True when no patch chain leads here from earlier releases (first release or a new epoch / format).
    pub rebase: bool,
    /// Smallest module format version able to read these files; above the module's own => "update the app".
    pub min_client_format: MinFormat,
    pub cards: Cards,
    pub prices: Prices,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MinFormat {
    pub coqd: u16,
    pub coqi: u16,
    pub coqr: u16,
}

/// What `build` needs to know about one measured file.
pub struct Measured {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

pub struct IndexMeasured {
    pub file: Measured,
    pub raw_size: u64,
    pub raw_sha256: String,
}

pub struct NewCards {
    pub stamp: String,
    pub epoch: u32,
    pub format: u16,
    pub min_reader: u16,
    pub file: Measured,
    pub index: IndexMeasured,
    pub index_version: u16,
    /// Patch from the previous release, with the index delta that goes with it.
    pub patch: Option<(Measured, Option<Measured>)>,
}

pub struct NewPrices {
    pub stamp: String,
    pub format: u16,
    pub min_reader: u16,
    pub file: Measured,
    pub patch: Option<Measured>,
}

pub struct Inputs {
    /// `https://github.com/OWNER/REPO/releases/download`
    pub base_url: String,
    pub run_tag: String,
    pub generated_at: String,
    /// Chain links published before this instant (ISO 8601 UTC, compared as text) are dropped.
    pub keep_since: String,
    pub force_rebase: bool,
    pub cards: NewCards,
    pub prices: NewPrices,
}

fn asset(base: &str, tag: &str, m: &Measured) -> Asset {
    Asset { name: m.name.clone(), url: format!("{base}/{tag}/{}", m.name), size: m.size, sha256: m.sha256.clone() }
}

/// `cards-<from>-to-<to>.coqp` -> (from, to). Also works for `prices-` and any extension.
pub fn patch_stamps(name: &str, prefix: &str) -> Option<(String, String)> {
    let stem = name.strip_prefix(prefix)?.rsplit_once('.')?.0;
    let (a, b) = stem.split_once("-to-")?;
    (!a.is_empty() && !b.is_empty()).then(|| (a.to_string(), b.to_string()))
}

pub fn build(inp: &Inputs, prev: Option<&Manifest>) -> Result<Manifest, String> {
    let base = inp.base_url.trim_end_matches('/');
    let c = &inp.cards;
    let p = &inp.prices;
    let min_client_format = MinFormat { coqd: c.min_reader, coqi: c.index_version, coqr: p.min_reader };

    // A usable previous manifest has the same epoch and card format; otherwise clients must start over.
    let prev_ok = prev.filter(|m| m.epoch == c.epoch && m.cards.format == c.format);
    let rebase = inp.force_rebase || prev_ok.is_none();

    let keep = |at: &str| at >= inp.keep_since.as_str();
    let mut card_patches: Vec<CardPatch> = Vec::new();
    if let (false, Some(pm)) = (rebase, prev_ok) {
        card_patches = pm.cards.patches.iter().filter(|x| keep(&x.at)).cloned().collect();
        if pm.cards.stamp != c.stamp {
            let (pf, pi) = c.patch.as_ref().ok_or_else(|| {
                format!("cards moved {} -> {} but no patch was given (use --rebase to start a new chain)", pm.cards.stamp, c.stamp)
            })?;
            let (from, to) = patch_stamps(&pf.name, "cards-").ok_or_else(|| format!("bad patch name {}", pf.name))?;
            if from != pm.cards.stamp || to != c.stamp {
                return Err(format!("patch {} does not lead {} -> {}", pf.name, pm.cards.stamp, c.stamp));
            }
            card_patches.push(CardPatch {
                from,
                to,
                at: inp.generated_at.clone(),
                patch: asset(base, &inp.run_tag, pf),
                index_delta: pi.as_ref().map(|d| asset(base, &inp.run_tag, d)),
            });
        }
    }

    let prev_prices = prev.filter(|m| m.prices.format == p.format);
    let mut price_patches: Vec<PricePatch> = Vec::new();
    if let (false, Some(pm)) = (inp.force_rebase, prev_prices) {
        price_patches = pm.prices.patches.iter().filter(|x| keep(&x.at)).cloned().collect();
        if pm.prices.stamp != p.stamp {
            let pf = p.patch.as_ref().ok_or_else(|| {
                format!("prices moved {} -> {} but no patch was given", pm.prices.stamp, p.stamp)
            })?;
            let (from, to) = patch_stamps(&pf.name, "prices-").ok_or_else(|| format!("bad patch name {}", pf.name))?;
            if from != pm.prices.stamp || to != p.stamp {
                return Err(format!("patch {} does not lead {} -> {}", pf.name, pm.prices.stamp, p.stamp));
            }
            price_patches.push(PricePatch { from, to, at: inp.generated_at.clone(), patch: asset(base, &inp.run_tag, pf) });
        }
    }

    Ok(Manifest {
        manifest_version: MANIFEST_VERSION,
        generated_at: inp.generated_at.clone(),
        run: inp.run_tag.clone(),
        epoch: c.epoch,
        rebase,
        min_client_format,
        cards: Cards {
            stamp: c.stamp.clone(),
            format: c.format,
            min_reader: c.min_reader,
            file: asset(base, LATEST_TAG, &c.file),
            index: IndexAsset {
                file: asset(base, LATEST_TAG, &c.index.file),
                encoding: "zstd".into(),
                raw_size: c.index.raw_size,
                raw_sha256: c.index.raw_sha256.clone(),
            },
            patches: card_patches,
        },
        prices: Prices {
            stamp: p.stamp.clone(),
            format: p.format,
            min_reader: p.min_reader,
            file: asset(base, LATEST_TAG, &p.file),
            patches: price_patches,
        },
    })
}

/// Names of the full files a manifest points at in `data-latest` (everything else there may be deleted).
pub fn current_files(m: &Manifest) -> Vec<String> {
    vec![m.cards.file.name.clone(), m.cards.index.file.name.clone(), m.prices.file.name.clone()]
}

/// Names of every asset that belongs in this run's immutable `data-<run>` release.
pub fn run_files(m: &Manifest) -> Vec<String> {
    let mut v = Vec::new();
    for x in &m.cards.patches {
        if x.patch.url.contains(&format!("/{}/", m.run)) {
            v.push(x.patch.name.clone());
            if let Some(d) = &x.index_delta {
                v.push(d.name.clone());
            }
        }
    }
    for x in &m.prices.patches {
        if x.patch.url.contains(&format!("/{}/", m.run)) {
            v.push(x.patch.name.clone());
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me(name: &str) -> Measured {
        Measured { name: name.into(), size: 10, sha256: sha_hex(name.as_bytes()) }
    }
    fn inputs(stamp: &str, prev_stamp: &str, pstamp: &str, pprev: &str, at: &str, epoch: u32) -> Inputs {
        Inputs {
            base_url: "https://example.test/o/r/releases/download/".into(),
            run_tag: format!("data-run{at}"),
            generated_at: format!("2026-10-{at}T22:00:00Z"),
            keep_since: "2026-10-05T00:00:00Z".into(),
            force_rebase: false,
            cards: NewCards {
                stamp: stamp.into(),
                epoch,
                format: 2,
                min_reader: 2,
                file: me(&format!("cards-{stamp}.coqd")),
                index: IndexMeasured { file: me(&format!("cards-{stamp}.coqi.zst")), raw_size: 100, raw_sha256: "ab".into() },
                index_version: 1,
                patch: Some((me(&format!("cards-{prev_stamp}-to-{stamp}.coqp")), Some(me(&format!("cards-{prev_stamp}-to-{stamp}.coqid"))))),
            },
            prices: NewPrices {
                stamp: pstamp.into(),
                format: 1,
                min_reader: 1,
                file: me(&format!("prices-{pstamp}.coqr")),
                patch: Some(me(&format!("prices-{pprev}-to-{pstamp}.coqx"))),
            },
        }
    }

    #[test]
    fn stamps_parse() {
        assert_eq!(patch_stamps("cards-A1-to-B2.coqp", "cards-"), Some(("A1".into(), "B2".into())));
        assert_eq!(patch_stamps("prices-20261024-to-20261027.coqx", "prices-"), Some(("20261024".into(), "20261027".into())));
        assert_eq!(patch_stamps("cards-A1.coqp", "cards-"), None);
        assert_eq!(patch_stamps("prices-A-to-B.coqx", "cards-"), None);
    }

    #[test]
    fn first_release_is_a_rebase_with_empty_chains() {
        let m = build(&inputs("S1", "S0", "20261024", "20261023", "24", 1), None).unwrap();
        assert!(m.rebase && m.cards.patches.is_empty() && m.prices.patches.is_empty());
        assert_eq!(m.cards.file.url, "https://example.test/o/r/releases/download/data-latest/cards-S1.coqd");
        assert_eq!(m.cards.index.encoding, "zstd");
        assert_eq!(current_files(&m).len(), 3);
        assert!(run_files(&m).is_empty());
    }

    #[test]
    fn chain_grows_links_and_expires_old_ones() {
        let m1 = build(&inputs("S1", "S0", "P1", "P0", "24", 1), None).unwrap();
        let m2 = build(&inputs("S2", "S1", "P2", "P1", "25", 1), Some(&m1)).unwrap();
        assert!(!m2.rebase);
        assert_eq!(m2.cards.patches.len(), 1);
        assert_eq!(m2.cards.patches[0].patch.url, "https://example.test/o/r/releases/download/data-run25/cards-S1-to-S2.coqp");
        assert_eq!(m2.prices.patches.len(), 1);
        let m3 = build(&inputs("S3", "S2", "P3", "P2", "26", 1), Some(&m2)).unwrap();
        assert_eq!(m3.cards.patches.iter().map(|x| (x.from.as_str(), x.to.as_str())).collect::<Vec<_>>(), [("S1", "S2"), ("S2", "S3")]);
        assert_eq!(run_files(&m3), ["cards-S2-to-S3.coqp", "cards-S2-to-S3.coqid", "prices-P2-to-P3.coqx"]);
        // Links older than keep_since fall out of the window.
        let mut late = inputs("S4", "S3", "P4", "P3", "26", 1);
        late.keep_since = "2026-10-26T00:00:00Z".into();
        let m4 = build(&late, Some(&m3)).unwrap();
        assert_eq!(m4.cards.patches.iter().map(|x| x.from.as_str()).collect::<Vec<_>>(), ["S2", "S3"]);
    }

    #[test]
    fn epoch_change_rebases() {
        let m1 = build(&inputs("S1", "S0", "P1", "P0", "24", 1), None).unwrap();
        let m2 = build(&inputs("S2", "S1", "P2", "P1", "25", 2), Some(&m1)).unwrap();
        assert!(m2.rebase && m2.cards.patches.is_empty());
    }

    #[test]
    fn wrong_or_missing_patch_is_an_error() {
        let m1 = build(&inputs("S1", "S0", "P1", "P0", "24", 1), None).unwrap();
        let bad = inputs("S2", "S9", "P2", "P1", "25", 1);
        assert!(build(&bad, Some(&m1)).unwrap_err().contains("does not lead"));
        let mut none = inputs("S2", "S1", "P2", "P1", "25", 1);
        none.cards.patch = None;
        assert!(build(&none, Some(&m1)).unwrap_err().contains("no patch"));
        none.force_rebase = true;
        assert!(build(&none, Some(&m1)).unwrap().rebase);
    }

    #[test]
    fn prices_only_change_keeps_card_chain() {
        let m1 = build(&inputs("S1", "S0", "P1", "P0", "24", 1), None).unwrap();
        let m2 = build(&inputs("S2", "S1", "P2", "P1", "25", 1), Some(&m1)).unwrap();
        let mut same = inputs("S2", "S1", "P3", "P2", "26", 1);
        same.cards.patch = None;
        let m3 = build(&same, Some(&m2)).unwrap();
        assert_eq!(m3.cards.stamp, "S2");
        assert_eq!(m3.cards.patches, m2.cards.patches);
        assert_eq!(m3.prices.patches.len(), 2);
    }

    #[test]
    fn json_round_trip() {
        let m = build(&inputs("S1", "S0", "P1", "P0", "24", 1), None).unwrap();
        let j = serde_json::to_string_pretty(&m).unwrap();
        assert_eq!(serde_json::from_str::<Manifest>(&j).unwrap(), m);
        assert!(j.contains("\"raw_sha256\"") && j.contains("\"url\""));
    }
}
