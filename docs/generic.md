# The generic dialect: a dataset is a schema plus JSONL (S5)

`coqc-generic` serves **any** dataset whose schema id is not a dedicated dialect's id (`mtg`). Nothing in it names Magic: fields,
types and operators come from the schema, ingest reads JSONL, and the container, patches, index, overlays, manifest and sync are
the same code the Magic data uses. S5 of `docs/schema-design.md` is this crate plus the proof below.

## Authoring a dataset
1. **Schema** (`schema.json`, the `Schema` serde shape; `demo::SCHEMA` is a full example). Tables, typed columns, dictionaries.
   `rel`: `"None"` (root) or `{"Parent": "work"}` (the table queries run over; the last-declared child table with a parent is the
   *primary* table). `ty`: `Str Int Float Bool Uuid {"Enum": dict} {"Set": dict} {"Ref": table} Bytes Json`. `index`: `None Exact Text Range Set`.
   Every table needs a `key` column (type `Uuid`).
2. **JSONL**, one file per table, a line per row, keys = column names. The key value is any unique string: a UUID is used as is,
   anything else is hashed ([`key_bytes`]). A `Ref(parent)` column holds the parent's key string.
3. `coqc generic-compile schema.json --table work=works.jsonl --table edition=editions.jsonl -o books.coqd [--prev old.coqd] [--rebase] [--source-date YYYYMMDD]`.
   With `--prev` rows keep their ids: a known key replaces its row (revived if it was removed), a new key is appended, a key
   missing from the input is **tombstoned** (row and key stay). Dictionaries are append-only, so ids are stable and `.coqp` patches
   and `.coqid` index deltas stay small (day 2 of the demo: patch 6% of the file). A schema change needs `--rebase` (new epoch).
4. Everything else is the usual tooling: `index-build`, `index-diff`, `patch`, `apply`, `search`, `manifest`, `data-publish.sh`.
   The dialect is picked from the file's schema id (`coqc::dialect_for`, `coqe::dialect_for`).

Not supported by ingest (yet): sidecar tables, columnar tables (those are overlays), `Bytes` columns.

## Overlays
`coqc generic-overlay overlay-schema.json base.coqd prices.jsonl -o prices.coqd`: one **columnar** table of `Int` columns; row i
follows row i of the base's primary table; a line is `{"key": <primary key>, <col>: <int>, ...}`; missing = no value. The overlay
carries the base's epoch (the engine refuses another epoch) and patches with ordinary `.coqp` (column-delta mode).

## Query language (schema-driven)
```
term      := [-] ( field OP value | word | "phrase" | ( expr ) )      terms AND together; `or` between terms
OP        := :  =  !=  <  <=  >  >=
directive := order:field  order:-field  dir:desc  unique:<parent table>  prefer:field  prefer:-field      has:field
```
| column | operators |
|---|---|
| Str, index Text | `:` folded substring, `=` / `!=` whole value (folded) |
| Str, other index | `:` / `=` / `!=` exact |
| Int, Float | all, numeric |
| Bool | `:true` / `:false` |
| Enum | `:name` (case-insensitive) / `!=` |
| Set (index Set) | `:a,b` has all, `=` exactly, `!=`, `<=` within, `>=` has all |
| overlay Int/Float | all, numeric: `price<1000` or `prices.price<1000` (alias or name; needs the overlay attached) |

A bare word searches every Text column of the primary table and its parent. Fields resolve by name or alias, primary table
first (`table.column` to qualify). `has:field` tests presence (Set: non-empty). `unique:work prefer:year` keeps one edition per
work, the one with the highest `year`. Columns with `index: None` are not searchable (except `has:`). Unknown fields list the
valid ones. The engine takes the same strings (`search {query}`), plus `scope`, `limit`, `offset` as for Magic.

## Record JSON
A primary row is an object of all its columns by name (Uuid as hyphenated string, Enum as its string, Set as an array of
strings, Json as the JSON value, Bytes omitted); a `Ref` column becomes the nested parent record. `getByKey {id}` takes the key string.

## Limits found by the S5 proof (kept on purpose, documented)
- **Set columns are mask-indexed: at most 32 dictionary values** (`index_spec` fails above that, naming the column). Unlimited
  sets need posting lists in the generic index; not needed yet.
- Overlay predicates are numeric only; an overlay is always row-aligned with the primary table.
- Query results are in row order unless `order:` is given; there is no relevance ranking.
- Found and fixed while building it: presence tests on `Json` columns (`has:extra`) did not work in the generic index.

## The proof (what is tested)
- `crates/generic/tests/books.rs`: determinism, round trip, ~20 queries compared with an independent evaluation of the JSONL,
  order/unique/prefer, error messages, day-2 stability (row ids, append-only dictionaries, tombstones), patch and index-delta
  replay byte-identical, overlay compile/patch/predicates, malformed input.
- `crates/engine/tests/generic_books.rs`: the same dataset through the engine's JSON API (open, search, getByKey, attachOverlay,
  applyPatch, applyIndexDelta).
- `scripts/test-generic-pipeline.sh`: three days through compile, patches, manifest (`books` / `books-index` / `prices`),
  `data-publish.sh` against mock-gh and a simulated client walking the chains with every hash checked.
- `modules/.../sync.test.ts`: the TS sync resolves a manifest whose base is `books` (roles decide, ids do not).
