//! `manifest.json` (v2): the one file clients fetch (fixed URL on the rolling `data-latest` release) to learn what is
//! current and how to get there. It is a list of **datasets**, each with a role (`base`, `index`, `overlay`), the
//! current full file, the format versions needed to read it, and its own chain of patches from older releases
//! (independent chains, since e.g. prices change without cards).
//!
//! - `base`: a `.coqd` that defines the row-id space (`epoch`). A new epoch means no patch chain: clients download full files.
//! - `index`: a derived `.coqi` of the dataset named by `of`, shipped zstd-compressed; `raw_*` describe the decompressed
//!   file (what an index delta's base/target hashes refer to). Its stamp equals its base's stamp. If the base moved but
//!   no index delta was given, the index chain starts over (clients fetch the full index).
//! - `overlay`: a `.coqd` whose row i belongs to row i of the dataset named by `of` (same epoch); chains independently.
//!
//! Pure logic: `build` takes already-measured assets and the previous manifest, so it is unit-testable without files.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MANIFEST_VERSION: u32 = 2;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Base,
    Index,
    Overlay,
}

/// One link of a dataset's patch chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub from: String,
    pub to: String,
    /// When this patch was published (ISO 8601 UTC); used to expire old links of the chain.
    pub at: String,
    pub patch: Asset,
}

/// Compression of the shipped file; `raw_*` describe the decompressed bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Raw {
    pub encoding: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dataset {
    pub id: String,
    pub role: Role,
    /// Dataset this one is derived from / attached to (`index`, `overlay`); absent for a `base`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<String>,
    pub stamp: String,
    /// Format version of the file as written.
    pub format: u16,
    /// Smallest module reader version able to read it; above the module's own => "update the app".
    pub min_client_format: u16,
    pub file: Asset,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Raw>,
    pub patches: Vec<Link>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub manifest_version: u32,
    pub generated_at: String,
    /// Tag of the immutable release that holds this run's patches.
    pub run: String,
    /// Row-id space of the base dataset(s). A client on another epoch cannot patch: it downloads the full files.
    pub epoch: u32,
    /// True when no patch chain leads here from earlier releases (first release or a new epoch / format).
    pub rebase: bool,
    pub datasets: Vec<Dataset>,
}

impl Manifest {
    pub fn dataset(&self, id: &str) -> Option<&Dataset> {
        self.datasets.iter().find(|d| d.id == id)
    }
}

