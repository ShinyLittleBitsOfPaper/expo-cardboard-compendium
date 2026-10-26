# Engine and Expo module (0.0.1-24)

Crate `coqc-engine` (lib `coqe`, `crates/engine`): opens a `.coqd` and its `.coqs`, answers searches, looks up cards, applies
updates. The only native entry point is the C ABI in `ffi.rs`: `coqe_call(method, argsJson) -> {"ok":..}|{"error":".."}` plus
`coqe_free`. One global engine; call `open` first. `Engine::call` is the same dispatcher for Rust callers.

| Method | Args | Result |
|---|---|---|
| `open` | `coqd`, `coqs?` | info. `.coqs` used only if its header dataset hash matches the `.coqd`; otherwise rebuilt and rewritten |
| `search` | `query`, `limit?`, `offset?`, `scope?`, `cards?` | `{total, rows, cards?}`; rows are printing rows (stable, append-only) |
| `getCards` | `rows[]` | card JSON per row (`null` if tombstoned/out of range) |
| `getByScryfallId` | `id` | card JSON or `null` |
| `registerSet` | `name`, `ids[]` | `{resolved, unknown}`; later `search {scope: name}` can only match those printings |
| `applyPatch` | `path` (.coqp) | replaces the .coqd atomically; index marked `stale` |
| `applyIndexDelta` | `path` (.coqsd) | refused unless it targets the current dataset hash; on failure the next search rebuilds |
| `info` / `close` | | |

Notes
- Default hiding (extras) still applies inside a scope; add `include:extras` if a collection can hold such cards.
- Phase 1 limits: the dataset is decoded into memory and the index is an owned copy (reload ~24 ms, dataset decode not yet measured on a
  device). A mapped, lazily-decoded reader is the next step if memory is a problem.
- `modules/expo-scryfall-engine` is a scaffold (TS API, Kotlin + JNI shim, Swift, podspec) and `scripts/build-native.sh` cross-compiles.
  None of it has been built or run: there is no Android/Xcode toolchain here. Expect first-build fixes (Gradle plugin names, CMake paths).
