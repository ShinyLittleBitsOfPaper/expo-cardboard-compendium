# Phase 0 findings and open items (0.0.1-1)

## Measured (synthetic data, 100k printings, 356 MB JSONL; real data will differ)

| | |
|---|---|
| Output file | 23.7 MB (oracle 1.6, printing 19.6, uuid indexes 2.4) |
| Compile | ~13 s ingest + ~7 s write; peak RSS ~550 MB (~780 MB with `--prev`) |
| Determinism | rebuild gives the identical SHA-256 |
| Modelled day-2 churn | 54% of printing blocks and 100% of oracle blocks change: 12.2 MB block patch |

## Real data (default_cards 2026-10-22, 0.0.1-5)

119,155 lines -> 107,212 printings, 35,736 oracle cards (9,168 digital and 2,775 art_series skipped).
`verify`: 0 mismatches. Images: 108,668 canonical, 0 raw. Healed printings: 175 (83 reversible-card
printings, 85 attraction_lights, a few oddities). File 46.6 MB: printing 297 MB raw -> 40.4 MB,
oracle 12 MB -> 3.5 MB, uuid indexes 2.7 MB. Compile ~26 s + write ~16 s, verify ~10 s.
Largest residual keys (raw): related_uris 63 MB, purchase_uris 49, all_parts 37, then `*_uri`.

## Per-key stored cost (real data, `--breakdown`, printing section 40.4 MB stored)

all_parts 6.2 MB (15%), related_uris 3.1, prints_search_uri 2.0, purchase_uris 2.0, illustration_id 1.9,
rulings_uri 1.9, uri 1.8, scryfall_uri 1.6, artist_ids 1.6, set_uri 1.4, set_id 1.4; the rest of the
residual keys are under 1 MB each; typed fields, images and structure about 11.2 MB. The large raw
URLs (related_uris, purchase_uris) compress ~20-25x, so deriving them buys little. Derivable from other
fields and worth ~7 MB together: `uri`, `rulings_uri` (card id), `prints_search_uri` (oracle id),
`set_uri` (set_id). Set-level fields (`set_id`, `set_uri`, `set_search_uri`, ...) should compress far
better when rows are grouped by set (0.0.1-7 default); compare `--row-order uuid` vs `set`.

## Row order and real daily churn (2026-10-22 -> 2026-10-23)

- `--row-order set` (new rows by set, then collector number) vs uuid: 33.6 MB vs 46.6 MB.
- Day 1 -> day 2: 119,155 -> 119,185 lines; 31 new printings, 14 new oracle cards, 1 tombstone.
  Block diff: oracle 140/140 blocks changed (3.47 MB), printing 92/419 (7.25 MB), naive patch
  10.72 MB of 33.6 MB. A handful of real changes dirties most blocks, so something volatile changes
  on every card daily. `coqc diff old new --keys` names the fields. Decision pending on that output:
  move volatile fields to their own section, and ship record-level patches.

## Daily churn, resolved in 0.0.1-9

`diff --keys` on real data (22 -> 23 Oct): oracle `edhrec_rank` changed on 28,932 of 35,736 cards and
`penny_rank` on 16,337 (everything else on 1-7 rows); printings: only 317 existing rows changed
(`purchase_uris` 192, `all_parts` 116, `cardmarket_id` 92, `mtgo_id` 84, image stamps 77) plus 31 new
and 1 removed. Fix: ranks table + record-level patches. Synthetic check: 0.04 MB patch vs 1.88 MB
block patch. Still to confirm on real data with `scripts/daily.sh` over consecutive days.

## Findings

1. **Residual JSON dominates the raw size, but zstd already removes most repetition.** Many URLs
   embed the card's own id, set or name (and `related_uris`/`purchase_uris` use affiliate-wrapped,
   percent-encoded forms), but each card's unique parts (ids, numbers) must be stored once anyway.
   Run `coqc compile --breakdown` to see what each key really costs after compression before
   building any URL derivation; it may save far less than the raw sizes suggest.
2. **Block-level patches are too coarse.** Scattered edits (tombstones, flavor text, rank changes)
   dirty most 256-record blocks. Ship record-level deltas (changed rows only) and apply them
   client-side, or use much smaller blocks.
3. **Volatile fields** such as `edhrec_rank` and `penny_rank` may change for many cards daily and
   dirty every oracle block. Check real day-over-day diffs; if confirmed, move them to a
   separate small section.
4. **Oracle fields differing across printings** are handled (`healed printings` in the compile
   output) but cost bytes. Real count unknown.

## Unverified (needs the real bulk file)

- Bulk format: confirmed JSONL via `jsonl_download_uri` (`.jsonl.gz`, ~80 MB compressed, default_cards 2026-10-22). Fetch script fixed in 0.0.1-2; not yet run end to end against the real download.
- Canonical image pattern on real DFC back faces and reversible cards (`images raw` count).
- Real size, timing, memory, and `healed printings` count.
- Real day-over-day diff via `coqc diff`.

## Not built yet

Search index (trigram, postings), price dataset (MTGJSON), patch format, manifest and publish
workflow (GitHub Releases), Rust engine (reader/query), UniFFI/JSI Expo module, example app.

## Open items

- Scryfall permission to publish derived data (email sent as a contact-form request; no reply yet).
  If declined: switch the published source to MTGJSON (has `scryfallId`).
- Repo name: avoid "scryfall" if the source may change.
- Volatile-field and URL-templating decisions after real-data numbers.
