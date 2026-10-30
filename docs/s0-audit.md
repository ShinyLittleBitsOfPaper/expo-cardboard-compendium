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

## Real-data baseline (user's machine, 0.0.2-11, 2026-10-28 data; `docs/baseline-real-data-0.0.2-11.txt`)
Match counts (the goldens for the MTG dialect; S2/S3 must reproduce them exactly): `bolt` 41, `t:creature` 20328, `c:rg` 704,
`o:flying` 4793, `mv>=5` 7559, `r:mythic` 2965, `s:lea` 290, `-t:land` 37848, `t:legendary t:creature c>=gu` 345.
Timings (desktop CLI; "index" = index build/load at CLI start, "query" = the search itself): index ~622-643 ms, queries 0.22-23.2 ms
(slowest `-t:land` 23 ms, `t:creature` 13.6 ms); whole process ~1.13-1.16 s. Latency budget for S3: query times within +10%.
Dataset: `cards-202610282106` (2026-10-28 release; the heading above says 10-27 by mistake). `coqc verify` against `default-cards.jsonl`:
119695 cards, 0 mismatches (11.4 s). Sizes: `.coqd` 31,266,744 B, `.coqs` 23,021,752 B (the +-5% size baseline for S2b/S3).

## Real-data section sizes, v2 file (`coqc inspect`, 0.0.2-13, cards-202610282106, 29.82 MB = 31,266,744 B)
meta 0.00 MB, oracle rows 3.40, printing rows 23.27, oracle uuids 0.75, printing uuids 2.28, oracle ranks 0.11. 39,160 oracle rows,
119,698 printing rows (3 tombstones), 7 dictionaries (promo_types 114, formats 23). Compare against the v3 `coqc inspect` after recompiling.
Note: `coqc index-stats` takes the `.coqd`, not the `.coqs`.

## Real-data check of COQD v3 (user's machine, 0.0.2-14, recompiled from default-cards.jsonl, 2026-10-30)
`coqc verify`: 119695 cards, 0 mismatches (21.3 s). `.coqd` 31,293,682 B (v2: 31,266,744 B, +0.09%); `.coqs` 23,028,224 B (v2 23,021,752 B).
Sections stored (MB): oracle 3.44 (v2 3.40), oracle_keys 0.75, oracle_ranks 0.17 (v2 0.11), printing 23.20 (v2 23.27), printing_keys 2.28.
Compile 31.1 s, write 12.9 s; index build 661 ms, reload 26 ms. Not yet run on v3: the 9-query count/timing loop (counts are implied
by the lossless verify + unchanged golden queries, timings still wanted).

## Real-data query baseline on COQD v3 (user's machine, 0.0.2-15, 2026-10-30; `docs/baseline-real-data-0.0.2-15.txt`)
Counts identical to the v2 baseline for every query that ran as intended: `bolt` 41, `t:creature` 20328, `c:rg` 704, `o:flying` 4793,
`mv>=5` 7559, `r:mythic` 2965, `s:lea` 290, `t:legendary t:creature c>=gu` 345. The `-t:land` row (15829) is a paste artifact: the
terminal wrap put a newline inside the quoted query (`-t:lan` + `d`), so it ran as `-t:lan d`; the real `-t:land` golden stays 37848
(re-run on a single line before S3 lands). Timings: index build 672-825 ms (first run 825 ms is cold), queries 0.24-17.8 ms
(`t:creature` 17.8 ms, `-t:lan d` 14.6 ms), process 1.33-1.56 s. Within the +10% budget except the cold first run; v3 changed no index code.
`coqc verify`: 119695 cards, 0 mismatches (21.2 s). `.coqd` 31,293,682 B, `.coqs` 23,028,224 B.

### Correction (0.0.2-17): single-line rerun, same data (`docs/baseline-real-data-0.0.2-15.txt` replaced)
All 9 counts match the v2 baseline, including `-t:land` 37848 (query 24.7 ms). Index build 664-719 ms, queries 0.18-24.7 ms (+-10% of
v2), process 1.26-1.36 s. `coqc verify` 0 mismatches (20.75 s). COQD v3 is signed off; S3a is next.