/// What `build` needs to know about one measured file.
#[derive(Clone)]
pub struct Measured {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

/// One dataset of the new release, already measured.
pub struct NewDataset {
    pub id: String,
    pub role: Role,
    pub of: Option<String>,
    /// File-name prefix of the full file and its patches (`cards-` for `cards-S.coqd` / `cards-A-to-B.coqp`).
    pub prefix: String,
    pub stamp: String,
    pub format: u16,
    pub min_reader: u16,
    pub file: Measured,
    /// `(encoding, raw size, raw sha256)` when the shipped file is compressed.
    pub raw: Option<(String, u64, String)>,
    /// Patch from the previous release of this dataset.
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
    pub epoch: u32,
    pub datasets: Vec<NewDataset>,
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

fn validate(inp: &Inputs) -> Result<(), String> {
    if inp.datasets.is_empty() {
        return Err("no datasets".into());
    }
    for (i, d) in inp.datasets.iter().enumerate() {
        if d.id.is_empty() || inp.datasets[..i].iter().any(|o| o.id == d.id) {
            return Err(format!("dataset id {:?} is empty or repeated", d.id));
        }
        match (d.role, &d.of) {
            (Role::Base, None) => {}
            (Role::Base, Some(_)) => return Err(format!("{}: a base has no `of`", d.id)),
            (_, None) => return Err(format!("{}: {:?} needs `of`", d.id, d.role)),
            (_, Some(of)) => {
                let t = inp.datasets.iter().find(|o| &o.id == of).ok_or_else(|| format!("{}: unknown `of` {of}", d.id))?;
                if t.role != Role::Base {
                    return Err(format!("{}: `of` {of} is not a base", d.id));
                }
            }
        }
        if (d.role == Role::Index) != d.raw.is_some() {
            return Err(format!("{}: only an index is shipped compressed (raw hashes required for it)", d.id));
        }
    }
    if !inp.datasets.iter().any(|d| d.role == Role::Base) {
        return Err("no base dataset".into());
    }
    Ok(())
}

/// Chain of `new` after `pd` (its previous entry): old links inside the window, plus the new link when the stamp moved.
fn extend(inp: &Inputs, new: &NewDataset, pd: &Dataset, lenient: bool) -> Result<Vec<Link>, String> {
    let base = inp.base_url.trim_end_matches('/');
    let mut links: Vec<Link> = pd.patches.iter().filter(|x| x.at >= inp.keep_since).cloned().collect();
    if pd.stamp != new.stamp {
        let Some(pf) = new.patch.as_ref() else {
            if lenient {
                return Ok(Vec::new()); // index: clients fetch the full file
            }
            return Err(format!("{} moved {} -> {} but no patch was given (use --rebase to start a new chain)", new.id, pd.stamp, new.stamp));
        };
        let (from, to) = patch_stamps(&pf.name, &new.prefix).ok_or_else(|| format!("bad patch name {}", pf.name))?;
        if from != pd.stamp || to != new.stamp {
            return Err(format!("patch {} does not lead {} -> {}", pf.name, pd.stamp, new.stamp));
        }
        links.push(Link { from, to, at: inp.generated_at.clone(), patch: asset(base, &inp.run_tag, pf) });
    }
    Ok(links)
}

pub fn build(inp: &Inputs, prev: Option<&Manifest>) -> Result<Manifest, String> {
    validate(inp)?;
    let base = inp.base_url.trim_end_matches('/');
    // A usable previous manifest has the same epoch; otherwise clients must start over.
    let prev_ok = prev.filter(|m| m.manifest_version == MANIFEST_VERSION && m.epoch == inp.epoch);
    let mut rebase = inp.force_rebase || prev_ok.is_none();
    let mut out: Vec<Dataset> = Vec::new();
    for d in &inp.datasets {
        // A chain continues only from the same dataset with the same role, `of` and format.
        let pd = prev_ok
            .and_then(|m| m.dataset(&d.id))
            .filter(|p| p.role == d.role && p.of == d.of && p.format == d.format && !inp.force_rebase);
        let patches = match pd {
            Some(pd) => extend(inp, d, pd, d.role == Role::Index)?,
            None => {
                if d.role == Role::Base {
                    rebase = true; // a base without history leaves clients nothing to patch from
                }
                Vec::new()
            }
        };
        out.push(Dataset {
            id: d.id.clone(),
            role: d.role,
            of: d.of.clone(),
            stamp: d.stamp.clone(),
            format: d.format,
            min_client_format: d.min_reader,
            file: asset(base, LATEST_TAG, &d.file),
            raw: d.raw.as_ref().map(|(e, s, h)| Raw { encoding: e.clone(), size: *s, sha256: h.clone() }),
            patches,
        });
    }
    Ok(Manifest {
        manifest_version: MANIFEST_VERSION,
        generated_at: inp.generated_at.clone(),
        run: inp.run_tag.clone(),
        epoch: inp.epoch,
        rebase,
        datasets: out,
    })
}

/// Names of the full files a manifest points at in `data-latest` (everything else there may be deleted).
pub fn current_files(m: &Manifest) -> Vec<String> {
    m.datasets.iter().map(|d| d.file.name.clone()).collect()
}

/// Names of every asset that belongs in this run's immutable `data-<run>` release.
pub fn run_files(m: &Manifest) -> Vec<String> {
    let tag = format!("/{}/", m.run);
    m.datasets.iter().flat_map(|d| &d.patches).filter(|x| x.patch.url.contains(&tag)).map(|x| x.patch.name.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me(name: &str) -> Measured {
        Measured { name: name.into(), size: 10, sha256: sha_hex(name.as_bytes()) }
    }
    fn ds(id: &str, role: Role, of: Option<&str>, prefix: &str, stamp: &str, ext: &str, patch_ext: &str, prev: &str) -> NewDataset {
        NewDataset {
            id: id.into(),
            role,
            of: of.map(Into::into),
            prefix: prefix.into(),
            stamp: stamp.into(),
            format: 3,
            min_reader: 3,
            file: me(&format!("{prefix}{stamp}.{ext}")),
            raw: (role == Role::Index).then(|| ("zstd".into(), 100, "ab".into())),
            patch: (!prev.is_empty()).then(|| me(&format!("{prefix}{prev}-to-{stamp}.{patch_ext}"))),
        }
    }
    fn inputs(stamp: &str, prev_stamp: &str, pstamp: &str, pprev: &str, at: &str, epoch: u32) -> Inputs {
        Inputs {
            base_url: "https://example.test/o/r/releases/download/".into(),
            run_tag: format!("data-run{at}"),
            generated_at: format!("2026-10-{at}T22:00:00Z"),
            keep_since: "2026-10-05T00:00:00Z".into(),
            force_rebase: false,
            epoch,
            datasets: vec![
                ds("cards", Role::Base, None, "cards-", stamp, "coqd", "coqp", prev_stamp),
                ds("cards-index", Role::Index, Some("cards"), "cards-", stamp, "coqi.zst", "coqid", prev_stamp),
                ds("prices", Role::Overlay, Some("cards"), "prices-", pstamp, "coqd", "coqp", pprev),
            ],
        }
    }
    fn chain<'a>(m: &'a Manifest, id: &str) -> Vec<&'a str> {
        m.dataset(id).unwrap().patches.iter().map(|x| x.from.as_str()).collect()
    }

