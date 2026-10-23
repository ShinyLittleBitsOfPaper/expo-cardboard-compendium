# COQD format, version 1

Little-endian. Strings are `varint length + UTF-8`. Optional values use `0 = absent`.

## File

```
"COQD" | u16 format_version | u16 min_reader | u32 cards_epoch | u32 source_date
u32 oracle_count | u32 printing_count | u32 section_count
section_count x { u32 id, u64 offset, u64 len }        (absolute offsets)
```

Sections: 1 meta, 2 oracle blocks, 3 printing blocks, 4 oracle uuid index, 5 printing uuid index.
Readers reject `min_reader` above what they support.

## Blocks

`u32 records_per_block (256) | u32 block_count`, then per block `{u64 off, u32 clen, u32 rawlen,
[u8;8] hash}` (hash = first 8 bytes of SHA-256 of the compressed block), then the zstd blocks
(level 15). A raw block is `varint(len) + record` repeated. Row N is in block N / 256.

## UUID index

`u32 count`, then `{[u8;16] uuid, u32 row}` sorted by uuid. Includes tombstones, so ids are never
reused. Derivable from the records; a patch format need not ship it.

## Meta

`cards_epoch`, `source_date`, seven append-only dictionaries (layouts, rarities, border_colors,
langs, promo_types, image_keys, formats), and `dropped_keys`.

## Records

Oracle (rules-level, one per `oracle_id`): name, mana_cost, type_line, oracle_text, colors and
color_identity (WUBRG mask, absent != empty), cmc (f64), faces (name/mana_cost/type_line/oracle_text + `extra`), `extra`
(power, toughness, keywords, edhrec_rank, ...; see `ORACLE_EXTRA_KEYS`).

Printing (one per Scryfall card id): oracle_row, layout, lang, released_at (YYYYMMDD), set,
set_name, collector_number, rarity, border_color, promo/reprint/digital (tri-state), finishes mask
(nonfoil 1, foil 2, etched 4), promo_types, legalities (nibble per format id: 1 legal, 2 not_legal,
3 restricted, 4 banned; per printing because real data differs between printings), flavor_text, artist, images, faces, `extra`.

The oracle record is built from the version most printings of a card agree on (ties: newest);
printings that differ keep their own values via `extra`.

Images are `Canonical {keys, stamp}` (keys are dictionary entries `key.ext`, e.g. `normal.jpg`,
`art.webp`) when every URL equals
`https://cards.scryfall.io/{key}/{front|back}/{id[0]}/{id[1]}/{id}.{jpg|png}?{stamp}` (checked by the
compiler), else `Raw` JSON.

`extra` is JSON text holding every key not modelled above, plus overrides written by the compiler's
self-check when a typed value would not round-trip. `__absent` lists keys to remove.
Rebuild order: oracle typed, oracle extra, printing typed, faces, printing extra, `__absent`.

## Stability rules (what keeps daily diffs small)

Row ids and dictionary ids are append-only. Output must not depend on input order or wall-clock
time. Pinned: zstd version (Cargo.lock) and level.
