import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { parseManifest, resolve, type Dataset, type Manifest } from '../manifest';
import { planSync, planBytes, MODULE_FORMAT, type CardsPlan, type LocalState } from '../plan';
import { runSync, dataPaths, openLocal, attachLocalOverlays, type SyncDeps, type SyncProgress } from '../run';

const sha = (b: Buffer) => createHash('sha256').update(b).digest('hex');
const DIR = '/data/cc';
const P = dataPaths(DIR);

// ---- a fake world: published releases, a device filesystem, a fake engine -----------------------------------------
// "datasets" are plain bytes; a "patch" is `PATCH:<day>` (the real format is tested in Rust); an index is
// `IDX:` + dataset; zst is `ZST!` + bytes.
const dataset = (day: number, pad = 0) => Buffer.from(`dataset-day-${day}-` + 'x'.repeat(pad));
const SERVER = 'https://example.test/d';
const PAD = 'x'.repeat(60); // overlay files are bigger than a patch, so the planner prefers patches

interface Release {
  manifest: Manifest;
  files: Map<string, Buffer>;
}
function asset(files: Map<string, Buffer>, tag: string, name: string, body: Buffer) {
  const url = `${SERVER}/${tag}/${name}`;
  files.set(url, body);
  return { name, url, size: body.length, sha256: sha(body) };
}
/** Release for `day`, with patch links from `chainFrom` days (each carrying an index delta unless noDelta). */
function release(day: number, o: { chainFrom?: number[]; epoch?: number; pad?: number; noDelta?: boolean; minCoqd?: number; corrupt?: string; extraOverlay?: boolean; overlayChain?: number[] } = {}): Release {
  const files = new Map<string, Buffer>();
  const d = dataset(day, o.pad ?? 5000);
  const idxRaw = Buffer.concat([Buffer.from('IDX:'), d]);
  const link = (id: string, ext: string, from: number, body: string) => ({
    from: `S${from}`, to: `S${from + 1}`, at: `2026-10-0${from + 1}T22:00:00Z`,
    patch: asset(files, `data-r${from + 1}`, `${id}-S${from}-to-S${from + 1}.${ext}`, Buffer.from(body)),
  });
  const chain = o.chainFrom ?? [];
  const overlay = (id: string, stamp: string): Dataset => ({
    id, role: 'overlay', of: 'cards', stamp, format: 3, min_client_format: 3,
    file: asset(files, 'data-latest', `${id}-${stamp}.coqd`, Buffer.from(`${id}-${day}${PAD}`)), raw: null,
    patches: (o.overlayChain ?? []).map((from) => link(id, 'coqp', from, `OPATCH:${from + 1}`)).map((l) => ({ ...l, from: l.from.replace('S', stamp[0]), to: l.to.replace('S', stamp[0]) })),
  });
  const datasets: Dataset[] = [
    {
      id: 'cards', role: 'base', of: null, stamp: `S${day}`, format: 3, min_client_format: o.minCoqd ?? 3,
      file: asset(files, 'data-latest', `cards-S${day}.coqd`, d), raw: null,
      patches: chain.map((from) => link('cards', 'coqp', from, `PATCH:${from + 1}`)),
    },
    {
      id: 'cards-index', role: 'index', of: 'cards', stamp: `S${day}`, format: 2, min_client_format: 2,
      file: asset(files, 'data-latest', `cards-S${day}.coqi.zst`, Buffer.concat([Buffer.from('ZST!'), idxRaw])),
      raw: { encoding: 'zstd', size: idxRaw.length, sha256: sha(idxRaw) },
      patches: o.noDelta ? [] : chain.map((from) => link('cards', 'coqid', from, `IDXD:${from + 1}`)),
    },
    overlay('prices', `P${day}`),
  ];
  if (o.extraOverlay) datasets.push(overlay('rulings', `R${day}`));
  const manifest: Manifest = {
    manifest_version: 2, generated_at: `2026-10-0${day}T22:00:00Z`, run: `data-r${day}`, epoch: o.epoch ?? 1, rebase: chain.length === 0, datasets,
  };
  if (o.corrupt) files.set(o.corrupt, Buffer.from('garbage!'.repeat(10)));
  return { manifest, files };
}

