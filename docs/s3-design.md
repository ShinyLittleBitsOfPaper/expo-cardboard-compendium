# S3: generic index and query engine over `Store` (design 0.0.2-16; S3a built 0.0.2-18)

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

## S3a status (0.0.2-18, built)
- `coqc_format::gindex` (`GIndex`, `GExpr`, `GQuery`, `Val`, `DerivedCol`/`DData`, `Hooks`): generic evaluator over a `Store`. Primary table =
  the one with `Rel::Parent`; parent and sidecar predicates are evaluated on parent rows and lifted through the `Ref` column. Text columns
  declared `IndexKind::Text` get an automatic folded arena + trigram index; dialect `DerivedCol`s (Text/Mask/Num/Flag, optional shared
  trigram group) win name lookups; `Custom` predicates go to the dialect's `Hooks`. Sort, unique-by-parent + prefer, hide-flag, scope,
  offset/limit are generic. Query tree is serde JSON.
- `coqc_mtg::gindex`: `derive(&Dataset)` (still reads the legacy `Dataset`, same folding as `SearchIndex::build`), `MtgHooks`
  (`tilde`, `mv_parity`, `name_exact`), `translate(&Query) -> GQuery`.
- Old `search.rs` helpers (`Bits`, `Arena`, `Trigrams`, `fold*`, `oracle_mask`, ...) made `pub` so the new code reuses them; deleted in S3c.
- `crates/compiler/tests/gindex.rs`: ~120 queries x unique x prefer x include-extras over sample_a/sample_b (with and without skip flags)
  and 4000 synthetic cards: identical totals and row order to the old index. 54 tests pass.
- Not done in S3a (S3b): `.coqi` serialisation/delta of the generic index, deriving from the `Store` instead of the `Dataset`, engine/CLI switch,
  `Enum` ordering flag in the schema (rarity order is a derived Num for now), skipping trigram indexes on Text columns the dialect never searches
  (printing `set_name`/`flavor_text`/`artist` are built automatically today), regex.
- Synthetic 120k-card timing (`cargo test --release -p coqc --test gindex -- --ignored --nocapture`, sandbox machine): old build 736 ms,
  generic derive+build 1476 ms (extra auto text columns + derive pass; S3b target is parity); queries old vs generic (ms): bolt 0.15/0.31,
  t:creature 13.9/15.5, c:rg 0.21/0.37, o:flying 10.2/12.2, mv>=5 11.0/12.7, r:mythic 0.79/0.91, s:lea 5.4/1.2, -t:land 15.5/15.8,
  legendary creature 2.6/1.1. Per-row closures cost ~10-15% on full scans; a typed fast path for hot fields is the S3b fix if the real-data
  numbers need it.

## S3b status (0.0.2-19, built)
- `.coqi` v2 (`crates/format/src/coqi.rs`): header (magic, version 2, dataset SHA-256, section count + directory), meta section (table, column, kind, trigram id per column), one raw section per column, one per trigram table. `.coqid` delta = per-section row diff + posting diff; requires identical meta.
- `GIndex::build_parts` (expensive) / `GIndex::with_parts` (cheap re-attach, validates column lengths against the tables). Engine keeps `IndexParts` and attaches per search; a mismatching or foreign `.coqi` is rebuilt.
- Trimmed auto text columns (6 MTG columns). `derive_store` reads the `Store` directly. Engine/CLI no longer need `Dataset`.
- Not done: typed fast path for hot fields (only if real-data latency needs it), schema `ordered` enum flag, `.coqi` memory-mapping, S3c (delete `Dataset`, old `search.rs`/`coqs.rs`, `container.rs`; docs rewrite), S3d (regex, `is:`/`f:`/`pow`/`tou`).
- Synthetic 120k: .coqi 20.57 MB raw, 2.05 MB zstd; build 1.32 s vs old 0.94 s; queries (ms, old/generic): bolt 0.21/0.45, t:creature 12.4/12.9, o:flying 8.3/10.3, mv>=5 5.3/6.6, s:lea 2.3/0.65, -t:land 11.9/12.8.
