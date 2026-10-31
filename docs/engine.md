# Engine and Expo module (0.0.1-24)

Crate `coqc-engine` (lib `coqe`, `crates/engine`): opens a `.coqd` and its `.coqi`, answers searches, looks up cards, applies
updates. The only native entry point is the C ABI in `ffi.rs`: `coqe_call(method, argsJson) -> {"ok":..}|{"error":".."}` plus
`coqe_free`. One global engine; call `open` first. `Engine::call` is the same dispatcher for Rust callers.

| Method | Args | Result |
|---|---|---|
| `open` | `coqd`, `coqi?` | info. `.coqi` used only if its header dataset hash matches the `.coqd`; otherwise rebuilt and rewritten |
| `search` | `query`, `limit?`, `offset?`, `scope?`, `cards?` | `{total, rows, cards?}`; rows are printing rows (stable, append-only) |
| `getCards` | `rows[]` | card JSON per row (`null` if tombstoned/out of range) |
| `getByScryfallId` | `id` | card JSON or `null` |
| `registerSet` | `name`, `ids[]` | `{resolved, unknown}`; later `search {scope: name}` can only match those printings |
| `applyPatch` | `path` (.coqp) | replaces the .coqd atomically; index marked `stale` |
| `sha256File` / `unzstFile` / `moveFile` / `removeFile` | `path` (`out`, `from`/`to`) | stateless (no open engine): streamed SHA-256, zstd decompress to a file, atomic replace, delete (missing is fine); used by `sync()` |
| `applyIndexDelta` | `path` (.coqid) | refused unless it targets the current dataset hash; on failure the next search rebuilds |
| `info` / `close` | | |

Notes
- Default hiding (extras) still applies inside a scope; add `include:extras` if a collection can hold such cards.
- Phase 1 limits: the dataset is decoded into memory and the index is an owned copy (reload ~24 ms, dataset decode not yet measured on a
  device). A mapped, lazily-decoded reader is the next step if memory is a problem.
- `modules/expo-cardboard-compendium` is a scaffold (TS API, Kotlin + JNI shim, Swift, podspec) and `scripts/build-native.sh` cross-compiles.
  None of it has been built or run: there is no Android/Xcode toolchain here. Expect first-build fixes (Gradle plugin names, CMake paths).

## Release pipeline (`.github/workflows/native.yml`)
Versioning: one version everywhere, equal to the tag. `scripts/set-version.sh X.Y.Z` (then commit, push, tag `vX.Y.Z`, push the tag);
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