function world(rel: Release) {
  const fs = new Map<string, Buffer>();
  const log: string[] = [];
  let open: { sha: string } | null = null;
  const text = new Map<string, string>();
  const deps: SyncDeps = {
    async fetchJson(url) {
      log.push(`GET ${url}`);
      return JSON.parse(JSON.stringify(rel.manifest));
    },
    async download(url, dest, onBytes) {
      log.push(`DL ${url.split('/').pop()}`);
      const b = rel.files.get(url);
      if (!b) throw new Error(`404 ${url}`);
      for (const q of [0.25, 0.5, 0.75, 1]) onBytes?.(Math.floor(b.length * q));
      fs.set(dest, b);
    },
    async readText(p) {
      return text.get(p) ?? null;
    },
    async writeText(p, t) {
      text.set(p, t);
    },
    async mkdirs() {},
    engine: {
      async open(coqd) {
        const b = fs.get(coqd);
        if (!b) throw new Error('no dataset');
        open = { sha: sha(b) };
        return { datasetSha256: open.sha, index: 'coqi' };
      },
      async info() {
        if (!open) throw new Error('engine not open');
        return { datasetSha256: open.sha, index: 'coqi' };
      },
      async applyPatch(p) {
        const b = fs.get(p)!;
        if (!b.subarray(0, 6).equals(Buffer.from('PATCH:')) || !open) throw new Error('bad patch');
        const nd = dataset(Number(b.subarray(6).toString()), 5000);
        fs.set(P.coqd, nd);
        open = { sha: sha(nd) };
        log.push('applyPatch');
        return { datasetSha256: open.sha, index: 'stale' };
      },
      async applyIndexDelta() {
        log.push('applyIndexDelta');
        return { datasetSha256: open!.sha, index: 'delta' };
      },
      async sha256File(p) {
        const b = fs.get(p);
        if (!b) throw new Error(`missing ${p}`);
        return { sha256: sha(b), size: b.length };
      },
      async unzstFile(p, out) {
        const b = fs.get(p)!;
        if (!b.subarray(0, 4).equals(Buffer.from('ZST!'))) throw new Error('not zstd');
        const raw = b.subarray(4);
        fs.set(out, Buffer.from(raw));
        return { sha256: sha(raw), size: raw.length };
      },
      async patchFile(path, patch, out) {
        const base = fs.get(path);
        const pb = fs.get(patch);
        if (pb && base && pb.subarray(0, 6).equals(Buffer.from('PATCH:'))) {
          // fake dataset patch: `PATCH:<n>` turns the file into dataset(n); the open engine is untouched (no engine lock)
          const nd = dataset(Number(pb.subarray(6).toString()), 5000);
          fs.set(out, nd);
          log.push('patchFile cards');
          return { sha256: sha(nd), size: nd.length };
        }
        if (!base || !pb || !pb.subarray(0, 6).equals(Buffer.from('OPATCH'))) throw new Error('bad overlay patch');
        // fake overlay patch: `OPATCH:<n>` turns the file into `<id>-<n>`
        const id = out.split('/').pop()!.split('.')[0];
        const nb = Buffer.from(`${id}-${pb.subarray(7).toString()}${PAD}`);
        fs.set(out, nb);
        log.push(`patchFile ${id}`);
        return { sha256: sha(nb), size: nb.length };
      },
      async attachOverlay(id, path) {
        if (!fs.has(path)) throw new Error('no overlay file');
        log.push(`attach ${id}`);
        return null;
      },
      async moveFile(from, to) {
        const b = fs.get(from) ?? (text.has(from) ? Buffer.from(text.get(from)!) : undefined);
        if (!b) throw new Error(`move: missing ${from}`);
        if (text.has(from)) {
          text.set(to, text.get(from)!);
          text.delete(from);
        } else {
          fs.set(to, b);
          fs.delete(from);
        }
      },
      async removeFile(p) {
        fs.delete(p);
      },
    },
  };
  return { deps, fs, text, log, setOpen: (b: Buffer) => (open = { sha: sha(b) }), setRelease: (r: Release) => (rel = r) };
}

