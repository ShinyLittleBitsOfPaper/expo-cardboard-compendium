# Data-source adapters

The engine, index, patches, manifest and sync are source-agnostic (schema-driven, see `schema-design.md`). Only the **ingest**
step knows a source. This note records what each source must provide so a replacement can be written without touching the rest.

## Current sources

| Dataset | Source | Entry point | Output |
| :-- | :-- | :-- | :-- |
| `cards` (schema `mtg`) | Scryfall bulk `default_cards` JSONL | `scripts/fetch-bulk.sh`, `coqc compile` (`crates/mtg/src/ingest.rs`) | `.coqd` base |
| `prices` (schema `mtg-prices`) | MTGJSON `AllPricesToday` + `AllIdentifiers` | `scripts/daily-prices.sh`, `coqc idmap`, `coqc prices` (`crates/mtg/src/prices.rs`) | `.coqd` overlay |
| any other (schema id != `mtg`) | JSONL per table | `coqc generic-compile`, `generic-overlay` (`crates/generic`) | `.coqd` base / overlay |

Scryfall's `prices` object is dropped at compile time; prices are MTGJSON only (docs/prices.md).

## What the `mtg` ingest reads from a card line

One JSON object per printing, Scryfall field names: `id` (uuid, the printing key), `oracle_id` (uuid, the card key; lines without
one are skipped and counted), `name`, `lang`, `layout`, `mana_cost`, `cmc`, `type_line`, `oracle_text`, `colors`, `color_identity`,
`legalities`, `card_faces` (per-face name/cost/type/text/colors/images), `image_uris`, `set`, `set_name`, `collector_number`,
`rarity`, `released_at`, `border_color`, `finishes`, `promo`, `promo_types`, `reprint`, `digital`, `artist`, `flavor_text`,
`edhrec_rank`, `penny_rank`, `prices` (ignored), and the `art_series` layout. Unknown keys are kept in the lossless "extra" payload
and counted per key (`Stats.extra_*`), so a source that adds fields does not break ingest. Digital and art_series printings are
included by default (decision 2026-10: Scryfall search includes them).

Row ids are stable across runs through `--prev` (new keys appended, vanished keys tombstoned). Any adapter must keep that contract:
the printing key must be stable between runs and the price overlay's row i must equal printing row i of the same `epoch`.

## Fallback: MTGJSON as the card source

Needed only if Scryfall refuses permission to publish derived data (docs/phase0.md; no reply yet). Two options, in order of cost:

1. **Shim to Scryfall shape (recommended).** A small converter (`coqc mtgjson-to-scryfall`, not written) reads `AllPrintings`
   and emits the JSONL above, so ingest, goldens, index, search and the app keep working unchanged. Required mapping: MTGJSON `uuid`
   is not Scryfall's id, so use `identifiers.scryfallId` as `id` and `identifiers.scryfallOracleId` as `oracle_id`; faces from
   `otherFaceIds`/`side`; `colors`/`colorIdentity`, `manaValue` -> `cmc`, `legalities` (same format names, different value
   spelling: `Legal` vs `legal`), `finishes`, `promoTypes`, `isReprint`, `isPromo`, `availability` (`mtgo`/`arena` only => `digital`),
   `edhrecRank`. Image URLs are not in MTGJSON: Scryfall CDN URLs can be built from `scryfallId`
   (`https://cards.scryfall.io/<size>/front/<id[0]>/<id[1]>/<id>.jpg`), which still depends on Scryfall's CDN.
   Known losses: `penny_rank` (MTGJSON has `edhrecSaltiness`, not Penny Dreadful rank), some `promo_types` and `lang` rows
   (MTGJSON lists foreign data under `foreignData`, not as separate printings), and Scryfall-only digital printings.
2. **Native `mtgjson` dialect.** New schema id and ingest in `coqc-mtg`-style crate. Only worth it if the Scryfall shape is dropped
   for good; it would change the query semantics the app depends on (Scryfall syntax is the contract, docs/search.md).

Prices already use the MTGJSON join (`AllIdentifiers` uuid -> `scryfallId`), so option 1 keeps that join trivially identity.

## Writing a new adapter (checklist)

1. Pick the schema id. `mtg` selects the Scryfall dialect; any other id uses the generic dialect (schema-driven query language).
2. Produce a `.coqd` base with stable row ids (`--prev`), an `epoch`, and a `source_date` prop.
3. Optional overlays as columnar Int `.coqd` files (row i = base row i, `overlay_of` prop).
4. Run `coqc verify` (0 mismatches) and the golden/pipeline scripts (`scripts/test-data-pipeline.sh`, `scripts/test-generic-pipeline.sh`).
5. Register the files with `coqc manifest --base/--index/--overlay` (docs/manifest.md); the TS sync needs no change.
