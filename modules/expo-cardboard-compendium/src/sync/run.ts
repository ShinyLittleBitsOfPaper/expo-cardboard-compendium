// Sync executor: manifest -> plan -> verified downloads -> files on disk. All I/O goes through `SyncDeps`, so the whole
// flow is testable with fakes (src/sync/__tests__). Invariant: a file only ever appears under its final name after its
// SHA-256 was checked, and every swap is an atomic rename, so an interrupted sync leaves a working (older) dataset.

import { parseManifest, type Asset, type CardPatch, type Manifest, type MinFormat } from './manifest';
import { planSync, MODULE_FORMAT, type FullReason, type LocalState } from './plan';

export const DEFAULT_MANIFEST_URL =
  'https://github.com/ShinyLittleBitsOfPaper/expo-cardboard-compendium/releases/download/data-latest/manifest.json';

export interface EngineOps {
  open(coqd: string, coqi: string): Promise<{ datasetSha256: string; index: string }>;
  info(): Promise<{ datasetSha256: string; index: string }>;
  applyPatch(path: string): Promise<{ datasetSha256: string; index: string }>;
  applyIndexDelta(path: string): Promise<{ datasetSha256: string; index: string }>;
  sha256File(path: string): Promise<{ sha256: string; size: number }>;
  unzstFile(path: string, out: string): Promise<{ sha256: string; size: number }>;
  moveFile(from: string, to: string): Promise<void>;
  removeFile(path: string): Promise<void>;
}

export interface SyncDeps {
  fetchJson(url: string): Promise<unknown>;
  /** Download `url` to `destPath` (plain filesystem path), replacing anything there. */
  download(url: string, destPath: string): Promise<void>;
  /** null when the file does not exist */
  readText(path: string): Promise<string | null>;
  writeText(path: string, text: string): Promise<void>;
  mkdirs(dir: string): Promise<void>;
  engine: EngineOps;
}

export interface SyncOptions {
  /** Directory (plain path) that holds cards.coqd, cards.coqi, prices.coqr and state.json. */
  dir: string;
  manifestUrl?: string;
  /** Called before each download/apply step. */
  onProgress?: (e: { step: string; index: number; total: number }) => void;
  /** Test hook: override the module's reader formats. */
  moduleFormat?: MinFormat;
}

export interface SyncResult {
  status: 'up_to_date' | 'patched' | 'full' | 'needs_app_update';
  /** Number of dataset patches applied (status 'patched'). */
  patches: number;
  prices: 'unchanged' | 'updated';
  /** True when files on disk changed: the app should (re)open the engine and re-register its sets. */
  changed: boolean;
  /** Why a full download happened (status 'full'). */
  reason?: FullReason;
  /** Formats this build cannot read yet (status 'needs_app_update'): ask the user to update the app. */
  required?: Partial<MinFormat> & { manifest?: number };
}

export const dataPaths = (dir: string) => {
  const d = dir.replace(/\/+$/, '');
  return { coqd: `${d}/cards.coqd`, coqi: `${d}/cards.coqi`, coqr: `${d}/prices.coqr`, state: `${d}/state.json` };
};

function parseState(text: string | null): LocalState | null {
  if (!text) return null;
  try {
    const s = JSON.parse(text);
    if (typeof s.epoch === 'number' && typeof s.cardsStamp === 'string' && typeof s.cardsSha256 === 'string') {
      return { epoch: s.epoch, cardsStamp: s.cardsStamp, cardsSha256: s.cardsSha256, pricesStamp: typeof s.pricesStamp === 'string' ? s.pricesStamp : null };
    }
  } catch {
    // fall through: unreadable state means "start over"
  }
  return null;
}