const state = (day: number, o: Partial<LocalState> = {}): LocalState => ({ epoch: 1, cardsStamp: `S${day}`, cardsSha256: sha(dataset(day, 5000)), overlays: { prices: `P${day}` }, ...o });
const noTemp = (fs: Map<string, Buffer>) => [...fs.keys()].filter((k) => k.endsWith('.part') || k.endsWith('.tmp'));

// ---- planner ---------------------------------------------------------------------------------------------------
test('plan: first install is a full download', () => {
  const m = release(3, { chainFrom: [1, 2] }).manifest;
  const p = planSync(m, null);
  assert.deepEqual(p.status === 'ok' && [p.cards.kind, p.overlays.prices.kind], ['full', 'full']);
  const r = resolve(m);
  assert.equal(planBytes(m, p), r.cards.file.size + r.index.file.size + r.overlays[0].file.size);
});

test('plan: up to date, patch chain, broken chain, epoch, hash repair, cost', () => {
  const m = release(3, { chainFrom: [1, 2] }).manifest;
  const cards = (l: LocalState | null, mm = m): CardsPlan => {
    const p = planSync(mm, l);
    if (p.status !== 'ok') throw new Error('unexpected needs_app_update');
    return p.cards;
  };
  assert.deepEqual(cards(state(3)), { kind: 'up_to_date' });
  const c1 = cards(state(1));
  assert.ok(c1.kind === 'patch' && c1.steps.map((s) => s.patch.to).join() === 'S2,S3');
  const c2 = cards(state(2));
  assert.ok(c2.kind === 'patch' && c2.steps.length === 1);
  assert.deepEqual(cards(state(0)), { kind: 'full', reason: 'chain_broken' });
  assert.deepEqual(cards(state(1, { epoch: 0 })), { kind: 'full', reason: 'epoch_changed' });
  assert.deepEqual(cards(state(3, { cardsSha256: 'f'.repeat(64) })), { kind: 'full', reason: 'hash_mismatch' });
  // tiny datasets: the chain costs more than the full files
  const heavy = release(3, { chainFrom: [1, 2] }).manifest;
  const hr = resolve(heavy);
  hr.cards.patches[0].patch.size = hr.cards.file.size + hr.index.file.size;
  assert.deepEqual(cards(state(1), heavy), { kind: 'full', reason: 'patches_not_smaller' });
  // a loop in the chain must not hang
  const loop = release(3, { chainFrom: [1, 2] }).manifest;
  const lc = resolve(loop).cards;
  lc.patches[1] = { ...lc.patches[1], to: 'S1' };
  assert.deepEqual(cards(state(1), loop), { kind: 'full', reason: 'chain_broken' });
});

test('plan: overlays are full downloads unless patching is enabled; format and version gates', () => {
  const m = release(3, { chainFrom: [1, 2] }).manifest;
  const p = planSync(m, state(2));
  assert.ok(p.status === 'ok' && p.overlays.prices.kind === 'full');
  const pp = planSync(m, state(2), MODULE_FORMAT, { patchOverlays: true });
  assert.ok(pp.status === 'ok' && pp.overlays.prices.kind === 'patch' === false);
  assert.equal(planSync(release(3).manifest, state(3)).status, 'ok');
  const gate = planSync(release(3, { minCoqd: 9 }).manifest, state(3), MODULE_FORMAT);
  assert.deepEqual(gate, { status: 'needs_app_update', required: { coqd: 9 } });
  const v3 = { ...release(3).manifest, manifest_version: 3 };
  assert.deepEqual(planSync(v3, state(3)), { status: 'needs_app_update', required: { manifest: 3 } });
  // a second overlay is planned like prices, and a new epoch forces every overlay full
  const two = release(3, { extraOverlay: true }).manifest;
  const tp = planSync(two, state(3, { overlays: { prices: 'P3' } }));
  assert.ok(tp.status === 'ok' && tp.overlays.prices.kind === 'up_to_date' && tp.overlays.rulings.kind === 'full');
  const ep = planSync(two, state(3, { epoch: 0, overlays: { prices: 'P3', rulings: 'R3' } }));
  assert.ok(ep.status === 'ok' && ep.overlays.rulings.kind === 'full' && ep.overlays.prices.kind === 'full');
});

