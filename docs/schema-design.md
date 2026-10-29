# Generalising coq: schema-driven format, engine and sync (DECISIONS MADE 2026-10-28)

Status: **decisions below approved, no code yet.** Written from a skim of the crates (types.rs, container.rs, patch.rs, search.rs,
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
quoted strings (regex is not implemented today; it is a new S3 feature), with fields and operator semantics taken from the schema. A schema without a dialect still works.

**Overlays replace prices.** `.coqr` generalises to an overlay (a `.coqd` with the overlay flag): numeric columns attached to rows of one table, own file, own delta,
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
- **S1 Boundary without format change (done 0.0.2-11: `coqc-mtg` crate holds ingestion, price ingestion, query parser, card JSON, synth; `coqc_format::dialect::Dialect` = parse_query/record_json/key_map/parse_key, the engine takes a boxed dialect (`Engine::open_with`); `coqc` re-exports the old paths; `compile`/`verify`/`breakdown`/`diff_keys` still operate on the fixed oracle/printing `Dataset` until S2; golden + all 45 tests unchanged).** Move MTG ingestion, query kinds and card JSON behind traits in `coqc-mtg`.
- **S2 Table model + schema block** (coqd v3). MTG expressed as a schema; compile, patch, verify operate over tables. Split in three:
  - **S2a (done 0.0.2-13, no format change):** `coqc_format::schema` (`Schema`/`TableDef`/`ColumnDef`, column types Str/Int/Float/Bool/Uuid/
    Enum/Set/Ref/Bytes/Json, index kinds, aliases, `Rel::{None,Parent,Sidecar}`, validation, binary + JSON encodings) and
    `coqc_format::table` (columnar `Table`, `Store` with `check()`); `coqc_mtg::tables` = `mtg_schema()` + lossless `to_store`/`from_store`.
    MTG ranks are a sidecar table (one row per oracle row) so volatile numbers stay out of the oracle blocks; faces, images and legalities
    are opaque `Bytes` columns using the legacy encoders (not searchable, not indexed), which keeps sizes comparable.
  - **S2b:** container v3 writes/reads the schema block and per-table row blocks from a `Store`; patch/verify/compile move onto it.
  - **S2c:** delete the legacy `Dataset`/record structs outside `coqc-mtg`, update docs.
- **S3 Generic index and query engine** (coqs v2) with dialect hooks; MTG dialect reproduces today's results exactly.
- **S4 Overlays, manifest v2, sync and TS API generalised.**
- **S5 Proof: a second, non-MTG schema** (a small synthetic dataset) compiled, searched, patched, published offline and synced
  end to end.

## Acceptance
MTG golden queries identical before and after; `coqc verify` 0 mismatches; file sizes within +5%, search latency within +10% of the
numbers above; the second schema passes compile, patch, index delta and the simulated-client test with no Magic code involved.

## Decisions (approved 2026-10-28)
1. **Self-describing schema in the file** plus compiled dialects (not fully compiled-in schemas).
2. **Schema-driven query language**: fields and operators come from the schema; Scryfall syntax lives in the MTG dialect only.
3. **No data release before S4**, i.e. until the compendium is in a solid state; clients never see the old formats.
4. **Three extensions only**: `.coqd` data (any table dataset, including price overlays), `.coqi` index (was `.coqs`), `.coqp` patch
   (data, index and overlay patches alike). The file header carries the kind (data/index, base/overlay, patch target) and the manifest
   carries the role, so no per-kind extensions. `.coqr`, `.coqx`, `.coqsd` are retired at the S2-S4 format bumps; file names keep a
   prefix (`cards-`, `prices-`) to tell them apart. Open point for S2: an overlay is a dataset whose rows are keyed by a parent table
   key and replaced wholesale; confirm it needs no container feature beyond that.

## Risks
Generic columns may cost speed and size against today's tuned structs: measure at every stage and keep a fast path for hot fields.
Scope creep into a general database: the non-goal is joins, writes and SQL; this stays a read-mostly, patchable search dataset.