    #[test]
    fn stamps_parse() {
        assert_eq!(patch_stamps("cards-A1-to-B2.coqp", "cards-"), Some(("A1".into(), "B2".into())));
        assert_eq!(patch_stamps("prices-20261024-to-20261027.coqp", "prices-"), Some(("20261024".into(), "20261027".into())));
        assert_eq!(patch_stamps("cards-A1.coqp", "cards-"), None);
        assert_eq!(patch_stamps("prices-A-to-B.coqp", "cards-"), None);
    }

    #[test]
    fn first_release_is_a_rebase_with_empty_chains() {
        let m = build(&inputs("S1", "S0", "20261024", "20261023", "24", 1), None).unwrap();
        assert!(m.rebase && m.datasets.iter().all(|d| d.patches.is_empty()));
        let c = m.dataset("cards").unwrap();
        assert_eq!(c.file.url, "https://example.test/o/r/releases/download/data-latest/cards-S1.coqd");
        assert_eq!((c.role, c.of.as_deref(), c.min_client_format), (Role::Base, None, 3));
        let i = m.dataset("cards-index").unwrap();
        assert_eq!((i.raw.as_ref().unwrap().encoding.as_str(), i.of.as_deref()), ("zstd", Some("cards")));
        assert_eq!(current_files(&m).len(), 3);
        assert!(run_files(&m).is_empty());
    }

    #[test]
    fn chain_grows_links_and_expires_old_ones() {
        let m1 = build(&inputs("S1", "S0", "P1", "P0", "24", 1), None).unwrap();
        let m2 = build(&inputs("S2", "S1", "P2", "P1", "25", 1), Some(&m1)).unwrap();
        assert!(!m2.rebase);
        let l = &m2.dataset("cards").unwrap().patches[0];
        assert_eq!(l.patch.url, "https://example.test/o/r/releases/download/data-run25/cards-S1-to-S2.coqp");
        assert_eq!(chain(&m2, "prices").len(), 1);
        let m3 = build(&inputs("S3", "S2", "P3", "P2", "26", 1), Some(&m2)).unwrap();
        assert_eq!(chain(&m3, "cards"), ["S1", "S2"]);
        assert_eq!(run_files(&m3), ["cards-S2-to-S3.coqp", "cards-S2-to-S3.coqid", "prices-P2-to-P3.coqp"]);
        // Links older than keep_since fall out of the window.
        let mut late = inputs("S4", "S3", "P4", "P3", "26", 1);
        late.keep_since = "2026-10-26T00:00:00Z".into();
        let m4 = build(&late, Some(&m3)).unwrap();
        assert_eq!(chain(&m4, "cards"), ["S2", "S3"]);
    }

    #[test]
    fn epoch_change_rebases_everything() {
        let m1 = build(&inputs("S1", "S0", "P1", "P0", "24", 1), None).unwrap();
        let m2 = build(&inputs("S2", "S1", "P2", "P1", "25", 2), Some(&m1)).unwrap();
        assert!(m2.rebase && m2.datasets.iter().all(|d| d.patches.is_empty()));
    }

    #[test]
    fn format_change_restarts_only_that_chain() {
        let m1 = build(&inputs("S1", "S0", "P1", "P0", "24", 1), None).unwrap();
        let mut i = inputs("S2", "S1", "P2", "P1", "25", 1);
        i.datasets[2].format = 4; // overlay format bump
        let m2 = build(&i, Some(&m1)).unwrap();
        assert!(!m2.rebase && chain(&m2, "cards").len() == 1 && m2.dataset("prices").unwrap().patches.is_empty());
        i.datasets[0].format = 4; // base format bump: no history to patch from
        assert!(build(&i, Some(&m1)).unwrap().rebase);
    }

