# Generalising coq: schema-driven format, engine and sync (DRAFT, 2026-10-27)

Status: **draft for approval, no code yet.** Written from a skim of the crates (types.rs, container.rs, patch.rs, search.rs,
coqs.rs, engine, manifest); verify each claim against the code in stage S0 before relying on it.

## Why now
Nothing depends on the formats yet: no data release has been published and slbop does not use the engine. Changing the formats later
costs a reader-version bump and a rebase for every installed client; now it costs only code.

## Where the Magic assumptions live today
- Generic already: `codec.rs`, `engine/files.rs`, ffi dispatch, the TypeScript `sync/` layer (only the names say "cards"/"prices").
- Generic machinery, MTG record shapes: `container.rs`, `patch.rs`, `search/coqs.rs` are written against two fixed record types
  (oracle, printing). `compiler/manifest.rs` has `cards`/`prices` hard-coded.
- MTG-specific: `types.rs`, `json.rs`, `parse.rs` (query syntax), `search.rs`, `prices.rs`, `compiler`, engine card lookup/JSON.

## Target model
**Dataset = tables of typed rows.** A table has columns and an optional parent table (MTG: `oracle` is the parent of `printing`).
Row ids are per table, stable and append-only; tombstones as today. Column types: string (dictionary or plain), int, float, bool,
enum, string set, list, uuid, and a lossless `extra` JSON passthrough (kept as a generic feature: nothing the source sends is dropped).
Each column declares how it is indexed: none, exact, text (trigram), numeric range, set membership.

**Self-describing files.** The schema (tables, columns, types, index kinds, field aliases, query operators) is stored in the `.coqd`
and mirrored in the `.coqs`. The engine reads layouts from the file; it contains no compiled-in card structs.

**Dialects for what data cannot express.** Some Magic semantics need code (colour subset/superset operators, type-line search,
`~` substitution in oracle text, mana-value comparison, `is:` shortcuts, `order:`/`unique:`/`prefer:`). They live in a registered
dialect (`coqc-mtg`) selected by the schema's id; the generic query language handles `field op value`, negation, `or`, parentheses,
quoted strings and regex, with fields and operator semantics taken from the schema. A schema without a dialect still works.

**Overlays replace prices.** `.coqr` generalises to an overlay: numeric columns attached to rows of one table, own file, own delta,
refreshed independently (MTG: prices keyed by printing row).

**Manifest v2.** A list of datasets with roles (`base`, `index`, `overlay`) instead of fixed `cards`/`prices`; the planner and executor
iterate over it. TypeScript API: generic `getRecords`/`getByKey`/`registerSet`; Magic helpers (`getByScryfallId`) become a thin wrapper.

## Proposed crate layout
`coqc-format` (container, codec, patch, table store, index, query engine) - `coqc-engine` (generic ABI, files) - `coqc-mtg`
(MTG schema, Scryfall/MTGJSON ingestion, dialect, card JSON) - `coqc` (generic `compile --schema`, patch, index, manifest; MTG
subcommands from `coqc-mtg`). Crate names stay as they are; only `coqc-mtg` is new.

## Stages (each ends with all tests green and a bundle)
- **S0 Lock behaviour.** Golden tests: query results over fixtures and the real 2026-10-27 data, sizes, timings (open ~<1 s, default
  search ~545 ms on the emulator). Audit the claims above.
- **S1 Boundary without format change.** Move MTG ingestion, query kinds and card JSON behind traits in `coqc-mtg`.
- **S2 Table model + schema block** (coqd v3). MTG expressed as a schema; compile, patch, verify operate over tables.
- **S3 Generic index and query engine** (coqs v2) with dialect hooks; MTG dialect reproduces today's results exactly.
- **S4 Overlays, manifest v2, sync and TS API generalised.**
- **S5 Proof: a second, non-MTG schema** (a small synthetic dataset) compiled, searched, patched, published offline and synced
  end to end.

## Acceptance
MTG golden queries identical before and after; `coqc verify` 0 mismatches; file sizes within +5%, search latency within +10% of the
numbers above; the second schema passes compile, patch, index delta and the simulated-client test with no Magic code involved.

## Decisions needed
1. Self-describing schema in the file plus compiled dialects (proposed), versus fully compiled-in schemas?
2. How generic should the query language be: schema-driven fields and operators (proposed), or Scryfall syntax kept as the only
   grammar with the data model alone generalised?
3. Should the very first data release wait for S4, so clients never see the old formats (proposed: yes)?
4. Keep `.coqd/.coqs/.coqr` extensions, or rename now since nothing is published?

## Risks
Generic columns may cost speed and size against today's tuned structs: measure at every stage and keep a fast path for hot fields.
Scope creep into a general database: the non-goal is joins, writes and SQL; this stays a read-mostly, patchable search dataset.
