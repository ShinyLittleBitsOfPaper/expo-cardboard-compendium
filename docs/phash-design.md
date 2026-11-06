# Perceptual hashing (art match) — design, step P0

Status 2026-11-06, 0.0.3-3: **P1 built** (`coqc phash-eval`, crate `crates/phash`, `scripts/fetch-art-crops.sh`) on the defaults below, which were taken as accepted when the owner said "continue" (decisions 1-5 are NOT explicitly confirmed; P2+ waits for real P1 numbers). Earlier: 0.0.3-2 plan only. Origin: slbop TODO #96 ("Perceptual-hash card-art matching", on hold because
slbop has no server/pipeline to maintain). The compendium already has a daily pipeline, an overlay mechanism and a native engine,
so the blocker is gone. Goal: given a photo of a card, narrow the candidate printings (the scanner's last unresolved signal, see
slbop `scanMatchers.ts`).

## What a hash can and cannot do
- An **art hash** identifies the *illustration*. Every printing that reuses an illustration (most reprints) hashes the same, so it
  separates "Lightning Bolt, art A" from "art B" but **not** two printings of art A. Frame, set symbol, border, collector number and
  year (existing OCR signals) still decide those. It is a narrowing signal, never a sole identifier.
- Do not hash the whole card for phase 1: frame/border/glare/perspective dominate the distance and the existing signals already cover
  frame variants.

## Proposed defaults (change before building)
1. **What**: 64-bit dHash of the `art_crop` image (9x8 grayscale, horizontal gradient), plus a second 64-bit pHash (32x32 DCT, low 8x8)
   so the on-device threshold can be calibrated on whichever separates real scans better. Stored as 4 u32 columns.
2. **Per illustration, not per printing**: hash once per `illustration_id` (~half the printings share one), copy to every printing row.
   *Needs a schema addition*: `illustration_id` (Uuid) is not in `mtg_schema()` today; cards without one (tokens, some layouts)
   get no hash. Multi-face cards: hash the front face's art; scanning a back face is out of scope.
3. **Where it lives**: a second overlay `art-hash` (columnar Int table, row i = printing i, like prices) in the daily release, patched
   with ordinary `.coqp`. ~120k rows x 16 bytes, compresses little (~1.5 MB full); a day's patch is the new illustrations only.
   No schema change for the base dataset beyond `illustration_id`.
