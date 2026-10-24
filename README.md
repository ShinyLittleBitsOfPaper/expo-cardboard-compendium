# expo-scryfall-engine (working name)

Compiles Scryfall bulk card data into a compact, block-compressed, **lossless** dataset
(`.coqd`) that a native engine can query on-device. Phase 0 here: the compiler and format.
No search index, price dataset, patching, reader/engine or Expo module yet (see
`docs/phase0.md`). Unofficial; see `NOTICE.md`.

## Quick start

Needs Rust 1.85+ (`rustup`), and `curl` for the fetch script.

```sh
cargo test                                   # unit + pipeline tests (fixtures included)
cargo build --release

scripts/fetch-bulk.sh data                   # real default_cards -> data/default-cards.json[l]
./target/release/coqc compile data/default-cards.jsonl -o data/cards.coqd --source-date 20261022
./target/release/coqc verify  data/cards.coqd data/default-cards.jsonl   # must print 0 mismatches
./target/release/coqc inspect data/cards.coqd

# next day: stable row ids come from the previous file
./target/release/coqc compile data/new.jsonl -o data/cards2.coqd --prev data/cards.coqd
./target/release/coqc diff data/cards.coqd data/cards2.coqd
```

Add `--progress` for progress lines on stderr and `--breakdown` for an estimate of how much each
residual key contributes to the stored size (slower; recompresses per key).

No network? `coqc synth 100000 data/synth.jsonl [--day N]` writes Scryfall-shaped fake data
(~350 MB) for timing and churn experiments. Its numbers are indicative only.

## Daily pipeline (and patches)

```sh
scripts/daily.sh                 # fetch -> compile on top of the latest release -> patch -> verify the patch
```

Keeps `data/daily/releases/cards-YYYYMMDDHHMM.coqd` and `patches/<old>-to-<new>.coqp`. By hand:
`coqc patch old.coqd new.coqd -o p.coqp`, `coqc apply old.coqd p.coqp -o new.coqd` (verifies SHA-256).

## Layout

- `crates/format`: `coqc-format` (records, container, card JSON reconstruction)
- `crates/compiler`: `coqc` (library + CLI)
- `crates/format/src/search.rs`: derived search index + query plan (docs/search.md)
- `scripts/fetch-bulk.sh`: bulk download with Scryfall-friendly headers; skips when updated_at is unchanged
- `docs/format.md`: file format spec; `docs/phase0.md`: findings, open questions, next steps

## Guarantees (tested)

- **Lossless**: every key Scryfall sends survives, including unknown future keys. Typed
  fields are the ones an engine will index; everything else is kept as residual JSON.
  `coqc compile` aborts if any card fails to rebuild; `coqc verify` re-checks a finished file.
  `null` and absent keys are treated as equal. `prices` is dropped by default
  (`--keep-prices` to keep it).
- **Deterministic**: same input and flags give byte-identical output (pinned zstd, sorted keys).
- **Stable row ids**: with `--prev`, existing cards keep their rows, new ones are appended,
  removed ones become tombstones. `--rebase` starts a new epoch.

## Exchanging bundles with Claude

```sh
scripts/package-bundle.sh                          # -> bundles/expo-scryfall-engine-<ver>-<N>.tar.gz
scripts/ingest-bundle.sh ~/Downloads/<bundle>.tar.gz [--prune]
```

`package-bundle.sh` packs tracked plus untracked-but-not-ignored files (never `target/`, `data/`,
`bundles/`) and bumps the package number in `.bundle-seq`. `ingest-bundle.sh` refuses to run with
uncommitted changes, copies the bundle over the tree, lists tracked files the bundle dropped
(`--prune` deletes them) and leaves your ignored and untracked files alone. Commit before ingesting
so the bundle's changes show up as one `git diff`. Needs GNU tar.
