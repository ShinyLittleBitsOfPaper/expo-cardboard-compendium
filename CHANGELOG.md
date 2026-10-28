# Changelog

## Unreleased

### Added
- 2026-10-28, 0.0.2-9: `docs/schema-design.md` updated with the approved decisions (schema in file + compiled dialects, schema-driven query language, no data release before S4, extensions reduced to `.coqd`/`.coqi`/`.coqp`; `.coqr`/`.coqx`/`.coqsd` to be retired). `docs/roadmap.md` still uses the old names until S2. Docs only, no code.
- 2026-10-27, 0.0.2-8: `docs/schema-design.md`: draft design for making the format, engine and sync schema-driven (tables, self-describing schema, MTG as a dialect crate, overlays, manifest v2), staged S0-S5 with acceptance criteria and four open decisions. Docs only, no code.
- 2026-10-27, 0.0.2-7: Milestone 2 (client sync) logic. Module `src/sync/` (manifest validation, pure planner, executor with injected deps, `sync()` single-flight; exports `sync`, `dataPaths`, `DEFAULT_MANIFEST_URL`), engine helpers `sha256File` / `unzstFile` / `moveFile` / `removeFile` (Rust test), 12 `node:test` tests (`npm test` in the module; added typescript + @types/node dev deps, `tsconfig.test.json`), CI step in `native.yml`. Not yet run on a device; expo-file-system calls are unverified.
- 2026-10-27, 0.0.2-5: Milestone 1 (data publishing) built. `coqc manifest` (manifest.json: epoch, rebase flag, min reader formats, current full files with sha256/size, independent card and price patch chains that expire after `--keep-since`, URLs into `data-latest` / `data-<run>`; refuses an index built from another dataset or a patch that does not lead from the previous stamp), `coqc zst` / `coqc unzst` (index ships as `.coqs.zst`), `scripts/data-build.sh`, `scripts/data-publish.sh` (ordering so a manifest never points at a missing file), `scripts/mock-gh.sh`, `scripts/test-data-pipeline.sh` (offline end to end, incl. simulated client and chain expiry/pruning), `.github/workflows/data.yml` (cron 22:00 UTC + dispatch with `publish`/`rebase` inputs). 7 new unit tests. Not yet run on GitHub. On the real 2026-10-27 files: index 21.88 MiB raw -> 4.74 MiB zstd.
- 2026-10-27, 0.0.2-4: roadmap: recorded real measurements (patch/delta sizes, per-section index sizes, emulator open/search times) and the decision to swap slbop to the engine fully, no feature flag. Docs only.
- 2026-10-27, 0.0.2-3: roadmap updated with the hosting decision: single repo, rolling `data-latest` release (manifest + current full files) plus immutable `data-<stamp>` prerelease patch releases, data never marked Latest, `GITHUB_TOKEN` only. Docs only.
- 2026-10-27, 0.0.2-2: `docs/roadmap.md`: plan for finishing the compendium and the daily data pipeline before the slbop swap (cron/publishing design with a separate data repo, once-daily cadence, release layout and manifest, client sync in the module, engine gaps, measurements). Docs only; proposed defaults for three decisions.
- 2026-10-26, 0.0.2-1: versions now follow the release tag. `scripts/set-version.sh X.Y.Z` sets Cargo.toml (+ Cargo.lock), the module's package.json, build.gradle (version + versionName), the podspec and the fetch scripts' User-Agent; `scripts/check-version.sh vX.Y.Z` verifies them and CI's `test` job runs it on tag pushes, so a tag that disagrees with the files fails before anything is built. Everything set to 0.0.2. Release procedure: `scripts/set-version.sh X.Y.Z`, commit, push, tag `vX.Y.Z`, push the tag.
- 2026-10-26, 0.0.2-1: `scripts/daily.sh` now also writes the search index per release (`cards-<stamp>.coqs`) and the index delta from the previous release (`patches/<prev>-to-<new>.coqsd`, replay-verified). The engine itself builds and saves a `.coqs` next to the `.coqd` on first open when none is present, so importing one is optional.
- 2026-10-26, 0.0.1-30: the npm package now ships the MIT `LICENSE` (copy of the root file in `modules/expo-cardboard-compendium/`) and declares `license` and `repository` in its `package.json`. Keep the two LICENSE files identical.
- 2026-10-25, 0.0.1-26: renamed the Expo module and repo to `expo-cardboard-compendium` (module dir, native module `ExpoCardboardCompendium`, Android package `expo.modules.cardboardcompendium` and JNI symbol, podspec, workflow paths, scripts, LICENSE/NOTICE, bundle names). Rust crates, `coqc`, `libcoqe` and the `.coqd`/`.coqs` extensions are unchanged on purpose: the coq family is meant to stay dataset-agnostic (README Naming; generalisation not designed yet). README intro brought up to date. Fetch scripts' default User-Agent now says expo-cardboard-compendium.
- 2026-10-25, 0.0.1-25: CI/release plan. `.github/workflows/native.yml` (test on push; on tag: Android + iOS native builds, `npm pack`, attach tarball to the GitHub Release), module `package.json` `files`, gitignore for built libraries, docs/engine.md release section. UNTESTED (no runner here).
- 2026-10-25, 0.0.1-24: engine + Expo module. New crate `coqc-engine` (lib `coqe`): `Engine` (open with `.coqs` reuse/rebuild, search with optional host-registered scope, getCards, getByScryfallId, registerSet, applyPatch, applyIndexDelta, info) and a one-function JSON C ABI (`coqe_call`/`coqe_free`). `Query.scope` added to the search. `patch` moved from `coqc` to `coqc_format` (re-exported by `coqc`) so the engine does not depend on the compiler. `modules/expo-scryfall-engine` scaffold (TS API, Kotlin/JNI, Swift, podspec) and `scripts/build-native.sh`: UNBUILT. docs/engine.md. Tests: 35.
- 2026-10-25, 0.0.1-23: `.coqs` compile-time search index. `coqc_format::search::coqs`: `serialize`/`deserialize` (13 raw, 8-byte-aligned sections: arenas, columns, trigram tables; header carries the SHA-256 of the source .coqd), `make_delta`/`apply_delta` (row-level changes + per-gram append/replace/new/removed postings + stop-gram sets; SHA-256 replay-verified). CLI `coqc index-build` (per-section raw/zstd sizes, reload check, 8 sample queries), `coqc index-diff old new [-o]` (accepts .coqd or .coqs; replays the delta and checks it), `coqc index-apply`. Synthetic 100k printings: 15.2 MB raw, ~1.8 MB whole-file zstd, build 0.5 s, reload 10 ms; a 25-card day delta 0.31 MB. Not yet run on real data. Tests: 33.

