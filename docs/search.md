# Search (derived index + query plan)

Code: `crates/format/src/search.rs` (shared by compiler CLI and the future Rust engine).
CLI: `coqc search <file> <query> [--plan JSON] [--cards] [--limit N] [--offset N]`, `coqc index-stats <file>`.
`crates/compiler/src/quick.rs` is a throwaway parser for the CLI/tests only; the app's searchParser must emit the plan JSON.

## Decisions
- **The index is derived, never stored or patched.** Built from the records at load (and after a patch);
  deterministic, so the file and patches stay small and there is one code path. Revisit only if
  `index-stats` build time on a real phone is too slow (then cache the serialized form beside the file).
- Everything is evaluated as **bitsets over printing rows**. Oracle predicates compute oracle matches and
  expand to printings; `not` is relative to live (non-tombstone) printings.
- Text is **folded** (lowercase, common Latin accents stripped, `æ/œ/ß` expanded, curly quotes/dashes
  normalised) on both sides. Name, type line and oracle text cover all faces.
- **Trigrams** (byte 3-grams, delta-varint postings) for names and oracle text. Type line scans (short).
  Query: intersect postings smallest-first, then **verify** by substring on the folded text.
  Needles under 3 bytes, or made only of stop-grams, scan all live rows.
- **Size gate:** a gram in more than max(25% of rows, 8) rows is a stop-gram: not stored, never used to
  narrow (`STOP_FRACTION`, `STOP_MIN`). Results are identical with any gate (tested with everything gated).
- Colors/identity: WUBRG mask. Typed value, else `extra` (non-canonical order lives there), else (colors)
  union of face colors, else colorless. Scryfall semantics: `:` and `>=` include; `=` exact; `<=` subset
  (colorless matches); `<`/`>` strict; `!=`.
- **Identity `:` = subset** (`id:wu` == `id<=wu`, colorless fits); `color` `:` stays superset (Scryfall). Parser expands named combos (`azorius`, `bant`...) to letters.
- Mana value is `f64` (half mana works): `mv=0.5`. Rarity rank (Scryfall's list order): `common<uncommon<rare<special<mythic<bonus`.
- Sort: default released desc, name, row. Fields: released, name, mv, rarity (each asc/desc). `unique:
  cards` keeps the first printing of each oracle card in sort order. `offset`/`limit` page the sorted rows.
  (Full sort of every match is ~30 ms for 100k hits on a laptop; use partial selection if the engine needs it.)

## Plan JSON
```json
{"expr":{"and":[{"name":"bolt"},{"color":{"op":":","colors":"r"}},{"mv":{"op":"<=","value":2}},
  {"not":{"type":"land"}}]},
 "sort":[{"field":"released","desc":true},{"field":"name"}],"unique":"cards","offset":0,"limit":60}
```
Leaves: `name`, `oracle`, `type` (substring), `nameexact` (`!"name"`: full name or one face), `color`/`identity` `{op,colors}` (letters `wubrg`, empty/`c` =
colorless), `mv` `{op,value}`, `colorcount`/`identitycount` `{op,value}` (`c>=2`, `id=3`), `rarity` `{op,value}`, `set` (code), `cn` (collector number, exact).
Ops: `: = != < <= > >=`. Combinators: `and`, `or`, `not`. `expr` omitted = all printings.

## Not done yet
`cn` ranges, `lang`, `finish`, legality (`f:`), `is:`/`o:~`, regex, price predicates, set-scoped queries
(`registerSet`), cursors. Real-data sizes/timings: run `coqc index-stats` on the real file (synthetic text
is not representative).

## Scope
Goal: mirror Scryfall's search syntax (https://scryfall.com/docs/syntax) in a standalone engine; where any app's
conventions differ, Scryfall wins. App-specific features (tags, collection state) are the host app's: it resolves
them to ids and passes a scoped set (`registerSet`). Parser moves into Rust (`parse(&str) -> Query`) taking the
raw Scryfall query string; plan JSON stays as IR/debug form.
Parser TODO: named color sets (guilds, shards, colleges, wedges, four-color), `c:m`/`multicolor` (= count>=2),
`colorless`, `-` negation, `or`/parentheses, quoted phrases, `!"exact"`, `~` in oracle text, `fo:`, `m:`/`mana:`,
`mv:even|odd`, `cn` ranges, `e:`/`b:`/`in:`/`st:`, `is:`/`has:`, `include:extras` (funny/vanguard/memorabilia hidden
by default), `unique:`/`order:`/`direction:`/`prefer:`. Unverified: whether Scryfall ignores `'`/`,` in names.
