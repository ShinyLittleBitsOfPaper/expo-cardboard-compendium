# Prices (COQR + COQX)

Separate dataset from the cards (`crates/format/src/prices.rs`, `crates/compiler/src/prices.rs`).
Source: MTGJSON `AllPricesToday` (retail only, paper). v1 columns: TCGplayer USD and Cardmarket EUR x
normal/foil/etched. Buylists, other providers (cardkingdom, manapool) and history are not stored yet.

- Rows are PRINTING rows of the card file (same `cards_epoch`); rows past the table are "no price".
- Value 0 = absent, else cents+1. File = header + one zstd blob of 6 varint columns (deterministic, level 15).
- Delta `.coqx`: zigzag varint of (new - old) per cell, base/target SHA-256, replay must hash to the target.
- MTGJSON keys prices by its own uuid: `AllIdentifiers` supplies uuid -> `identifiers.scryfallId`
  (`coqc idmap` -> `.coqm`). Refreshed only when missing, or when >50 price uuids are unknown and the map
  is older than a day (new sets). Two uuids mapping to one Scryfall id (faces): first value wins, conflicts counted.
- Prices dated per MTGJSON `meta.date`; if a day is missing the client keeps yesterday's file.

CLI: `coqc idmap`, `coqc prices`, `coqc prices-patch`, `coqc prices-apply`, `coqc price <prices> <cards> <name>`.
Pipeline: `scripts/daily-prices.sh` (run after `scripts/daily.sh`).