4. **Images** come from Scryfall's CDN (`cards.scryfall.io/art_crop/...`), only for illustrations not hashed in the previous release
   (the cron keeps the previous overlay and `--prev` row stability, as it does for cards). First run ~60k small JPEGs (~3-4 GB
   transfer, a one-off; do it from the user's machine or a throttled workflow, not on every cron). **Needs Scryfall's permission
   answer** (still pending for the data release) and a descriptive User-Agent; hashes only, no images are stored or redistributed.
5. **One implementation of the hash**: the Rust engine exposes `hashImageFile(path, crop?)` (decode with the `image` crate, area
   downscale) and the *compiler uses the same function*. Hashing in JS/expo-image-manipulator on the device and in Rust at compile
   time would differ by resampling filter and drift the distances. Cost: the `image` crate (jpeg/png only, no default features) in
   the native libs; measure the size delta.
6. **Matching**: `matchArtHash(hash, {maxDistance, rows?})` returns rows ordered by Hamming distance; brute force over ~60k
   unique hashes is a few ms (popcount), no index needed. A `scope` (the candidate rows the OCR stages left) makes it near-free.
7. **Threshold**: not guessed. Calibrate from slbop's scan-debug captures (`scanner-log.txt` + the session export) against known
   cards; the doc below says how.

## Steps
- **P0 (this doc)** decisions below.
- **P1 evaluation tool, no engine change**: `coqc phash-eval --images DIR`: hash a list of art crops (dHash + pHash), then perturb each
  (blur, +-20% brightness/contrast, +-2 deg rotation, 3% crop jitter, JPEG q40, a mild perspective warp) and report, per algorithm:
  same-art hit rate at distances 0..16 and the nearest *different*-illustration distance. Needs ~300 art crops from the user
  (a mix of near-identical arts, e.g. 20 Lightning Bolts, basic lands) because the sandbox cannot reach Scryfall.
- **P2 schema + compile**: `illustration_id` column; `coqc art-hash` (incremental over `--prev`), overlay `art-hash` in `data-build.sh`,
  manifest dataset (role overlay, `of` cards), TS sync already handles any overlay.
- **P3 engine/module**: `hashImageFile`, `matchArtHash`, attach the overlay, node/Rust tests, golden hashes for 5 fixture images.
- **P4 slbop**: wire the slot in `scanMatchers.ts`; calibrate thresholds on real captures; ranking-only first (like the promo-stamp signal),
  never hides a candidate.

## Decisions needed
1. Art crop only (default) or also a full-card hash?
2. Per-illustration hashing with an `illustration_id` schema column (default)?
3. dHash + pHash both stored (default) or dHash only?
4. First bulk image pass: your machine (default, one-off script) or the GitHub workflow (slower, rate-limited, needs the permission reply)?
5. OK to add the `image` crate to the native module (size measured in P3)?
6. Can you collect ~300 art crops for P1, or should P1 be a script that downloads them for you to run?

## P1 as built (0.0.3-3)
- `crates/phash` (`coqc-phash`): `load_gray` (image crate, jpeg+png only), hand-written `area_resize` (deterministic f32, same code the engine will reuse), `dhash` (9x8), `phash` (32x32 DCT, low 8x8, median split, DC dropped), `hamming`, ten perturbations (blur, brightness +-20%, contrast +-20%, rotation +-2 deg, 3% crop jitter, JPEG q40, mild perspective), `evaluate`.
- `coqc phash-eval --images DIR [--limit N]` prints per algorithm: same-art hit rate at distance 0..16, mean distance per perturbation, and the nearest DIFFERENT image distance (min/p5/p50, plus images with a neighbour within 8 bits). Pick the threshold below the nearest-different minimum and above the perturbation tail.
- `scripts/fetch-art-crops.sh OUT_DIR [N=300]`: Scryfall search -> art_crop JPEGs (near-identical arts: Lightning Bolt, basics, Counterspell, Llanowar Elves + random rarities), polite UA and 100 ms spacing. Needs curl + jq; the sandbox cannot reach Scryfall, so the user runs it.
- Smoke run on 30 synthetic value-noise images (NOT evidence about real art): both hashes ~0 bits under brightness/contrast/blur/JPEG, 3-4 under +-2 deg rotation, 1-2 under perspective, **11-12 under a 3% crop jitter** (the weak spot: alignment error in the on-device art crop will matter more than tone), nearest different image 18+. Smooth synthetic images (sinusoids) made dHash collapse across seeds and pHash flip on noise, so use real art.
- Not built: the 5 golden-hash fixture images (P3), `illustration_id` column and `coqc art-hash` (P2).

## P1 real-art result and P1b (0.0.3-4)
- Owner's run, 300 real art crops (0.0.3-3 tool, 10 perturbations): **pHash beats dHash**: nearest different image min 14 / p5 16 / p50 20 bits (dHash min 6, three images with a different-art neighbour within 8 bits). Blur, brightness, contrast, JPEG all under ~3 bits mean, rotation +-2 deg ~4, perspective ~2; **3% crop jitter ~13 bits for both** and accounts for nearly all same-art misses. pHash distances are always even (median split sets exactly 32 bits). Working threshold ~10-12 bits; recommendation: store pHash only (drop dHash) unless real captures disagree.
- P1b: `coqc phash-eval --images DIR --variants` compares matching variants, each hashing the reference once and the photo at each offset (best distance wins): full (baseline), inner 80%, 3x3 shifts of a 94% window (+-3%), inner 80% + 3x3 shifts. Added perturbation `CropJitterNeg` (mirror of the crop jitter), so the baseline now averages 11 perturbations. Synthetic smoke only: inner 80% + 3x3 shifts looked best (crop-jitter mean ~4-6 bits, nearest different >= 12-16); real numbers pending the owner's run. Shifts only change the photo side, so the stored hash is unaffected.
