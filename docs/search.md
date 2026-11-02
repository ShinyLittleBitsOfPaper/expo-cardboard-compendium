# Search (generic index + MTG dialect)

Code: `crates/format/src/gindex.rs` (generic evaluator over a `Store`), `crates/format/src/search.rs` (folding, bitsets, trigram
postings), `crates/format/src/coqi.rs` (`.coqi` file + deltas), `crates/mtg/src/{parse,query,gindex}.rs` (Scryfall syntax, derived
columns, `Custom` predicates, translation to the generic query). Shared by the compiler CLI and the Rust engine.
CLI: `coqc search <file> <query> [--plan GQUERY_JSON] [--cards] [--limit N] [--offset N]`, `coqc index-stats|index-build|index-diff|index-apply`.
Parser: `crates/mtg/src/parse.rs` (`parse(&str) -> Query`, the Scryfall query tree); `gindex::translate(&Query) -> GQuery`.

## How it works
- **Schema drives the index.** Column `IndexKind` (`Text`, `Exact`, `Set`, `Range`) says what a column gets; the dialect adds *derived*
  columns (folded text, colour masks, numbers, flags) and `Custom` predicates (`IndexSpec`, `Hooks`). The MTG dialect also lists the
  schema `Text` columns it never searches by name (`SKIP_AUTO`), so no folded copy or trigram table is built for them.
- **The index is derived at compile time and shipped** as `.coqi` (below). The engine attaches it to the loaded `Store` with
  `GIndex::with_parts` (cheap: liveness and parent maps only). A missing, foreign (other dataset hash) or mis-sized `.coqi` is rebuilt
  with `GIndex::build_parts` and rewritten.
- Queries are evaluated as **bitsets over primary-table rows** (MTG: printings). Oracle predicates compute oracle matches and lift them
  through the printing's `Ref`; `not` is relative to live (non-tombstone) rows.
- Text is **folded** (lowercase, common Latin accents stripped, `ae/oe/ss` expanded, curly quotes/dashes normalised) on both sides.
  Name, type line and oracle text cover all faces.
- **Trigrams** (byte 3-grams, delta-varint postings) for names and oracle text (`name_n`; `text` + `full_text` share one table). Type line
  scans. Query: intersect postings smallest-first, then **verify** by substring on the folded text. Needles under 3 bytes, or made only of
  stop-grams, scan all live rows. **Size gate:** a gram in more than max(25% of rows, 8) rows is a stop-gram (`STOP_FRACTION`,
  `STOP_MIN`); results are identical with any gate (tested with everything gated).
- Colours/identity: WUBRG mask. Typed value, else `extra` (non-canonical order lives there), else (colours) union of face colours, else
  colourless. Scryfall semantics: `:` and `>=` include; `=` exact; `<=` subset (colourless matches); `<`/`>` strict; `!=`.
  **Identity `:` = subset** (`id:wu` == `id<=wu`); colour `:` stays superset. The parser expands named combos (`azorius`, `bant`, ...).
- Mana value is `f64` (half mana works). Rarity rank: `common<uncommon<rare<special<mythic<bonus` (a derived number today; a schema
  `ordered` flag is future work).
- Sort: default released desc, name, row. Fields: released, name, mv, rarity (asc/desc). `unique:cards` keeps the first printing of each
  oracle card in sort order. `offset`/`limit` page the sorted rows.
- **Regression oracle:** `crates/compiler/tests/golden/gindex-oracle.txt` holds, for ~120 queries x unique x prefer x include-extras on
  four fixture variants plus 4000 synthetic cards, the total and an FNV hash of the ordered rows, recorded from the pre-S3 `SearchIndex`
  before it was removed. `tests/gindex.rs` checks the fresh index and the serialise/reload round trip against it; any diff is a
  behaviour change (regenerate only on purpose).

## Generic query JSON (`GQuery`, `--plan`)
`{"expr": <GExpr>, "sort": [...], "unique_parent": bool, "limit": N, "offset": N, "scope": [rows]}`. `GExpr`: `all`, `none`, `and`, `or`,
`not`, `text {field, needle}` (needle pre-folded), `cmp {field, op, val}` with `val` = `num | mask | str`, `custom {name, args}`.
Ops: `: = != < <= > >=`. A schema column works without dialect help (`text` on a `Text` column, `cmp` on a numeric one).

## `.coqi` (COQI v2) and deltas (`.coqid`, magic `CQID`)
Header: magic, version 2, SHA-256 of the `.coqd` it was built from, section directory. Sections (raw, 8-byte aligned): a meta section
(table, column, kind, trigram id per column), one section per derived/auto column (text arenas, u32 masks, f64 numbers, u8 flags), one per
trigram table (keys, offsets, stop-grams, postings). Distribution: zstd the file.
- `coqc index-build x.coqd -o x.coqi` prints per-section raw/zstd sizes, whole-file zstd, reload time, a byte-identical round-trip check
  and sample-query agreement. `coqc index-stats x.coqd` prints build time, sections and sample-query timings.