export async function runSync(deps: SyncDeps, o: SyncOptions): Promise<SyncResult> {
  const { engine } = deps;
  const paths = dataPaths(o.dir);
  const progress = (step: string, index: number, total: number) => o.onProgress?.({ step, index, total });

  await deps.mkdirs(o.dir);
  const manifest: Manifest = parseManifest(await deps.fetchJson(o.manifestUrl ?? DEFAULT_MANIFEST_URL));
  let local = parseState(await deps.readText(paths.state));
  const plan = planSync(manifest, local, o.moduleFormat ?? MODULE_FORMAT);
  if (plan.status === 'needs_app_update') {
    return { status: 'needs_app_update', patches: 0, prices: 'unchanged', changed: false, required: plan.required };
  }

  const tmp = (name: string) => `${o.dir.replace(/\/+$/, '')}/${name}`;
  /** Download to a temp name and check size + SHA-256; returns the temp path (caller moves or removes it). */
  const fetchVerified = async (a: Asset, tmpName: string): Promise<string> => {
    const p = tmp(tmpName);
    await engine.removeFile(p);
    await deps.download(a.url, p);
    const got = await engine.sha256File(p);
    if (got.sha256 !== a.sha256 || got.size !== a.size) {
      await engine.removeFile(p);
      throw new Error(`${a.name}: download failed verification (size ${got.size}, expected ${a.size})`);
    }
    return p;
  };
  const saveState = async (s: LocalState) => {
    local = s;
    await deps.writeText(`${paths.state}.tmp`, JSON.stringify(s));
    await engine.moveFile(`${paths.state}.tmp`, paths.state);
  };

  const fullCards = async () => {
    const total = 2;
    progress('cards', 1, total);
    const d = await fetchVerified(manifest.cards.file, 'cards.coqd.part');
    progress('index', 2, total);
    const z = await fetchVerified(manifest.cards.index, 'cards.coqi.zst.part');
    const raw = await engine.unzstFile(z, tmp('cards.coqi.part'));
    await engine.removeFile(z);
    if (raw.sha256 !== manifest.cards.index.raw_sha256 || raw.size !== manifest.cards.index.raw_size) {
      await engine.removeFile(tmp('cards.coqi.part'));
      await engine.removeFile(d);
      throw new Error('cards index: decompressed data failed verification');
    }
    // Swap dataset first, then index: a crash in between leaves a stale index, which the engine rebuilds on open.
    await engine.moveFile(d, paths.coqd);
    await engine.moveFile(tmp('cards.coqi.part'), paths.coqi);
    await saveState({ epoch: manifest.epoch, cardsStamp: manifest.cards.stamp, cardsSha256: manifest.cards.file.sha256, pricesStamp: local?.pricesStamp ?? null });
  };

  const patchCards = async (chain: CardPatch[]) => {
    let info: { datasetSha256: string } | null = null;
    try {
      info = await engine.info();
    } catch {
      // engine not open yet
    }
    if (!info || info.datasetSha256 !== local!.cardsSha256) info = await engine.open(paths.coqd, paths.coqi);
    for (const [i, s] of chain.entries()) {
      progress(`patch ${s.to}`, i + 1, chain.length);
      const p = await fetchVerified(s.patch, 'step.coqp.part');
      try {
        info = await engine.applyPatch(p);
      } finally {
        await engine.removeFile(p);
      }
      if (s.index_delta) {
        // Best effort: if the delta does not apply, the engine rebuilds the index on the next search.
        try {
          const d = await fetchVerified(s.index_delta, 'step.coqid.part');
          try {
            info = await engine.applyIndexDelta(d);
          } finally {
            await engine.removeFile(d);
          }
        } catch {
          // keep going
        }
      }
      await saveState({ ...local!, cardsStamp: s.to, cardsSha256: info.datasetSha256 });
    }
    if (info.datasetSha256 !== manifest.cards.file.sha256) throw new Error('patched dataset does not match the published file');
  };

  let status: SyncResult['status'] = 'up_to_date';
  let patches = 0;
  let reason: FullReason | undefined;
  if (plan.cards.kind === 'full') {
    await fullCards();
    status = 'full';
    reason = plan.cards.reason;
  } else if (plan.cards.kind === 'patch') {
    try {
      await patchCards(plan.cards.steps);
      status = 'patched';
      patches = plan.cards.steps.length;
    } catch {
      // A failed patch step (corrupt, wrong base) must not strand the user: fetch the full files instead.
      await fullCards();
      status = 'full';
      reason = 'fallback';
    }
  }

  let prices: SyncResult['prices'] = 'unchanged';
  if (plan.prices.kind === 'full') {
    progress('prices', 1, 1);
    const p = await fetchVerified(manifest.prices.file, 'prices.coqr.part');
    await engine.moveFile(p, paths.coqr);
    await saveState({ ...local!, pricesStamp: manifest.prices.stamp });
    prices = 'updated';
  }

  const changed = status !== 'up_to_date' || prices === 'updated';
  return { status, patches, prices, changed, ...(reason ? { reason } : {}) };
}