test('parseManifest: accepts a real manifest, names the bad field otherwise', () => {
  const m = release(2, { chainFrom: [1] }).manifest;
  assert.deepEqual(parseManifest(JSON.parse(JSON.stringify(m))), m);
  const j = JSON.parse(JSON.stringify(m));
  j.datasets[0].file.url = 'http://insecure.test/x';
  assert.throws(() => parseManifest(j), /datasets\[0\]\.file\.url/);
  const k = JSON.parse(JSON.stringify(m));
  delete k.datasets[1].raw.sha256;
  assert.throws(() => parseManifest(k), /raw\.sha256/);
  const nb = JSON.parse(JSON.stringify(m));
  nb.datasets.shift();
  assert.throws(() => parseManifest(nb), /no base/);
  assert.throws(() => parseManifest('nope'), /root/);
});

// ---- executor --------------------------------------------------------------------------------------------------
test('sync: first install downloads, verifies, unpacks the index and records state', async () => {
  const rel = release(1);
  const w = world(rel);
  const r = await runSync(w.deps, { dir: DIR, manifestUrl: 'https://m/manifest.json' });
  assert.deepEqual([r.status, r.reason, r.overlays, r.changed], ['full', 'no_local_data', 'updated', true]);
  assert.deepEqual(w.fs.get(P.coqd), dataset(1, 5000));
  assert.deepEqual(w.fs.get(P.coqi), Buffer.concat([Buffer.from('IDX:'), dataset(1, 5000)]));
  assert.deepEqual(w.fs.get(P.overlay('prices')), Buffer.from('prices-1' + PAD));
  assert.deepEqual(JSON.parse(w.text.get(P.state)!), state(1));
  assert.deepEqual(noTemp(w.fs), []);
  assert.ok(![...w.fs.keys()].some((k) => k.endsWith('.zst.part')));
});

test('sync: next day applies the patch chain to the file (no engine lock, no index delta), then is idempotent', async () => {
  const w = world(release(1));
  await runSync(w.deps, { dir: DIR });
  w.setRelease(release(3, { chainFrom: [1, 2] }));
  w.log.length = 0;
  const r = await runSync(w.deps, { dir: DIR });
  assert.deepEqual([r.status, r.patches, r.overlays], ['patched', 2, 'updated']);
  assert.deepEqual(w.log.filter((l) => !l.startsWith('GET')), ['DL cards-S1-to-S2.coqp', 'patchFile cards', 'DL cards-S2-to-S3.coqp', 'patchFile cards', 'DL prices-P3.coqd']);
  assert.deepEqual(w.fs.get(P.coqd), dataset(3, 5000));
  assert.deepEqual(JSON.parse(w.text.get(P.state)!), state(3));
  assert.deepEqual(noTemp(w.fs), []);
  w.log.length = 0;
  const again = await runSync(w.deps, { dir: DIR });
  assert.deepEqual([again.status, again.changed, w.log.length], ['up_to_date', false, 1]);
});

test('sync: a corrupt patch falls back to the full files', async () => {
  const w = world(release(1));
  await runSync(w.deps, { dir: DIR });
  const rel = release(2, { chainFrom: [1] });
  rel.files.set(resolve(rel.manifest).cards.patches[0].patch.url, Buffer.from('tampered'));
  w.setRelease(rel);
  const r = await runSync(w.deps, { dir: DIR });
  assert.deepEqual([r.status, r.reason], ['full', 'fallback']);
  assert.deepEqual(w.fs.get(P.coqd), dataset(2, 5000));
  assert.deepEqual(JSON.parse(w.text.get(P.state)!), state(2));
});

