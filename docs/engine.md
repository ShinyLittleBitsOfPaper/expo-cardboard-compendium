# Engine and Expo module (0.0.1-24)

Crate `coqc-engine` (lib `coqe`, `crates/engine`): opens a `.coqd` and its `.coqi`, answers searches, looks up cards, applies
updates. The only native entry point is the C ABI in `ffi.rs`: `coqe_call(method, argsJson) -> {"ok":..}|{"error":".."}` plus
`coqe_free`. One global engine; call `open` first. `Engine::call` is the same dispatcher for Rust callers.

| Method | Args | Result |
|---|---|---|
| `open` | `coqd`, `coqi?` | info. `.coqi` used only if its header dataset hash matches the `.coqd`; otherwise rebuilt and rewritten |
| `search` | `query`, `limit?`, `offset?`, `scope?`, `cards?` | `{total, rows, cards?}`; rows are printing rows (stable, append-only) |
| `getRecords` (alias `getCards`) | `rows[]` | record JSON per row (MTG: card JSON; `null` if tombstoned/out of range) |
| `getByKey` (alias `getByScryfallId`) | `key` (or `id`) | record by natural key (MTG: Scryfall id) or `null` |
| `attachOverlay` | `id`, `path` (.coqd) | attaches (or replaces) an overlay dataset, e.g. `prices`; refused when its epoch differs from the open dataset. `info.overlays` lists id -> `{sha256, rows, sourceDate}`. Re-attach after a sync replaced the file |
| `getPrices` | `rows[]` | MTGJSON retail prices per row from the `prices` overlay, Scryfall-shaped: `{usd, usd_foil, usd_etched, eur, eur_foil, eur_etched}` (strings, `null` = no price); `null` for an unknown row. `getRecords` / `search {cards:true}` take `prices: true` to add the same object as `card.prices` |
| `rowsOf` | `ids[]` | row (or `null`) per Scryfall id, in order |
| `getByKeys` (alias `getByScryfallIds`) | `ids[]`, `prices?` | record (or `null`) per id, in order |
| `registerRows` / `dropSet` | `name`, `rows[]` / `name` | scope from rows (`{resolved, unknown}`; removed or out-of-range rows count as unknown) / remove it |
| `saveSets` / `loadSets` | `path` | persist all scopes (with the dataset epoch) / load them back: `{loaded}`; another epoch, a missing or bad file loads 0, so re-register from your own data |
| `detachOverlay` | `id` | removes it again |
| `registerSet` | `name`, `ids[]` | `{resolved, unknown}`; later `search {scope: name}` can only match those printings |
| `applyPatch` | `path` (.coqp) | replaces the .coqd atomically; index marked `stale` |
| `sha256File` / `unzstFile` / `patchFile` / `patchIndexFile` / `moveFile` / `removeFile` | `path` (`out`, `patch`, `from`/`to`) | stateless (no open engine): streamed SHA-256, zstd decompress to a file, atomic replace, delete (missing is fine); used by `sync()`; `patchFile {path, patch, out}` applies a `.coqp` to a file without an open engine (overlay updates); `patchIndexFile {path, delta, out}` applies a `.coqid` to a `.coqi` the same way, so the index matches the patched dataset without a rebuild on open |
| `applyIndexDelta` | `path` (.coqid) | refused unless it targets the current dataset hash; on failure the next search rebuilds |
| `info` / `close` | | |

Notes
- Default hiding (extras) still applies inside a scope; add `include:extras` if a collection can hold such cards.
- Memory: the dataset is held as a columnar `Store` (no per-record structs, no `Dataset`); the index columns are owned copies attached
  per search. Not yet measured on a device. A mapped, lazily-decoded reader is the next step if memory is a problem.
- `modules/expo-cardboard-compendium` is a scaffold (TS API, Kotlin + JNI shim, Swift, podspec) and `scripts/build-native.sh` cross-compiles.
  None of it has been built or run: there is no Android/Xcode toolchain here. Expect first-build fixes (Gradle plugin names, CMake paths).

## Release pipeline (`.github/workflows/native.yml`)
Versioning: one version everywhere, equal to the tag. `scripts/cut-release.sh major|minor|patch` does it all (bump, commit, tag, push); or by hand `scripts/set-version.sh X.Y.Z` (then commit, push, tag `vX.Y.Z`, push the tag);
the `test` job runs `scripts/check-version.sh` on tags and fails on a mismatch.
- Every push/PR: `cargo test --workspace`.
- Tag `vX.Y.Z` (or manual dispatch): build `libcoqe.so` for 3 Android ABIs (cargo-ndk installed with taiki-e/install-action, runner NDK) and
  `libcoqe.xcframework` (macOS runner), then `npm pack` the module with both inside and attach the `.tgz` to the GitHub Release.
  Version = tag without the `v`.
- slbop pins the release asset URL as the dependency (`"expo-cardboard-compendium": "https://github.com/<owner>/<repo>/releases/download/vX.Y.Z/expo-cardboard-compendium-X.Y.Z.tgz"`).
  A private repo cannot serve that URL anonymously; then publish to GitHub Packages (npm registry) instead, same tarball.
- iOS is optional for now: its job is `continue-on-error` and the package job runs after Android alone. A package built without the
  xcframework works on Android only (the podspec expects `libcoqe.xcframework`); rerun the workflow when a macOS runner is available.
- First real run (2026-10-26): `test` and `android` passed; the Android libs checked out: 3 ABIs, export `coqe_call`/`coqe_free`,
  depend only on libc/libdl, 64-bit slices are 16 KB aligned. `ios` could not get a macOS runner. Node 20 deprecation notices
  (actions/checkout@v4, upload-artifact@v4) are warnings only.
- Compiled libraries are gitignored; local development uses `scripts/build-native.sh`.

## Lock and slow-call diagnostics (0.0.3-17)
All engine calls share one global lock (a `Mutex` around the engine in `ffi.rs`), so a slow call (open, search, an overlay attach, a big batch) delays every other call. Stateless file helpers (`sha256File`, `unzstFile`, `patchFile`, ...) do not take it. A call that waited >= 100 ms for the lock or ran >= 250 ms carries `t: {method, wait, run, holder}` in its reply; the TS layer hands it to the observer registered with `setSlowCallObserver`. `holder` is the method that held the lock when the call queued, a best guess at what it waited for. Apps should log these (slbop writes `[CardEngine] slow native call ...` lines).
