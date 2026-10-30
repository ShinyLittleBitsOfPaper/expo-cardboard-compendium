# S3: generic index and query engine over `Store` (design, 0.0.2-16; no code yet)

Goal: `coqc-format` indexes and searches any `Store` from its schema; everything Magic-specific is a `coqc-mtg` hook. MTG goldens
(60 fixture queries, 9 real-data counts), `.coqi` size (+-5%) and query latency (+10%) must not move. Today `search.rs` (928 lines) and
`search/coqs.rs` (566) are written against `Dataset` (oracle/printing vectors), so S3 is a rewrite, not a move; it is cut into stages
that each end green.

## What the current index actually holds (the thing to generalise)
- Per **parent** row (oracle): folded name (3 variants: plain, `fold_name`, alnum-only for sorting), folded type line, oracle text with and
  without reminder text, colour mask, identity mask, mv; trigram postings over name and over text.
- Per **child** row (printing): oracle row, live flag, folded set code, folded collector number, rarity id, released date, hidden flag.
- Queries evaluate to bitsets over child rows; parent predicates run on parent rows and expand through `oracle_row`.
- MTG-only facts that are *derived*, not stored: face-joined text (faces are opaque `Bytes`), `~` expansion, hidden (vanguard, plane,
  art_series, memorabilia via `extra` JSON, playtest promo), rarity order, `fold_name` punctuation rules.

## Design
1. **Index plan from the schema.** For each column its `IndexKind` picks a structure, built once per table:
   `Text` = folded arena + byte-trigram postings (existing `Trigrams`, same stop-gram gate); `Exact` = folded arena (or id column) with
   equality/prefix scan, plus a value->postings map only if the column cardinality is small; `Set` = per-row u32 mask (colours, finishes)
   or postings per element (promo_types); `Range` = raw numeric column (scan; 120k rows is ~0.1 ms, no sorted copy needed).
   Parent/child: the query's *primary table* is the one with `Rel::Parent`; parent-column predicates evaluate on parent rows and expand
   through the `Ref` column (today's `expand`/`oracle_pred`).
2. **Derived columns (dialect hook).** `Dialect::derive(&Store) -> Vec<Derived>`: extra text/mask/number/flag columns computed at index
   build time and serialised in the `.coqi` (MTG: face-joined name/type/text variants, colour union over faces, hidden flag, rarity rank).
   This keeps faces opaque in the data file and keeps the generic code free of Magic.
3. **Generic query tree.** `Expr::{And, Or, Not, Cmp{field, op, value}, Text{field, needle, mode}, Regex{field, pattern}, Custom{name, args}}`.
   `field` is a column name or alias from the schema (or a derived column); operators `: = != < <= > >=` with semantics by column type
   (Int/Float numeric; Enum by dictionary order if the schema marks it ordered, else equality; Set masks: `:`/`>=` superset, `<=` subset,
   `=` equal, `<`/`>` proper). `Custom` is evaluated by a dialect hook returning a bitset (MTG: `~` text, `mv:even/odd`, colour counts,
   `!"name"`, later `is:`).
4. **Result shaping generic:** `sort` = list of (column, desc), `unique` = by parent ref with `prefer` = (column, newest/oldest),
   `default_filter` hook (MTG: hide extras unless `include:extras`), `scope`, offset/limit stay as they are.
5. **`.coqi` v2:** header (magic, version, SHA-256 of the `.coqd`, schema hash) + one section per (table, column or derived column),
   same 8-byte alignment, per-section row-level delta as today (`make_delta`/`apply_delta` generalise from named tables to sections).
6. **Regex** (new Scryfall feature, `/.../` on name/oracle/type) = `Regex` node: trigram candidate extraction from literal runs when
   the pattern has any, otherwise full scan with the `regex` crate on folded text (decision: folded, case-insensitive; `regex` crate
   with size limits so a hostile pattern cannot blow up on a phone).

## Stages
- **S3a** Generic column index builders + generic `Expr`/evaluator over a `Store`, built next to the old index and cross-checked against it
  in tests (same bitsets for all 60 golden queries). Dialect hook `derive` + `Custom`. No file change yet.
- **S3b** MTG dialect emits the generic tree; engine and `coqc search` switch to the new index; `.coqi` v2 (+ `index-diff/apply` on sections);
  rename `.coqs` -> `.coqi` here. Goldens/sizes/timings re-measured by the user on real data.
- **S3c** Delete `Dataset`, `OracleRecord`/`PrintingRecord`, `container.rs`, old `search.rs`/`coqs.rs` from `coqc-format`; MTG ingestion
  writes `Store` directly (the legacy structs move into `coqc-mtg` only if ingestion still needs them). Docs rewrite for v3 (format.md,
  search.md, engine.md, README).
- **S3d** Regex + `is:`/`f:`/`pow`/`tou` on the generic tree (the parser gaps listed in `s0-audit.md`).

## Decisions taken (change any before S3a starts)
- Scan, not sorted copies, for `Range`; postings only for trigram and small-cardinality `Exact`/`Set` columns (size budget).
- Enum ordering is declared in the schema (`ordered: true` on the dictionary; MTG rarity: common<uncommon<rare<special<mythic<bonus).
- Derived columns live in the `.coqi`, never in the `.coqd`.
- Query JSON is the generic tree above; the MTG dialect's `parse_query` stays the only Scryfall-syntax entry point.

## Needs from the user before S3b (not S3a)
Single-line `-t:land` re-run (expect 37848) and the real-data timing loop again after S3b; nothing is needed for S3a.