test('sync: a failed index delta is tolerated (index is rebuilt on demand)', async () => {
  const w = world(release(1));
  await runSync(w.deps, { dir: DIR });
  const rel = release(2, { chainFrom: [1] });
  rel.files.delete(resolve(rel.manifest).index.patches[0].patch.url);
  w.setRelease(rel);
  const r = await runSync(w.deps, { dir: DIR });
  assert.deepEqual([r.status, r.patches], ['patched', 1]);
  assert.deepEqual(w.fs.get(P.coqd), dataset(2, 5000));
});

test('sync: a corrupt full download throws and leaves the old data alone', async () => {
  const w = world(release(1));
  await runSync(w.deps, { dir: DIR });
  const before = w.fs.get(P.coqd);
  const rel = release(2, { epoch: 2 });
  rel.files.set(resolve(rel.manifest).cards.file.url, Buffer.from('truncated'));
  w.setRelease(rel);
  await assert.rejects(runSync(w.deps, { dir: DIR }), /failed verification/);
  assert.equal(w.fs.get(P.coqd), before);
  assert.deepEqual(JSON.parse(w.text.get(P.state)!), state(1));
  assert.deepEqual(noTemp(w.fs), []);
});

test('sync: an index that does not unpack to the published hash is rejected', async () => {
  const rel = release(1);
  const ix = resolve(rel.manifest).index;
  const zurl = ix.file.url;
  const bad = Buffer.concat([Buffer.from('ZST!'), Buffer.from('IDX:other')]);
  rel.files.set(zurl, bad);
  ix.file.sha256 = sha(bad);
  ix.file.size = bad.length;
  const w = world(rel);
  await assert.rejects(runSync(w.deps, { dir: DIR }), /decompressed data failed verification/);
  assert.equal(w.fs.has(P.coqd), false);
  assert.deepEqual(noTemp(w.fs), []);
});

test('sync: needs_app_update downloads nothing', async () => {
  const w = world(release(2, { minCoqd: 9 }));
  const r = await runSync(w.deps, { dir: DIR });
  assert.deepEqual([r.status, r.required, r.changed], ['needs_app_update', { coqd: 9 }, false]);
  assert.deepEqual(w.log.filter((l) => l.startsWith('DL')), []);
});

test('sync: epoch change replaces everything with full files', async () => {
  const w = world(release(1));
  await runSync(w.deps, { dir: DIR });
  w.setRelease(release(2, { epoch: 2 }));
  const r = await runSync(w.deps, { dir: DIR });
  assert.deepEqual([r.status, r.reason], ['full', 'epoch_changed']);
  assert.equal(JSON.parse(w.text.get(P.state)!).epoch, 2);
});

test('sync: a newer manifest version is refused before its fields are read', async () => {
  const rel = release(2);
  const w = world(rel);
  rel.manifest = { manifest_version: 3, shape: 'unknown' } as unknown as Manifest;
  const r = await runSync(w.deps, { dir: DIR });
  assert.deepEqual([r.status, r.required, r.changed], ['needs_app_update', { manifest: 3 }, false]);
});

test('sync: a second overlay is downloaded like prices, and a new epoch forgets stale overlays', async () => {
  const w = world(release(1, { extraOverlay: true }));
  const r = await runSync(w.deps, { dir: DIR });
  assert.equal(r.overlays, 'updated');
  assert.deepEqual(w.fs.get(P.overlay('rulings')), Buffer.from('rulings-1' + PAD));
  assert.deepEqual(JSON.parse(w.text.get(P.state)!).overlays, { prices: 'P1', rulings: 'R1' });
  w.setRelease(release(2, { epoch: 2 })); // rulings overlay gone from the manifest
  await runSync(w.deps, { dir: DIR });
  assert.deepEqual(JSON.parse(w.text.get(P.state)!).overlays, { prices: 'P2' });
});

