# manifest.json (v2)

Written by `coqc manifest` (`crates/compiler/src/manifest.rs`), published last on the rolling `data-latest` release. Clients hard-code only its URL.

```
{ manifest_version: 2, generated_at, run, epoch, rebase,
  datasets: [ { id, role, of?, stamp, format, min_client_format, file{name,url,size,sha256}, raw?{encoding,size,sha256},
                patches: [ { from, to, at, patch{...} } ] } ] }
```

- `role: base` - a `.coqd` that defines the row-id space; `epoch` (manifest level) changes when row ids are reassigned. Today: `cards`.
- `role: index` - derived `.coqi` of the base named by `of`, shipped zstd (`raw` = decompressed size/hash, which the index deltas refer to). Its stamp equals the base's. Today: `cards-index`.
- `role: overlay` - a `.coqd` whose row i belongs to row i of `of`. Today: `prices`. Add another by passing one more `--overlay`.
- `min_client_format` is per dataset: above the module's reader => "update the app".
- Chains are per dataset and independent. `rebase: true` = no chain leads here (first release, new epoch, base format change); clients download the full files.
  Epoch change restarts every chain. A format change restarts that dataset's chain only. An index whose delta is missing restarts its own chain (full download).
- Links older than `--keep-since` are dropped. A `--prev` manifest of another version is ignored (new chain).
- Patch files are named `<prefix><from>-to-<to>.<ext>` (`cards-A-to-B.coqp`, `cards-A-to-B.coqid`, `prices-A-to-B.coqp`); `prefix` defaults to `<id>-`.

CLI: `coqc manifest --base cards=F[,patch=P] --index cards-index=F.zst,of=cards,prefix=cards-[,patch=D] --overlay prices=F,of=cards[,patch=P] --run TAG --at ISO --keep-since ISO (--repo O/R | --base-url U) [--prev M] [--rebase] -o manifest.json`.
Stdout ends with `latest:` (full files for data-latest) and `run:` (patch files for the run release) lines read by `data-publish.sh`.

Status: Rust side done (S4b, 0.0.2-25); TypeScript sync reads v2 (S4c part 1, 0.0.2-26: `src/sync/`, overlays as full files, `<id>.coqd` on the device, `state.json` has `overlays`). S4c part 2 (0.0.2-27): overlays are patched on disk (`patchFile`, full-file fallback on any failure) and the engine attaches them (`openSynced`).
