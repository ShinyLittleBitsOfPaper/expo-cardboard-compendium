# Search (derived index + query plan)

Code: `crates/format/src/search.rs` (shared by compiler CLI and the future Rust engine).
CLI: `coqc search <file> <query> [--plan JSON] [--cards] [--limit N] [--offset N]`, `coqc index-stats <file>`.
Parser: `crates/mtg/src/parse.rs` (`parse(&str) -> Query`, Scryfall syntax). `crates/mtg/src/quick.rs` is a thin wrapper (drops sort/unique) used by the CLI.

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
Parser supports: bare words, `"phrases"`, `!"exact"`, `-` negation, `( )`, `or`/`and`; keys `name t/type o/oracle/fo c/color id/identity/ci
mv/manavalue/cmc r/rarity (full names or c/u/r/m) s/set/e/edition cn/number`; color names (guilds, shards, colleges, wedges,
four-color), `c`/`colorless`, `m`/`multicolor`, numeric counts; `mv:even|odd`; `cn` numeric ranges; directives `order:`
(name released cmc rarity), `direction:`, `unique:` (cards prints), `include:extras` (reveals hidden extras). Unknown or
unsupported keywords and `~` in oracle text are errors, as on Scryfall. Default direction (no `direction:`): released desc,
others asc (rarity default unverified).
Defaults (as Scryfall): `unique:cards`, `prefer:newest`, alphabetical order. `prefer:oldest` supported; others (usd-low, promo, ...) TODO.
Name matching ignores `'`, `’` and `,` (confirmed: "krrik son" finds "K'rrik, Son of Yawgmoth"); other punctuation unverified.
`o:` ignores reminder text (parenthesised spans), `fo:` includes it. `~` = the card's own name (full name, a face name, or the
part before the first comma) or generic "this land"/"this creature"/... (verified: Scryfall's `o:"~ enters tapped"` returns 762 cards, mostly "This land enters tapped"). The generic-word list is a fixed approximation.
Parser TODO: `~` in oracle text, `m:`/`mana:`, `is:`/`has:`/`f:`/`in:`/`b:`/`st:`/`lang:`, price keys, `a:`/`ft:`/`wm:`,
`unique:art`, `prefer:`, regex, `include:extras` + default hiding of funny/vanguard/memorabilia, `keyword:`, `pow/tou/loy`.

## Compile-time index (`.coqi`, implemented in -23; real-data numbers pending)
Code: `crates/format/src/search/coqi.rs`. The compiler serializes `SearchIndex` next to the dataset; the header holds the
SHA-256 of the .coqd it was built from. Sections are raw, 8-byte aligned fixed-width arrays (mappable): meta, six oracle text
arenas (names, names_n, names_s, types, texts, full_texts), oracle columns, printing columns, set, cn, name/text trigram tables
(keys, offsets, stop-grams, postings). Distribution: zstd the file (or per section).
- `coqc index-build x.coqd -o x.coqi` prints per-section raw/zstd sizes, whole-file zstd, reload time, a byte-identical round-trip check and sample-query agreement.
- Delta: `coqc index-diff old new [-o d.coqid]` (inputs .coqd or .coqi). Row tables carry changed/appended rows; trigram tables carry
  stop-gram add/remove and per-gram `append` (new list extends the old bytes), `replace`, `new`, `remove`. `index-apply` replays it;
  the result must hash to the target. Run on two consecutive releases (e.g. `scripts/daily.sh --force` ~12 h apart) for the real delta.
- Open: a mapped (zero-copy) reader; today `deserialize` builds an owned `SearchIndex`. Arenas dominate size (texts and full_texts
  duplicate each other); if the download matters, derive `full_texts` from `texts` + reminder spans instead of storing both.

Default hiding (Scryfall): layout vanguard/planar/scheme/phenomenon/art_series, set_type memorabilia and promo_types `playtest` (cmb1/cmb2/unk) are hidden; other funny sets (ugl, unh, unf, ...) are visible, verified against Scryfall unless `include:extras` or the query contains a `t:` or `s:`/`e:` term (anywhere, approximation). Tokens/emblems not hidden yet (unverified).