    #[test]
    fn wrong_or_missing_patch_is_an_error() {
        let m1 = build(&inputs("S1", "S0", "P1", "P0", "24", 1), None).unwrap();
        let bad = inputs("S2", "S9", "P2", "P1", "25", 1);
        assert!(build(&bad, Some(&m1)).unwrap_err().contains("does not lead"));
        let mut none = inputs("S2", "S1", "P2", "P1", "25", 1);
        none.datasets[0].patch = None;
        assert!(build(&none, Some(&m1)).unwrap_err().contains("no patch"));
        none.force_rebase = true;
        assert!(build(&none, Some(&m1)).unwrap().rebase);
    }

    #[test]
    fn missing_index_delta_restarts_the_index_chain_only() {
        let m1 = build(&inputs("S1", "S0", "P1", "P0", "24", 1), None).unwrap();
        let m2 = build(&inputs("S2", "S1", "P2", "P1", "25", 1), Some(&m1)).unwrap();
        let mut i = inputs("S3", "S2", "P3", "P2", "26", 1);
        i.datasets[1].patch = None;
        let m3 = build(&i, Some(&m2)).unwrap();
        assert!(m3.dataset("cards-index").unwrap().patches.is_empty());
        assert_eq!(chain(&m3, "cards"), ["S1", "S2"]);
    }

    #[test]
    fn overlay_only_change_keeps_other_chains() {
        let m1 = build(&inputs("S1", "S0", "P1", "P0", "24", 1), None).unwrap();
        let m2 = build(&inputs("S2", "S1", "P2", "P1", "25", 1), Some(&m1)).unwrap();
        let mut same = inputs("S2", "S1", "P3", "P2", "26", 1);
        same.datasets[0].patch = None;
        same.datasets[1].patch = None;
        let m3 = build(&same, Some(&m2)).unwrap();
        assert_eq!(m3.dataset("cards").unwrap().stamp, "S2");
        assert_eq!(m3.dataset("cards").unwrap().patches, m2.dataset("cards").unwrap().patches);
        assert_eq!(m3.dataset("cards-index").unwrap().patches, m2.dataset("cards-index").unwrap().patches);
        assert_eq!(m3.dataset("prices").unwrap().patches.len(), 2);
    }

    #[test]
    fn a_second_overlay_needs_no_code() {
        let mut i = inputs("S1", "S0", "P1", "P0", "24", 1);
        i.datasets.push(ds("rulings", Role::Overlay, Some("cards"), "rulings-", "R1", "coqd", "coqp", "R0"));
        let m1 = build(&i, None).unwrap();
        assert_eq!(m1.datasets.len(), 4);
        let mut j = inputs("S2", "S1", "P2", "P1", "25", 1);
        j.datasets.push(ds("rulings", Role::Overlay, Some("cards"), "rulings-", "R2", "coqd", "coqp", "R1"));
        assert_eq!(chain(&build(&j, Some(&m1)).unwrap(), "rulings"), ["R1"]);
    }

    #[test]
    fn invalid_inputs_are_refused() {
        let mut i = inputs("S1", "S0", "P1", "P0", "24", 1);
        i.datasets[2].of = Some("nope".into());
        assert!(build(&i, None).unwrap_err().contains("unknown `of`"));
        let mut i = inputs("S1", "S0", "P1", "P0", "24", 1);
        i.datasets[2].id = "cards".into();
        assert!(build(&i, None).unwrap_err().contains("repeated"));
        let mut i = inputs("S1", "S0", "P1", "P0", "24", 1);
        i.datasets[1].raw = None;
        assert!(build(&i, None).is_err());
        let mut i = inputs("S1", "S0", "P1", "P0", "24", 1);
        i.datasets[2].of = Some("cards-index".into());
        assert!(build(&i, None).unwrap_err().contains("not a base"));
    }

    #[test]
    fn v1_manifest_is_ignored_as_previous() {
        let m1 = build(&inputs("S1", "S0", "P1", "P0", "24", 1), None).unwrap();
        let mut old = m1.clone();
        old.manifest_version = 1;
        assert!(build(&inputs("S2", "S1", "P2", "P1", "25", 1), Some(&old)).unwrap().rebase);
    }

    #[test]
    fn json_round_trip() {
        let m = build(&inputs("S1", "S0", "P1", "P0", "24", 1), None).unwrap();
        let j = serde_json::to_string_pretty(&m).unwrap();
        assert_eq!(serde_json::from_str::<Manifest>(&j).unwrap(), m);
        assert!(j.contains("\"raw\"") && j.contains("\"min_client_format\"") && !j.contains("\"of\": null"));
    }
}