test('sync: overlays are patched on disk without the engine, verified, then idempotent', async () => {
  const w = world(release(1));
  await runSync(w.deps, { dir: DIR });
  w.setRelease(release(3, { chainFrom: [1, 2], overlayChain: [1, 2], pad: 5000 }));
  // make the overlay patches cheaper than the full file so the planner picks them
  w.log.length = 0;
  const r = await runSync(w.deps, { dir: DIR });
  assert.equal(r.overlays, 'updated');
  assert.deepEqual(w.log.filter((l) => l.startsWith('patchFile') && l !== 'patchFile cards'), ['patchFile prices', 'patchFile prices']);
  assert.ok(!w.log.includes('DL prices-P3.coqd'));
  assert.deepEqual(w.fs.get(P.overlay('prices')), Buffer.from('prices-3' + PAD));
  assert.deepEqual(JSON.parse(w.text.get(P.state)!).overlays, { prices: 'P3' });
  assert.deepEqual(noTemp(w.fs), []);
});

test('sync: a bad overlay patch falls back to the full overlay file', async () => {
  const w = world(release(1));
  await runSync(w.deps, { dir: DIR });
  const rel = release(2, { chainFrom: [1], overlayChain: [1] });
  const pa = rel.manifest.datasets[2].patches[0].patch;
  rel.files.set(pa.url, Buffer.from('OPATCH:99')); // applies, but to the wrong content
  pa.sha256 = sha(Buffer.from('OPATCH:99'));
  pa.size = 9;
  w.setRelease(rel);
  await runSync(w.deps, { dir: DIR });
  assert.deepEqual(w.fs.get(P.overlay('prices')), Buffer.from('prices-2' + PAD));
  assert.deepEqual(noTemp(w.fs), []);
});

test('openLocal: opens the dataset and attaches every overlay in state', async () => {
  const w = world(release(1, { extraOverlay: true }));
  await runSync(w.deps, { dir: DIR });
  w.log.length = 0;
  const r = await openLocal(w.deps, DIR);
  assert.deepEqual([r.attached.sort(), r.failed], [['prices', 'rulings'], []]);
  w.fs.delete(P.overlay('rulings'));
  assert.deepEqual((await openLocal(w.deps, DIR)).failed, ['rulings']);
  await assert.rejects(openLocal(world(release(1)).deps, '/nowhere'), /run sync first/);
});

test('a non-MTG dataset (base "books") resolves: ids do not matter, roles do', async () => {
  const r = release(1, { pad: 5000 });
  const rename = (id: string) => id.replace('cards', 'books');
  for (const d of r.manifest.datasets) {
    d.id = rename(d.id);
    if (d.of) d.of = rename(d.of);
  }
  const res = resolve(parseManifest(JSON.parse(JSON.stringify(r.manifest))));
  assert.equal(res.cards.id, 'books');
  assert.equal(res.index.id, 'books-index');
  assert.deepEqual(res.overlays.map((o) => o.id), ['prices']);
  // two bases are ambiguous, a base without an index is refused
  const two = JSON.parse(JSON.stringify(r.manifest)) as Manifest;
  two.datasets.push({ ...two.datasets[0], id: 'other' });
  assert.throws(() => resolve(two), /more than one base/);
  const noIdx = JSON.parse(JSON.stringify(r.manifest)) as Manifest;
  noIdx.datasets = noIdx.datasets.filter((d) => d.role !== 'index');
  assert.throws(() => resolve(noIdx), /no index of "books"/);
});

