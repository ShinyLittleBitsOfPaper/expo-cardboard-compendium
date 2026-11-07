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

## Decision from the real-art run, and P2 as built (0.0.3-6)
- Owner's `--variants` run on 300 real crops: **pHash, inner 80% of the art, photo tried at 3x3 shifts of +-3%** gives 98.9% same-art hits at <=10 bits (baseline 87.2%), crop-jitter mean 6.7 (was 12.9), nearest different art min 14 / p5 16 / p50 18. dHash dropped. Threshold to start with: <=10, ranking-only.
- Stored: one u64 pHash of `window(art, 0.8)` per illustration (`art_phash`). Query side (`query_hashes`): 9 hashes of a 94% window at +-3% shifts, best distance wins. Nothing stored depends on the shifts.
- **Deviation from P0: no `illustration_id` column in the base `mtg` schema.** The overlay compile reads `illustration_id` and the front-face `art_crop` URL straight from the Scryfall bulk file (face 0 for multi-face cards), joined to printing rows by Scryfall id. Same result, no base format change, no recompile of `.coqd`. Cost: the art-hash step needs the bulk file next to the card file (the cron has both).
- Overlay: schema `mtg-arthash`, table `art` (columnar), two nullable Int columns `phash_hi`/`phash_lo`, row i = printing row i, `overlay_of printing`; null = no hash (tokens without art, etc.). Compatible with the existing overlay sync/patch path.
- Hash cache: TSV `illustration_id<TAB>16 hex` (about 60k lines); the overlay is rebuilt from it every run, so `--prev`-style row stability is not needed, and only illustrations without a cached hash are ever downloaded.
- Commands: `coqc art-hash BULK --cards cards.coqd --cache art-hashes.tsv -o art.coqd [--images DIR] [--todo todo.tsv]` (hash images named `<illustration_id>.jpg|png` that are not cached yet, write the overlay, list what is still missing); `scripts/fetch-art.sh todo.tsv DIR [PARALLEL]` downloads them (resumable, UA, 4 parallel, 0.1 s pause); `coqc art-match IMAGE --overlay art.coqd --cards cards.coqd [--top N] [--max D]` is the CLI prototype of the engine's `matchArtHash` (reports candidates and brute-force time).
- First run: `coqc art-hash` (writes todo.tsv, ~60k+ lines) -> `fetch-art.sh` (one-off, hours at 4 parallel; stop and resume freely) -> `coqc art-hash --images` again. Then `art-match` with your scan captures' `crop.jpg` to check real matches.
- NOT done: wiring into `data-build.sh`/the manifest (needs the cache kept as a release asset and a decision on who downloads new art daily, plus Scryfall's permission reply), engine `matchArtHash`/`hashImageFile` (P3), slbop use (P4).

## Art box for whole-card photos (0.0.3-10)
- `coqc art-match IMAGE --card ...` cuts the art box out of a straightened whole-card image first (`card_art_region`: x 8.3-91.7%, y 11.3-55%) so scanner `crop.jpg` files can be used as they are. Checked by eye on 5 real captures (draw-the-box overlay): fits the standard-frame cards (Leonin Lightscribe, Akroan Horse) and the blurry old-border Italian card closely. Captures 2 and 5 of that session were NOT usable: both report `cropSource live-detected, cardCropped true` yet the crop is badly framed (card cut off / small card inside a large frame); that is a scanner crop-quality problem, independent of hashing. Real match results on the other three: pending the owner's run.
