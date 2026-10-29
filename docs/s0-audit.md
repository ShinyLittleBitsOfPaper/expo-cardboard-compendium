# S0: behaviour lock and audit of docs/schema-design.md (2026-10-28)

## Golden tests (new)
`crates/compiler/tests/golden.rs` runs 60 queries x {unique cards, unique prints} over `tests/fixtures/sample_a.jsonl` and compares
names + printing rows against `tests/golden/queries.txt`; it also checks the `.coqd` size is within +-5% of `tests/golden/size.txt`
(2613 B on the fixture). Regenerate deliberately with `UPDATE_GOLDEN=1 cargo test -p coqc --test golden`. The fixture is tiny
(14 rows), so this locks semantics, not scale. Still to do for S0: golden queries and timings against the real
`cards-<stamp>.coqd` (run by the user; needs a script that prints result counts + hash per query, plus open/search timings on the
emulator). Baseline: 45 tests pass (44 + golden).

## Audit of the design doc's claims (verified against the code)
- Confirmed MTG-shaped: `container.rs` has a fixed `SectionId` (Meta, Oracle, Printing, OracleUuids, PrintingUuids, Ranks) and a
  `Dataset { oracle, ranks, printings }`; `patch.rs` diffs exactly those two record vectors; `search/coqs.rs` hard-codes
  oracle/printing columns (`oracle_cols`, `printing_cols`, `p_oracle`, set/cn/rarity); `prices.rs` is six fixed price columns keyed
  by printing row; `compiler/manifest.rs` has `cards`/`prices` fields; the engine keys lookups on `scryfall_id`.
- Confirmed generic: `codec.rs`, `engine/files.rs`, ffi dispatch.
- **Doc claim wrong:** the doc says the generic query language handles "regex". There is no regex support anywhere (no regex crate,
  `/.../` is parsed as plain text and matches nothing, e.g. `name:/^l/`, `o:/draw (a|two)/`). Regex is a new feature, not existing
  behaviour; it is excluded from the goldens and belongs in S3 (needs a trigram-prefilter + verify design).
- Parser gaps vs Scryfall (all currently "unsupported keyword" errors, not locked in the goldens): `is:`, `pow`/`tou`/`loy`, `art:`,
  `f:`/`legal:`, `order:mv`, `prefer:auto`, `prefer:special`. The supported keyword set is: c/color, ci/id, cmc/mv/manavalue, cn/number,
  e/s/set/edition, name, o/oracle/fo, r/rarity, t/type, plus `order`, `direction`, `unique`, `prefer`. The MTG dialect in S1/S3 must
  keep exactly these, and `is:`/`f:` are the likely first additions after the generalisation.
- Not locked: `limdul vault` finds nothing for "Lim-Dul's Vault" (hyphen is not treated as a word break); Scryfall's behaviour here
  should be checked before treating this as a bug.

## Real-data baseline (user's machine, 0.0.2-11, 2026-10-27 data; `docs/baseline-real-data-0.0.2-11.txt`)
Match counts (the goldens for the MTG dialect; S2/S3 must reproduce them exactly): `bolt` 41, `t:creature` 20328, `c:rg` 704,
`o:flying` 4793, `mv>=5` 7559, `r:mythic` 2965, `s:lea` 290, `-t:land` 37848, `t:legendary t:creature c>=gu` 345.
Timings (desktop CLI; "index" = index build/load at CLI start, "query" = the search itself): index ~622-643 ms, queries 0.22-23.2 ms
(slowest `-t:land` 23 ms, `t:creature` 13.6 ms); whole process ~1.13-1.16 s. Latency budget for S3: query times within +10%.
Not captured: `coqc verify` result and file sizes (`.coqd`, `.coqs`); still wanted as the +-5% size baseline.
