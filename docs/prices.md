# Prices (price overlay, `.coqd` + `.coqp`)

Since S4a (0.0.2-24) prices are an ordinary `.coqd` dataset, not a separate format (`.coqr`/`.coqx` are gone). Code:
`crates/mtg/src/prices.rs` (schema, `PriceTable`, MTGJSON ingestion), generic container/patch in `crates/format`.
Source: MTGJSON `AllPricesToday` (retail only, paper). v1 columns: TCGplayer USD and Cardmarket EUR x
normal/foil/etched. Buylists, other providers (cardkingdom, manapool) and history are not stored yet.

- Schema id `mtg-prices`, one **columnar** table `price` (`TableDef::columnar`: Int columns only, stored column by column) with
  columns `tcg_normal, tcg_foil, tcg_etched, cm_normal, cm_foil, cm_etched`; a cell is cents, NULL = no price.
- Row i = PRINTING row i of the card file with the same `epoch` (store prop `overlay_of = printing`); rows past the table mean "no
  price". `source_date` = MTGJSON `meta.date`.
- Patch = ordinary `.coqp` (COQP v2) between two overlays; for a columnar table the patch body is one zigzag-varint delta stream per
  column, so a daily patch costs about what the old `.coqx` did. Synthetic 120k rows: full 332,756 B (old `.coqr` 325,185, +2.3%),
  daily patch 70,175 B (old `.coqx` 68,868, +1.9%).
- MTGJSON keys prices by its own uuid: `AllIdentifiers` supplies uuid -> `identifiers.scryfallId`
  (`coqc idmap` -> `.coqm`). Refreshed only when missing, or when >50 price uuids are unknown and the map
  is older than a day (new sets). Two uuids mapping to one Scryfall id (faces): first value wins, conflicts counted.
- If MTGJSON misses a day the client keeps yesterday's file.

CLI: `coqc idmap`, `coqc prices`, `coqc prices-patch`, `coqc prices-apply`, `coqc price <prices.coqd> <cards.coqd> <name>`.
Pipeline: `scripts/daily-prices.sh` (run after `scripts/daily.sh`).
The manifest lists prices as an `overlay` dataset (v2, S4b; docs/manifest.md). The TypeScript sync is on manifest v2 (prices are synced as the full `prices.coqd`, 0.0.2-26). Source policy: prices come exclusively from MTGJSON (retail, paper, latest date per finish: TCGplayer -> `usd*`, Cardmarket -> `eur*`). Scryfall's `prices` object is dropped at compile time (`--keep-prices` exists for debugging only; no script uses it), so card JSON from the engine never carries Scryfall prices. The engine attaches it (`attachOverlay`) and searches `usd>1`, `eur<=0.5`, ... (docs/search.md); the sync patches it on disk with `patchFile` (S4c part 2, 0.0.2-27). Device behaviour is unverified.
