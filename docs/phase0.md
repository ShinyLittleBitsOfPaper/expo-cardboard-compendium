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
