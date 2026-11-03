# Publish preparation

Module distribution is a tagged GitHub Release tarball (no npm publish needed). Status as of 0.0.2-33.

## Blocking items before the first public data release
- [ ] Scryfall permission to publish derived data (NOTICE.md, docs/phase0.md). Without it: do not run `data.yml` with `publish=true`;
      code releases are unaffected. Fallback plan: docs/adapters.md.
- [ ] Device numbers on an arm64 phone (open time, search latency, peak memory; roadmap M3.1). Emulator only so far.
- [ ] Device test of `sync()` / `openSynced()` (expo-file-system calls unverified).
- [ ] iOS xcframework (needs a macOS runner); until then the package is Android-only. State this in the README of the release.

## Code release procedure (module tarball)
1. `scripts/set-version.sh X.Y.Z`; commit; push to `trunk`.
2. Tag `vX.Y.Z` and push the tag. `native.yml` checks tag vs versions (`scripts/check-version.sh`), runs tests, builds Android
   (3 ABIs) and iOS (`continue-on-error`), `npm pack`s the module and attaches the `.tgz` to the GitHub Release.
3. slbop pins the release asset URL in `package.json`; the data pipeline is independent (`data.yml`, `data-latest` release).

## Data release procedure
1. `data.yml` with `publish=false` (dry run: stage, sizes, manifest) until three consecutive runs are clean.
2. First publish is a rebase (`rebase=true`); afterwards the daily cron (22:00 UTC) publishes patches.
3. Clients read only `releases/download/data-latest/manifest.json`; never `releases/latest`.

## Package hygiene (open)
- Module `package.json` lacks nothing functional; `description` still says "Scryfall-syntax card search" (accurate for the `mtg`
  dialect, generic datasets use their own query language). Keep until a second dataset ships.
- `peerDependencies` already lists `expo`, `expo-modules-core`, `react-native`.
- LICENSE files (root and module) must stay identical.
- No `exports` map; consumers import the package root (`src/index.ts`).
