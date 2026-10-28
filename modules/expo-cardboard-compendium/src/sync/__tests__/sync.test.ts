import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { parseManifest, type Manifest } from '../manifest';
import { planSync, planBytes, MODULE_FORMAT, type CardsPlan, type LocalState } from '../plan';
import { runSync, dataPaths, type SyncDeps } from '../run';

const sha = (b: Buffer) => createHash('sha256').update(b).digest('hex');
const DIR = '/data/cc';
const P = dataPaths(DIR);

// ---- a fake world: published releases, a device filesystem, a fake engine -----------------------------------------
// "datasets" are plain bytes; a "patch" is `PATCH:<day>` (the real format is tested in Rust); an index is
// `IDX:` + dataset; zst is `ZST!` + bytes.
const dataset = (day: number, pad = 0) => Buffer.from(`dataset-day-${day}-` + 'x'.repeat(pad));
const SERVER = 'https://example.test/d';

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
function release(day: number, o: { chainFrom?: number[]; epoch?: number; pad?: number; noDelta?: boolean; minCoqd?: number; corrupt?: string } = {}): Release {
  const files = new Map<string, Buffer>();
  const d = dataset(day, o.pad ?? 5000);
  const idxRaw = Buffer.concat([Buffer.from('IDX:'), d]);
  const patches = (o.chainFrom ?? []).map((from) => {
    return {
      from: `S${from}`, to: `S${from + 1}`, at: `2026-10-0${from + 1}T22:00:00Z`,
      patch: asset(files, `data-r${from + 1}`, `cards-S${from}-to-S${from + 1}.coqp`, Buffer.from(`PATCH:${from + 1}`)),
      index_delta: o.noDelta ? null : asset(files, `data-r${from + 1}`, `cards-S${from}-to-S${from + 1}.coqsd`, Buffer.from(`IDXD:${from + 1}`)),
    };
  });
  const manifest: Manifest = {
    manifest_version: 1, generated_at: `2026-10-0${day}T22:00:00Z`, run: `data-r${day}`, epoch: o.epoch ?? 1, rebase: patches.length === 0,
    min_client_format: { coqd: o.minCoqd ?? 2, coqs: 1, coqr: 1 },
    cards: {
      stamp: `S${day}`, format: 2, min_reader: 2,
      file: asset(files, 'data-latest', `cards-S${day}.coqd`, d),
      index: { ...asset(files, 'data-latest', `cards-S${day}.coqs.zst`, Buffer.concat([Buffer.from('ZST!'), idxRaw])), encoding: 'zstd', raw_size: idxRaw.length, raw_sha256: sha(idxRaw) },
      patches,
    },
    prices: { stamp: `P${day}`, format: 1, min_reader: 1, file: asset(files, 'data-latest', `prices-P${day}.coqr`, Buffer.from(`prices-${day}`)), patches: [] },
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
    async download(url, dest) {
      log.push(`DL ${url.split('/').pop()}`);
      const b = rel.files.get(url);
      if (!b) throw new Error(`404 ${url}`);
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
        return { datasetSha256: open.sha, index: 'coqs' };
      },
      async info() {
        if (!open) throw new Error('engine not open');
        return { datasetSha256: open.sha, index: 'coqs' };
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

const state = (day: number, o: Partial<LocalState> = {}): LocalState => ({ epoch: 1, cardsStamp: `S${day}`, cardsSha256: sha(dataset(day, 5000)), pricesStamp: `P${day}`, ...o });
const noTemp = (fs: Map<string, Buffer>) => [...fs.keys()].filter((k) => k.endsWith('.part') || k.endsWith('.tmp'));

// ---- planner ---------------------------------------------------------------------------------------------------
test('plan: first install is a full download', () => {
  const m = release(3, { chainFrom: [1, 2] }).manifest;
  const p = planSync(m, null);
  assert.deepEqual(p.status === 'ok' && [p.cards.kind, p.prices.kind], ['full', 'full']);
  assert.equal(planBytes(m, p), m.cards.file.size + m.cards.index.size + m.prices.file.size);
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
  assert.ok(c1.kind === 'patch' && c1.steps.map((s) => s.to).join() === 'S2,S3');
  const c2 = cards(state(2));
  assert.ok(c2.kind === 'patch' && c2.steps.length === 1);
  assert.deepEqual(cards(state(0)), { kind: 'full', reason: 'chain_broken' });
  assert.deepEqual(cards(state(1, { epoch: 0 })), { kind: 'full', reason: 'epoch_changed' });
  assert.deepEqual(cards(state(3, { cardsSha256: 'f'.repeat(64) })), { kind: 'full', reason: 'hash_mismatch' });
  // tiny datasets: the chain costs more than the full files
  const heavy = release(3, { chainFrom: [1, 2] }).manifest;
  heavy.cards.patches[0].patch.size = heavy.cards.file.size + heavy.cards.index.size;
  assert.deepEqual(cards(state(1), heavy), { kind: 'full', reason: 'patches_not_smaller' });
  // a loop in the chain must not hang
  const loop = release(3, { chainFrom: [1, 2] }).manifest;
  loop.cards.patches[1] = { ...loop.cards.patches[1], to: 'S1' };
  assert.deepEqual(cards(state(1), loop), { kind: 'full', reason: 'chain_broken' });
});

test('plan: prices are a full download unless patching is enabled; format gates', () => {
  const m = release(3, { chainFrom: [1, 2] }).manifest;
  const p = planSync(m, state(2));
  assert.ok(p.status === 'ok' && p.prices.kind === 'full');
  assert.equal(planSync(release(3).manifest, state(3)).status, 'ok');
  const gate = planSync(release(3, { minCoqd: 3 }).manifest, state(3), MODULE_FORMAT);
  assert.deepEqual(gate, { status: 'needs_app_update', required: { coqd: 3 } });
  const v2 = { ...release(3).manifest, manifest_version: 2 };
  assert.deepEqual(planSync(v2, state(3)), { status: 'needs_app_update', required: { manifest: 2 } });
});

test('parseManifest: accepts a real manifest, names the bad field otherwise', () => {
  const m = release(2, { chainFrom: [1] }).manifest;
  assert.deepEqual(parseManifest(JSON.parse(JSON.stringify(m))), m);
  const j = JSON.parse(JSON.stringify(m));
  j.cards.file.url = 'http://insecure.test/x';
  assert.throws(() => parseManifest(j), /cards\.file\.url/);
  const k = JSON.parse(JSON.stringify(m));
  delete k.cards.index.raw_sha256;
  assert.throws(() => parseManifest(k), /raw_sha256/);
  assert.throws(() => parseManifest('nope'), /root/);
});

// ---- executor --------------------------------------------------------------------------------------------------
test('sync: first install downloads, verifies, unpacks the index and records state', async () => {
  const rel = release(1);
  const w = world(rel);
  const r = await runSync(w.deps, { dir: DIR, manifestUrl: 'https://m/manifest.json' });
  assert.deepEqual([r.status, r.reason, r.prices, r.changed], ['full', 'no_local_data', 'updated', true]);
  assert.deepEqual(w.fs.get(P.coqd), dataset(1, 5000));
  assert.deepEqual(w.fs.get(P.coqs), Buffer.concat([Buffer.from('IDX:'), dataset(1, 5000)]));
  assert.deepEqual(w.fs.get(P.coqr), Buffer.from('prices-1'));
  assert.deepEqual(JSON.parse(w.text.get(P.state)!), state(1));
  assert.deepEqual(noTemp(w.fs), []);
  assert.ok(![...w.fs.keys()].some((k) => k.endsWith('.zst.part')));
});

test('sync: next day applies the patch chain and the index deltas, then is idempotent', async () => {
  const w = world(release(1));
  await runSync(w.deps, { dir: DIR });
  w.setRelease(release(3, { chainFrom: [1, 2] }));
  w.log.length = 0;
  const r = await runSync(w.deps, { dir: DIR });
  assert.deepEqual([r.status, r.patches, r.prices], ['patched', 2, 'updated']);
  assert.deepEqual(w.log.filter((l) => !l.startsWith('GET')), ['DL cards-S1-to-S2.coqp', 'applyPatch', 'DL cards-S1-to-S2.coqsd', 'applyIndexDelta', 'DL cards-S2-to-S3.coqp', 'applyPatch', 'DL cards-S2-to-S3.coqsd', 'applyIndexDelta', 'DL prices-P3.coqr']);
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
  rel.files.set(rel.manifest.cards.patches[0].patch.url, Buffer.from('tampered'));
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
  rel.files.delete(rel.manifest.cards.patches[0].index_delta!.url);
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
  rel.files.set(rel.manifest.cards.file.url, Buffer.from('truncated'));
  w.setRelease(rel);
  await assert.rejects(runSync(w.deps, { dir: DIR }), /failed verification/);
  assert.equal(w.fs.get(P.coqd), before);
  assert.deepEqual(JSON.parse(w.text.get(P.state)!), state(1));
  assert.deepEqual(noTemp(w.fs), []);
});

test('sync: an index that does not unpack to the published hash is rejected', async () => {
  const rel = release(1);
  const zurl = rel.manifest.cards.index.url;
  const bad = Buffer.concat([Buffer.from('ZST!'), Buffer.from('IDX:other')]);
  rel.files.set(zurl, bad);
  rel.manifest.cards.index.sha256 = sha(bad);
  rel.manifest.cards.index.size = bad.length;
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