- Delta: `coqc index-diff old new [-o d.coqid]` (inputs `.coqd` or `.coqi`). Column sections carry changed/appended rows; trigram tables
  carry stop-gram add/remove and per-gram `append` (new list extends the old bytes), `replace`, `new`, `remove`. `index-apply` replays it;
  the result must hash to the target. A delta only applies between two files with the same layout (same meta section); otherwise ship
  the full index.
- Open: a mapped (zero-copy) reader; today `deserialize` builds owned columns. Arenas dominate size (`text` and `full_text` overlap).

## Scope and parser
Goal: mirror Scryfall's search syntax (https://scryfall.com/docs/syntax) in a standalone engine; where any app's conventions differ,
Scryfall wins. App-specific features (tags, collection state) are the host app's: it resolves them to ids and passes a scoped set
(`registerSet`).
Parser supports: bare words, `"phrases"`, `!"exact"`, `-` negation, `( )`, `or`/`and`; keys `name t/type o/oracle/fo c/color id/identity/ci
mv/manavalue/cmc r/rarity (full names or c/u/r/m) s/set/e/edition cn/number`; colour names (guilds, shards, colleges, wedges, four-colour),
`c`/`colorless`, `m`/`multicolor`, numeric counts; `mv:even|odd`; `cn` numeric ranges; directives `order:` (name released cmc rarity),
`direction:`, `unique:` (cards prints), `include:extras` (reveals hidden extras). Unknown or unsupported keywords are errors, as on
Scryfall. Default direction: released desc, others asc (rarity default unverified). Defaults (as Scryfall): `unique:cards`,
`prefer:newest`, alphabetical order. `prefer:oldest` supported; others (usd-low, promo, ...) TODO.
Name matching ignores `'`, `’` and `,` ("krrik son" finds "K'rrik, Son of Yawgmoth"). `o:` ignores reminder text (parenthesised
spans), `fo:` includes it. `~` = the card's own name (full name, a face name, or the part before the first comma) or generic
"this land"/"this creature"/... (verified: Scryfall's `o:"~ enters tapped"` returns 762 cards); the generic-word list is a fixed
approximation.
S3d additions (0.0.2-23):
- **Regex** `name:/re/`, `o:/re/`, `fo:/re/`, `t:/re/` (spaces allowed inside, `\\/` = slash). Rust `regex` crate, case-insensitive, `^`/`$` per line,
  run on the folded text; size limits (1 MB program, 8 MB DFA) so a hostile pattern errors instead of running. A plain ASCII word/phrase
  pattern takes the trigram path; everything else scans the (oracle) rows. Unverified vs Scryfall: per-line anchors, `o:` regex ignoring reminder text.
- `pow`/`tou`/`loy` (+ long names) with a number or another stat (`pow>tou`, `pow>mv`); any face may match; `*` = 0, `1+*` = 1. Derived `pt_f`.
- `m:`/`mana:` mana cost as a multiset (`:`/`>=` contains, `=` equal, `<=` subset); `2RR` shorthand; any face. Derived `mana_f`.
- `is:`/`not:` (`-is:`): oracle flags permanent spell historic vanilla commander gold hybrid phyrexian reserved; printing flags foil nonfoil
  etched promo reprint digital fullart(full) textless oversized variation spotlight funny split flip transform meld leveler saga adventure
  mdfc dfc token planar scheme vanguard augment host. Derived masks `is_o`/`is_p`. permanent/spell/historic/vanilla/commander/hybrid are
  approximations; unknown values are errors.
- `f:`/`format:`/`legal:` (legal **or restricted**, unverified vs Scryfall; `edh` = commander), `banned:`, `restricted:`; unknown format is a search error.
- `usd` `usdfoil` `usdetched` (TCGplayer) and `eur` `eurfoil` `euretched` (Cardmarket) with `: = != < <= > >=` and an amount (`usd>=1.5`, `eur<0.5`): compare the printing's price in the attached `prices` overlay (the generic index exposes overlays to `Custom` predicates via `GIndex::overlay`). A printing without that price never matches; without the overlay attached the search fails with a clear error. Which provider/finish Scryfall maps to `usd` is not verified against Scryfall.
- `kw:`/`keyword:` (exact, case-insensitive), `a:`/`artist:`, `ft:`/`flavor:` (substring scan of top-level printing fields; face-level artist/flavor not covered;
  `ft:` is the slowest, ~70 ms on 120k rows desktop), `order:mv`.
- Index: four new derived columns (+1.15 MB raw, +0.1 MB zstd on synthetic 120k; old queries unchanged). `Hooks::required` makes an index built by an
  older version fail to attach, so the engine rebuilds it.
Not done: price predicates (`usd>1`; prices are outside the indexed store, wait for the overlay stage S4), `in:`, `has:`, `lang:`, `b:`/`st:`,
`wm:`, `unique:art`, `prefer:` beyond oldest/newest, tagger `is:` (fetchland...).
Default hiding (Scryfall): layout vanguard/planar/scheme/phenomenon/art_series, set_type memorabilia and promo_types `playtest`
(cmb1/cmb2/unk) are hidden; other funny sets are visible, unless `include:extras` or the query contains a `t:` or `s:`/`e:` term
(anywhere, approximation). Tokens/emblems not hidden yet (unverified).
