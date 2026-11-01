# COQD format, version 3 (schema driven)

(0.0.2-14, S2b of docs/schema-design.md.) The authoritative layout is the header comment
of `crates/format/src/coqd.rs`: a section directory (`kind`, `arg`, offset, len), a SCHEMA section (tables, typed columns, relations,
index kinds, aliases), a META section (epoch, source date, dictionaries, props, per-table row counts), and per table a ROWS section
(256-row zstd blocks; rows are written from the schema) plus, for tables with a natural key, a sorted KEYS section. MTG is the
`mtg` schema (`oracle`, `oracle_ranks` sidecar, `printing`). Patches are COQP v2 (per-table changed/appended rows).

The schema model is in docs/schema-design.md; the in-memory shapes are
`Store` (generic, `crates/format/src/table.rs`) and, for MTG ingestion only, `coqc_mtg::Dataset` (`crates/mtg/src/types.rs`).
The search index is a separate file, `.coqi` (docs/search.md).

## Stability rules (what keeps daily diffs small)

Row ids and dictionary ids are append-only. Output must not depend on input order or wall-clock
time. Pinned: zstd version (Cargo.lock) and level.

## Patches (`.coqp`)

Row patches over stores (COQP v2): per table, the changed and appended rows, with the base and target hashes; the base hash is checked first and the replayed file must hash to the target (a mismatch, for
example a different zstd build, means: download the full file). Patches only exist within one epoch and only for growing row
counts. They reproduce the new
file byte for byte (`crates/format/src/patch.rs`).

## Columnar tables (S4a)
A table with `TableDef::columnar` (root table, no key, Int columns only; schema flag = bit 7 of the relation tag) has no row blocks.
Its ROWS section is `u32 rows | u32 raw_len | zstd(raw)`, raw = one tombstone byte per row, then each column as `rows` varints of
`0 | zigzag+1`. In a COQP patch such a table is written as a column-delta stream instead of a row list (mode byte 1; row lists are
mode 0). `block_table` refuses columnar tables. Used by the price overlay (docs/prices.md).