### Fixed
- 2026-10-27, 0.0.2-6: `scripts/test-data-pipeline.sh` failed in the `data` workflow on a `publish=false` dry run: the job-level `PUBLISH=false` leaked into the test's own publish step, so `data-latest` was never created. The test now sets `PUBLISH=true` for that step and unsets `REBASE`/`GH_TOKEN`.
- 2026-10-26, 0.0.2-1: Android `UnsatisfiedLinkError: library "/home/.../jniLibs/x86/libcoqe.so" not found` on first run in slbop. The Rust cdylib has no SONAME, so CMake wrote the build machine's absolute path to libcoqe.so into libcoqe_jni.so's DT_NEEDED; `IMPORTED_NO_SONAME TRUE` makes it record just `libcoqe.so`. Verified only by reasoning about the linker, not rebuilt on a device.
- 2026-10-26, 0.0.1-32: Android build fixes from the first slbop build: `android/build.gradle` now sets `defaultConfig.versionCode/versionName` (expo-module-gradle-plugin failed with "'android.defaultConfig.versionName' is not defined"); `build-native.sh` also builds the 32-bit `x86` ABI (slbop's emulator build is `-PreactNativeArchitectures=x86`, which had no libcoqe.so); CMake fails with a clear message when an ABI's prebuilt library is missing; `expo-modules-core` declared as a peerDependency. Needs a new tag (v0.0.2) for CI to publish a tarball with the x86 library.
- 2026-10-26, 0.0.1-31: `native.yml` package step: `npm version --allow-same-version` (a tag equal to the module's package.json version, e.g. v0.0.1, made `npm version` fail with "Version not changed" and fail the package job), and an explicit `if` instead of `[ ] && cmd`.
- 2026-10-26, 0.0.1-29: `native.yml`: `ios` job is `continue-on-error`, `package` runs when android succeeded (`!cancelled() && needs.android.result == 'success'`), xcframework download is optional, so a macOS-runner capacity failure no longer blocks the tarball. docs/engine.md records the first CI run and the jniLibs check.
- 2026-10-26, 0.0.1-28: `native.yml` runs the test job on pushes to `trunk` (the repo's default branch; it said `main`). Repo: github.com/ShinyLittleBitsOfPaper/expo-cardboard-compendium.
- 2026-10-25, 0.0.1-27: `scripts/ingest-bundle.sh` no longer breaks when a bundle replaces the script itself (bash reads scripts incrementally, so a changed length made it resume mid-file: "line 43: syntax error near unexpected token `(`" on the -26 ingest). The whole script now runs inside one `{ }` group. Verified by ingesting a bundle with a different-length copy of itself.
- 2026-10-25, 0.0.1-22: name sort key ignores everything but letters/digits, as on Scryfall (verified on results page 6: "Jwari…" before "Jwar Isle…", "Lake-town" after "Lakeside…", `"Lifetime" Pass Holder` between Library and Lindblum). Tests: 32.

### Fixed
- 2026-10-25, 0.0.1-21: default hiding corrected from real Scryfall results: funny sets (Un-sets) are visible; hidden are vanguard/planar/scheme/phenomenon/art_series layouts, set_type memorabilia, and promo_type `playtest` cards. Expected `o:"~ enters tapped" --cards` = 762 (757 + 5 Un-set cards). Real-data check pending. Tests: 32.

### Added
- 2026-10-25, 0.0.1-20: default hiding of extras like Scryfall: printings with layout vanguard/planar/scheme/phenomenon/art_series or set_type memorabilia/funny are hidden unless `include:extras` or the query names a type (`t:`) or set (`s:`/`e:`). New `Query.include_extras`; set_type is read from the printing's residual JSON at index build. `coqc search` now uses the full parser (default unique cards, name order; `--cards` still forces unique cards). Real data after -19 patch: 1.83 MB one-time for +11,954 printings/+3,243 oracle rows (file 29.70 MB). `o:"~ enters tapped"` was 768 before hiding vs Scryfall 762. Tests: 32.

### Changed
- 2026-10-25, 0.0.1-19: compile now includes digital-only and art_series printings by default (Scryfall search includes them; `o:"~ enters tapped"` was 5 short because Alchemy cards like Potential Horizon were skipped). New flags `--skip-digital`, `--skip-art-series` (Options `skip_digital`/`skip_art_series`); `verify` tolerates datasets compiled with skips. Expect ~11.9k appended rows (9,168 digital + 2,775 art_series) in the next patch from a previous release (one-time, size unmeasured). Default hiding of extras at search time still TODO. Tests: 31.

### Fixed
- 2026-10-25, 0.0.1-18: `~` in `o:`/`fo:` also matches generic referents ("this land", "this creature", ...), per Scryfall's `o:"~ enters tapped"` results (762 cards). Real-data check pending: `coqc search 'o:"~ enters tapped"'` unique cards vs 762.

### Added
- 2026-10-24, 0.0.1-17: Scryfall defaults and text semantics. Parser default is `unique:cards`, `prefer:newest` (new `Query.prefer`, picks the printing before sorting), alphabetical order; `prefer:oldest`. Names ignore `'`, `’`, `,` (separate normalised name arena). `o:` now excludes reminder text, new `fo:`/`fulloracle` leaf `fulloracle`; text trigrams cover both texts. `~` in oracle queries = the card's own name (full/face/pre-comma). Tests: 30.

### Added
- 2026-10-24, 0.0.1-16: Scryfall-syntax parser in Rust (`coqc_format::parse::parse(&str) -> Query`): and/or/parens/negation, quoted and `!exact`, named colors, `c:m`, numeric color counts, `mv:even|odd`, `cn` ranges, `order:`/`direction:`/`unique:`; unsupported keywords error. New plan leaves `mvparity`, `cnnum`. `quick.rs` now wraps it. Tests: 29 (3 new parser tests). Not run on real data.

### Added
- 2026-10-24, 0.0.1-15: search follows Scryfall conventions (slbop-isms dropped). Reverted name punctuation stripping and `typeeq`/`oracleeq` (not Scryfall); rarity order is now common<uncommon<rare<special<mythic<bonus; added `nameexact` (`!"name"`). Kept identity `:` = subset, `colorcount`/`identitycount`. Docs: standalone-module scope + parser TODO. Tests: 26. Supersedes -14's slbop-parity notes.
- 2026-10-24, 0.0.1-14: (superseded by -15) added slbop-parity leaves.

### Added
- 2026-10-24, 0.0.1-13: price dataset. COQR table (TCGplayer USD + Cardmarket EUR retail x normal/foil/etched, keyed by printing row, cents+1), COQX zigzag deltas with SHA-256 replay, COQM uuid map from AllIdentifiers. CLI `idmap`, `prices`, `prices-patch`, `prices-apply`, `price`; `scripts/daily-prices.sh`; docs/prices.md. Tests: 25. Not yet run on real MTGJSON data.

### Added
- 2026-10-24, 0.0.1-12: search. `coqc_format::search`: derived (not stored) index + query plan (`Expr` and/or/not; name, oracle, type, color, identity, mv, rarity, set, cn), folded text, trigram postings with stop-gram size gate and verify, scan fallback, bitset evaluation, sort/unique/paging. CLI `coqc search`, `coqc index-stats`; quick test parser (`coqc::quick`). docs/search.md. Tests: 23 (4 new, incl. gated-index equivalence). Synthetic 107k printings: index build 0.3 s; real-data numbers pending.

### Fixed
- 2026-10-24, 0.0.1-11: `daily.sh` labelled releases by date only, so the 21:05 bulk collided with the 09:05 one ("already exists"). Labels are now bulk `updated_at` as YYYYMMDDHHMM (source_date stays YYYYMMDD); `fetch-bulk.sh` records `<out>.updated_at` and skips download only when it is unchanged (was: file <24h old). Old `cards-20261024.coqd` stays valid as the prev release.

### Fixed
- 2026-10-24, 0.0.1-10: `scripts/daily.sh` printed only the last 4 lines of each compile, hiding the derived-URL and healed counts; it now prints a summary of counts, derived keys, healed printings and section sizes. First real v2 run: 27.3 MB file (was 33.6), compile 27 s + write 10 s.

### Added
- 2026-10-24, 0.0.1-9: format v2 + patches, from the real day-over-day diff (every oracle block changed
  because `edhrec_rank`/`penny_rank` change on ~80%/45% of cards daily; only 317 printing rows changed).
  Ranks moved to their own section (oracle blocks now stay identical between releases). `uri`,
  `rulings_uri`, `prints_search_uri`, `scryfall_uri` are derived and stored only when different.
  New `coqc patch` / `coqc apply` (record-level, SHA-256 verified; patch replays to a byte-identical
  file) and `scripts/daily.sh`. `diff --keys` also reports the ranks table. Fixtures now carry the
  derived URL keys. v1 files must be recompiled. Tests: 19.

### Added
- 2026-10-23, 0.0.1-8: `coqc diff <old> <new> --keys` reports, per key, how many existing rows changed between releases (oracle and printing separately). `--breakdown` variation stats now only count oracle cards with 2+ printings (single-printing cards were trivially "identical"). Tests: 16. Real data so far: `--row-order set` cut the file from 46.6 to 33.6 MB; day1 -> day2 (31 new printings, 1 removed) still changed 100% of oracle blocks and 22% of printing blocks (10.7 MB block patch).

### Added
- 2026-10-23, 0.0.1-7: `--row-order set|uuid` (default `set`): NEW rows are assigned by (set, collector number) instead of uuid so each block holds similar data; existing rows never move, so this only matters at first compile or after `--rebase`. `--breakdown` now also reports, for the 12 costliest residual keys, how many oracle cards have identical values on every printing (candidates to store once on the oracle record). Tests: 15.

### Added
- 2026-10-22, 0.0.1-6: `coqc compile --progress` (stderr progress for both passes and the write) and `--breakdown` (estimated stored bytes per residual key). `scripts/package-bundle.sh` and `scripts/ingest-bundle.sh` (tested end to end in throwaway repos), expanded `.gitignore`, `.bundle-seq`. Real-data run: images 108,668 canonical / 0 raw, file 46.6 MB (was 52.0).

### Fixed
- 2026-10-22, 0.0.1-5: second real-data run: healed printings 3,981 -> 175, but images still 0% canonical because Scryfall's `jpeg` key points at a `/normal/` URL (JSON key != URL path segment). Image dictionary entries are now `key=segment.ext`. Compile output now lists every distinct image failure shape with counts, and shows a sample value for each large residual key (to design URL derivation). Test extended; 13 pass.

### Fixed
- 2026-10-22, 0.0.1-4: real-data findings. Images were never canonical because Scryfall added an `art` size served as `.webp` (assumed `.jpg` unless `png`); image dictionary entries are now `key.ext`. 3,716 of 3,981 healed printings were `legalities`, which differ between printings, so legalities moved from the oracle to the printing record. The oracle record now comes from the version most printings agree on instead of the newest printing. Tests added for each (13 total).

### Added
- 2026-10-22, 0.0.1-3: first real-data run (107,212 printings, verify 0 mismatches, 51.8 MB) showed 0 canonical image sets and 3,981 healed printings. `coqc compile` now prints why image URLs fail the canonical check and ranks healed keys by count with an example card each.

### Fixed
- 2026-10-22, 0.0.1-2: `scripts/fetch-bulk.sh` read the old `download_uri` field and exited silently; Scryfall's bulk-data response now carries `jsonl_download_uri` (a `.jsonl.gz`). The script now prefers it, gunzips by magic bytes, falls back to `download_uri`, and prints the response if neither exists. Tested offline via `COQC_BULK_META_URL`.

### Added
- 2026-10-22, 0.0.1-1: Phase 0 prototype. `coqc-format` (records, COQD container, JSON
  reconstruction) and `coqc` CLI (`compile`, `verify`, `inspect`, `diff`, `synth`). Lossless:
  every Scryfall key is kept (typed core + residual JSON), `prices` dropped by default.
  Stable append-only row ids across releases, tombstones, deterministic output, JSONL and
  JSON-array input. Test fixtures plus a synthetic data generator for scale checks.