test('sync: progress reports bytes per file and overall, monotonic, ending at the planned total', async () => {
  const w = world(release(1));
  const ev: SyncProgress[] = [];
  await runSync(w.deps, { dir: DIR, onProgress: (e) => ev.push(e) });
  const dl = ev.filter((e) => e.file);
  assert.ok(dl.length > 0);
  for (const e of dl) {
    assert.ok(e.received! <= e.size! && e.bytesDone! <= e.bytesTotal!);
  }
  for (let i = 1; i < dl.length; i++) assert.ok(dl[i].bytesDone! >= dl[i - 1].bytesDone!);
  const last = dl[dl.length - 1];
  assert.equal(last.bytesDone, last.bytesTotal);
  assert.equal(last.received, last.size);
  // step events (no file) are still emitted as before
  assert.ok(ev.some((e) => !e.file && e.step === 'cards'));
});

test('sync: progress total grows (never shrinks) when a patch falls back to the full files', async () => {
  const w = world(release(1));
  await runSync(w.deps, { dir: DIR });
  const rel = release(2, { chainFrom: [1] });
  rel.files.set(resolve(rel.manifest).cards.patches[0].patch.url, Buffer.from('tampered'));
  w.setRelease(rel);
  const ev: SyncProgress[] = [];
  await runSync(w.deps, { dir: DIR, onProgress: (e) => ev.push(e) });
  const dl = ev.filter((e) => e.file);
  for (let i = 1; i < dl.length; i++) {
    assert.ok(dl[i].bytesDone! >= dl[i - 1].bytesDone!);
    assert.ok(dl[i].bytesTotal! >= dl[i - 1].bytesTotal!);
  }
  assert.ok(dl.every((e) => e.bytesDone! <= e.bytesTotal!));
});

test('attachLocalOverlays: attaches the tracked overlays without opening the dataset', async () => {
  const w = world(release(1, { extraOverlay: true }));
  await runSync(w.deps, { dir: DIR });
  w.log.length = 0;
  const r = await attachLocalOverlays(w.deps, DIR);
  assert.deepEqual([r.attached.sort(), r.failed], [['prices', 'rulings'], []]);
  await assert.rejects(attachLocalOverlays(world(release(1)).deps, '/nowhere'), /run sync first/);
});

test('sync: patch chain also patches the index with the published deltas (no rebuild on open)', async () => {
  const w = world(release(1));
  await runSync(w.deps, { dir: DIR });
  w.setRelease(release(3, { chainFrom: [1, 2] }));
  w.log.length = 0;
  w.deps.engine.patchIndexFile = async (path, delta, out) => {
    const d = w.fs.get(delta)!;
    if (!w.fs.has(path) || !d.subarray(0, 5).equals(Buffer.from('IDXD:'))) throw new Error('bad index delta');
    const nb = Buffer.concat([Buffer.from('IDX:'), dataset(Number(d.subarray(5).toString()), 5000)]);
    w.fs.set(out, nb);
    w.log.push('patchIndexFile');
    return { sha256: sha(nb), size: nb.length };
  };
  const r = await runSync(w.deps, { dir: DIR });
  assert.deepEqual([r.status, r.patches], ['patched', 2]);
  assert.ok(w.log.includes('patchIndexFile'));
  assert.deepEqual(w.fs.get(P.coqi), Buffer.concat([Buffer.from('IDX:'), dataset(3, 5000)]));
  assert.deepEqual(noTemp(w.fs), []);
});

test('sync: a failing index delta leaves the old index (engine rebuilds) and does not fail the sync', async () => {
  const w = world(release(1));
  await runSync(w.deps, { dir: DIR });
  const before = w.fs.get(P.coqi)!;
  w.setRelease(release(3, { chainFrom: [1, 2] }));
  w.deps.engine.patchIndexFile = async () => {
    throw new Error('delta base hash mismatch');
  };
  const r = await runSync(w.deps, { dir: DIR });
  assert.deepEqual([r.status, r.patches, r.fallbackError], ['patched', 2, undefined]);
  assert.deepEqual(w.fs.get(P.coqd), dataset(3, 5000));
  assert.deepEqual(w.fs.get(P.coqi), before);
  assert.deepEqual(noTemp(w.fs), []);
});
