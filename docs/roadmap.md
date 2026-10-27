# Roadmap: finish the compendium and the data pipeline before the slbop swap

Status 2026-10-27, version 0.0.2. Decisions marked **Decision** carry a proposed default; change them here before building.

## Where we are
Done and tested: compiler (`coqc`), `.coqd` cards (v2) with record-level patches, `.coqs` search index + deltas, `.coqr/.coqx` prices
(MTGJSON) + deltas, Rust engine behind a JSON C ABI, Expo module (Android verified on an x86 emulator inside slbop: import, open, search),
tag-driven CI for the module (`native.yml`), local scripts `daily.sh` / `daily-prices.sh`.
Not done: automated publishing (no cron), client-side download/update flow, prices in the engine, real-device memory/speed numbers,
iOS library, conformance testing against Scryfall's own search results, published data is blocked on Scryfall's permission reply.

## Milestone 1: the cron job (data publishing)
Goal: a scheduled GitHub Action compiles, verifies and publishes data; clients can find it from one stable URL.

**Decision 1 (hosting): a separate public data repo**, `ShinyLittleBitsOfPaper/cardboard-compendium-data`, whose workflow checks out
the code repo at a pinned tag, builds `coqc`, and publishes to the data repo's own Releases with its own `GITHUB_TOKEN` (no PAT).
Why: "latest release" URLs stay unambiguous (the code repo's `v*` releases would collide), pruning data releases never touches code
releases, the source or host can change (e.g. if Scryfall declines, or a move to Cloudflare R2) without touching code, and the token
scope is small. Alternative: one repo with a rolling `data-latest` tag.

**Decision 2 (cadence): once a day, ~22:00 UTC**, after Scryfall's ~21:05 refresh (they refresh about every 12 h, 09:05/21:05). Prices
(MTGJSON, daily) run in the same job. Twice daily is a config change if freshness matters. GitHub cron is best effort (can be late or
skipped), so the job is idempotent: it exits cleanly if the bulk `updated_at` is already released, and `workflow_dispatch` is allowed.

Release layout (tag `data-YYYYMMDDHHMM`, one per run):
- `cards-<stamp>.coqd`, `cards-<stamp>.coqs.zst`, `prices-<date>.coqr`: full files, kept only on the newest 2 releases (assets of older
  releases are deleted; clients never need an old full file).
- `cards-<prev>-to-<stamp>.coqp`, `...coqsd`, `prices-<old>-to-<new>.coqx`: patches from the previous release, kept for 30 days.
- `manifest.json` (also at a fixed URL, see below): format versions, epoch, current stamps, sha256 + size of every asset, the patch chain
  for the last 30 days, `min_client_format`, `generated_at`, `rebase` flag.
- Stable entry point: the data repo's `releases/latest/download/manifest.json`.

Pipeline steps (any failure = no publish, workflow fails and emails):
1. Download the previous release's `.coqd`/`.coqs` (for `--prev` row stability); none => first release / new epoch.
2. Fetch bulk (UA header set), compile, `verify` against the raw file (0 mismatches required), patch + replay byte-identical.
3. Index build + delta + replay, price compile + delta + replay.
4. Create the release as a draft, upload everything, verify the uploaded sizes/hashes, then publish and replace the manifest last.
5. Prune: delete full-file assets beyond the newest 2, patches older than 30 days.
- **Epoch/rebase**: a format version bump or heavy tombstone growth starts a new epoch (`rebase: true`); clients on an older epoch
  download the full files. Prices built against an older card release stay valid inside an epoch (rows only append).
- **Card ids that Scryfall retires**: consume Scryfall's migrations feed (merge/delete) and publish a small `migrations.json` so user data keyed
  by `scryfall_id` can be redirected instead of silently orphaned. Needs design before slbop re-keys its data.
- Dry-run mode (`workflow_dispatch` input `publish=false`) runs everything and uploads the files as workflow artifacts only.
- Failure alerts: GitHub's failure email at minimum; optional Discord webhook.

Exit criteria: three consecutive scheduled runs publish verified releases, and a client starting from day 1 reaches day 3 via the patch
chain with hashes matching the full files.

## Milestone 2: client sync inside the module
**Decision 3: the module owns download/verify/apply** (TypeScript layer using expo-file-system for downloads; the Rust engine stays free
of networking/TLS). New API: `sync({manifestUrl, dir})` returns what happened (`up_to_date | patched(n) | full | needs_app_update`).
Logic: read manifest, compare local stamp/epoch, plan (patch chain if every link is present, else full), download to a temp name,
verify sha256 (engine gets a `sha256File` call), apply `applyPatch` + `applyIndexDelta`, swap atomically, fall back to a full download on any
failure, never leave the app without a working dataset. Planning logic is pure and unit-tested in Jest/Node with mock manifests.
`min_client_format` > module's supported format => `needs_app_update` (the app shows "update the app").

## Milestone 3: engine completeness and measurements
1. Real-device numbers on an arm64 phone: open time, search latency, peak memory (the engine decodes the whole dataset into RAM today).
   If memory is a problem: lazy block decode of printing records and a mapped index.
2. Prices in the engine: `openPrices`, `getPrices(rows)`, `applyPriceDelta`, and price terms in search (`usd>=`, `eur<`) now that data exists.
3. Batch helpers for user data: `getByScryfallIds(ids[])`, `rowsOf(ids[])`, persistent named sets; row tombstones surface as `null`.
4. Sort and uniqueness controls slbop needs (`order:`, `unique:`, `prefer:`): audit against the queries slbop actually issues today.
5. Robustness: fuzz/malformed-input tests for every reader (`.coqd/.coqs/.coqr` and all patch formats), no panics across the C ABI.
6. iOS: build the xcframework (macOS runner capacity; retry or another runner label), then build it in an iOS app once.
7. Conformance corpus: a few hundred queries whose result sets are compared against Scryfall's own search on the same day (rate-limited,
   with a UA), run on demand; the list of known differences goes in `docs/search.md`.

## Milestone 4: before slbop depends on it
Publish the module (tagged tarball is enough), write the data-source adapter note (Scryfall bulk now; MTGJSON as a fallback source if
permission is refused), and keep the "dataset-agnostic" direction as a design note only (README Naming) until slbop is live.

## Measurements still needed (from your machine)
- Real `.coqp` and `.coqsd` sizes between two consecutive releases (`ls -l data/daily/patches`), and `.coqr/.coqx` sizes.
- Open time and a search time from the device (Settings > Card engine), plus the reported `index` value.

## After this: slbop (summary, see slbop TODO #104)
Re-key user data from `card_ref_id` to `scryfall_id`, move card joins app-side, engine-backed search behind a flag, prices, scanner
lookups, then retire the SQLite card ingestion. Only start once Milestones 1 and 2 hold.
