// Sync executor: manifest -> plan -> verified downloads -> files on disk. All I/O goes through `SyncDeps`, so the whole
// flow is testable with fakes (src/sync/__tests__). Invariant: a file only ever appears under its final name after its
// SHA-256 was checked, and every swap is an atomic rename, so an interrupted sync leaves a working (older) dataset.

import { parseManifest, resolve, SUPPORTED_MANIFEST_VERSION, type Asset, type Link, type Manifest } from './manifest';
import { planSync, planBytes, MODULE_FORMAT, type CardStep, type FullReason, type LocalState, type ModuleFormat } from './plan';

export const DEFAULT_MANIFEST_URL =
  'https://github.com/ShinyLittleBitsOfPaper/expo-cardboard-compendium/releases/download/data-latest/manifest.json';

export interface EngineOps {
  open(coqd: string, coqi: string): Promise<{ datasetSha256: string; index: string }>;
  info(): Promise<{ datasetSha256: string; index: string }>;
  applyPatch(path: string): Promise<{ datasetSha256: string; index: string }>;
  applyIndexDelta(path: string): Promise<{ datasetSha256: string; index: string }>;
  sha256File(path: string): Promise<{ sha256: string; size: number }>;
  unzstFile(path: string, out: string): Promise<{ sha256: string; size: number }>;
  /** Apply a .coqp to the file at `path`, writing `out`; needs no open engine (used for overlays). */
  patchFile(path: string, patch: string, out: string): Promise<{ sha256: string; size: number }>;
  attachOverlay(id: string, path: string): Promise<unknown>;
  moveFile(from: string, to: string): Promise<void>;
  removeFile(path: string): Promise<void>;
}

export interface SyncDeps {
  fetchJson(url: string): Promise<unknown>;
  /**
   * Download `url` to `destPath` (plain filesystem path), replacing anything there. `onBytes` (optional) is called with the
   * bytes written so far; an implementation that cannot report progress may ignore it (the sync still reports completion).
   */
  download(url: string, destPath: string, onBytes?: (received: number) => void): Promise<void>;
  /** null when the file does not exist */
  readText(path: string): Promise<string | null>;
  writeText(path: string, text: string): Promise<void>;
  mkdirs(dir: string): Promise<void>;
  engine: EngineOps;
}

export interface SyncProgress {
  /** Human-readable step: 'cards', 'index', 'patch <stamp>', an overlay id, ... */
  step: string;
  /** Position of `step` within its group (1-based) and the group's size. */
  index: number;
  total: number;
  /** Set on download events only. */
  file?: string;
  /** Bytes of `file` written so far and its published size. */
  received?: number;
  size?: number;
  /** Bytes downloaded in this sync so far / expected in total (see `SyncOptions.onProgress`). */
  bytesDone?: number;
  bytesTotal?: number;
}

export interface SyncOptions {
  /** Directory (plain path) that holds cards.coqd, cards.coqi, <overlay>.coqd (prices.coqd) and state.json. */
  dir: string;
  manifestUrl?: string;
  /**
   * Called before each download/apply step (no `file`), and while a file downloads (`file`, `received`, `size`, `bytesDone`,
   * `bytesTotal`; at most about once per percent of that file, and once at its end). Use `bytesDone / bytesTotal` for one
   * overall bar. `bytesTotal` starts at the planned download size and only grows (a patch that fails and falls back to the
   * full files adds those files), so a bar can slow down but never jumps backwards.
   */
  onProgress?: (e: SyncProgress) => void;
  /** Test hook: override the module's reader formats. */
  moduleFormat?: ModuleFormat;
}

export interface SyncResult {
  status: 'up_to_date' | 'patched' | 'full' | 'needs_app_update';
  /** Number of dataset patches applied (status 'patched'). */
  patches: number;
  /** Overlay datasets (prices, ...) whose files were replaced. */
  overlays: 'unchanged' | 'updated';
  /** True when files on disk changed: the app should (re)open the engine and re-register its sets. */
  changed: boolean;
  /** Why a full download happened (status 'full'). */
  reason?: FullReason;
  /** Formats this build cannot read yet (status 'needs_app_update'): ask the user to update the app. */
  required?: Partial<ModuleFormat> & { manifest?: number };
}

export const dataPaths = (dir: string) => {
  const d = dir.replace(/\/+$/, '');
  return {
    coqd: `${d}/cards.coqd`,
    coqi: `${d}/cards.coqi`,
    /** File of an overlay dataset (`prices` -> prices.coqd). */
    overlay: (id: string) => `${d}/${id}.coqd`,
    state: `${d}/state.json`,
  };
};

function parseState(text: string | null): LocalState | null {
  if (!text) return null;
  try {
    const s = JSON.parse(text);
    if (typeof s.epoch === 'number' && typeof s.cardsStamp === 'string' && typeof s.cardsSha256 === 'string') {
      const overlays: Record<string, string> = {};
      for (const [k, v] of Object.entries(s.overlays ?? {})) if (typeof v === 'string') overlays[k] = v;
      return { epoch: s.epoch, cardsStamp: s.cardsStamp, cardsSha256: s.cardsSha256, overlays };
    }
  } catch {
    // fall through: unreadable state means "start over"
  }
  return null;
}

export async function runSync(deps: SyncDeps, o: SyncOptions): Promise<SyncResult> {
  const { engine } = deps;
  const paths = dataPaths(o.dir);
  let curStep = { step: '', index: 0, total: 0 };
  const progress = (step: string, index: number, total: number) => {
    curStep = { step, index, total };
    o.onProgress?.({ step, index, total });
  };

  await deps.mkdirs(o.dir);
  const rawManifest = await deps.fetchJson(o.manifestUrl ?? DEFAULT_MANIFEST_URL);
  // A newer manifest may have another shape: decide "update the app" before validating the fields.
  const mv = (rawManifest as { manifest_version?: unknown } | null)?.manifest_version;
  if (typeof mv === 'number' && mv > SUPPORTED_MANIFEST_VERSION) {
    return { status: 'needs_app_update', patches: 0, overlays: 'unchanged', changed: false, required: { manifest: mv } };
  }
  const manifest: Manifest = parseManifest(rawManifest);
  const { cards, index, overlays } = resolve(manifest);
  let local = parseState(await deps.readText(paths.state));
  const plan = planSync(manifest, local, o.moduleFormat ?? MODULE_FORMAT, { patchOverlays: true });
  let bytesDone = 0;
  let bytesTotal = planBytes(manifest, plan);
  if (plan.status === 'needs_app_update') {
    return { status: 'needs_app_update', patches: 0, overlays: 'unchanged', changed: false, required: plan.required };
  }

  const tmp = (name: string) => `${o.dir.replace(/\/+$/, '')}/${name}`;
  /** Download to a temp name and check size + SHA-256; returns the temp path (caller moves or removes it). */
  const fetchVerified = async (a: Asset, tmpName: string): Promise<string> => {
    const p = tmp(tmpName);
    await engine.removeFile(p);
    bytesTotal = Math.max(bytesTotal, bytesDone + a.size);
    const emit = (received: number) =>
      o.onProgress?.({ ...curStep, file: a.name, received, size: a.size, bytesDone: bytesDone + received, bytesTotal });
    let lastPct = -1;
    emit(0);
    await deps.download(a.url, p, (received) => {
      const pct = a.size > 0 ? Math.floor((Math.min(received, a.size) * 100) / a.size) : 0;
      if (pct > lastPct && pct < 100) {
        lastPct = pct;
        emit(Math.min(received, a.size));
      }
    });
    emit(a.size);
    bytesDone += a.size;
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
    const d = await fetchVerified(cards.file, 'cards.coqd.part');
    progress('index', 2, total);
    const z = await fetchVerified(index.file, 'cards.coqi.zst.part');
    const raw = await engine.unzstFile(z, tmp('cards.coqi.part'));
    await engine.removeFile(z);
    if (raw.sha256 !== index.raw!.sha256 || raw.size !== index.raw!.size) {
      await engine.removeFile(tmp('cards.coqi.part'));
      await engine.removeFile(d);
      throw new Error('cards index: decompressed data failed verification');
    }
    // Swap dataset first, then index: a crash in between leaves a stale index, which the engine rebuilds on open.
    await engine.moveFile(d, paths.coqd);
    await engine.moveFile(tmp('cards.coqi.part'), paths.coqi);
    // Overlays follow the card row ids: on a new epoch the ones on disk no longer match, so forget them.
    const keep = local && local.epoch === manifest.epoch ? local.overlays : {};
    await saveState({ epoch: manifest.epoch, cardsStamp: cards.stamp, cardsSha256: cards.file.sha256, overlays: keep });
  };

  const patchCards = async (chain: CardStep[]) => {
    let info: { datasetSha256: string } | null = null;
    try {
      info = await engine.info();
    } catch {
      // engine not open yet
    }
    if (!info || info.datasetSha256 !== local!.cardsSha256) info = await engine.open(paths.coqd, paths.coqi);
    for (const [i, s] of chain.entries()) {
      progress(`patch ${s.patch.to}`, i + 1, chain.length);
      const p = await fetchVerified(s.patch.patch, 'step.coqp.part');
      try {
        info = await engine.applyPatch(p);
      } finally {
        await engine.removeFile(p);
      }
      if (s.indexDelta) {
        // Best effort: if the delta does not apply, the engine rebuilds the index on the next search.
        try {
          const d = await fetchVerified(s.indexDelta.patch, 'step.coqid.part');
          try {
            info = await engine.applyIndexDelta(d);
          } finally {
            await engine.removeFile(d);
          }
        } catch {
          // keep going
        }
      }
      await saveState({ ...local!, cardsStamp: s.patch.to, cardsSha256: info.datasetSha256 });
    }
    if (info.datasetSha256 !== cards.file.sha256) throw new Error('patched dataset does not match the published file');
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

  let updated: SyncResult['overlays'] = 'unchanged';
  /** Apply the overlay's patch chain to the file on disk (no engine needed), verifying the end result. */
  const patchOverlay = async (id: string, steps: Link[], want: Asset) => {
    const cur = paths.overlay(id);
    let from = cur;
    for (const [i, s] of steps.entries()) {
      progress(`${id} ${s.to}`, i + 1, steps.length);
      const p = await fetchVerified(s.patch, `${id}.coqp.part`);
      const out = tmp(`${id}.coqd.${i % 2}.part`);
      try {
        await engine.patchFile(from, p, out);
      } finally {
        await engine.removeFile(p);
      }
      if (from !== cur) await engine.removeFile(from);
      from = out;
    }
    const got = await engine.sha256File(from);
    if (got.sha256 !== want.sha256 || got.size !== want.size) {
      await engine.removeFile(from);
      throw new Error(`${id}: patched file does not match the published one`);
    }
    await engine.moveFile(from, cur);
  };
  for (const d of overlays) {
    const op = plan.overlays[d.id];
    if (!op || op.kind === 'up_to_date') continue;
    if (op.kind === 'patch') {
      try {
        await patchOverlay(d.id, op.steps, d.file);
      } catch {
        // A bad patch must not strand the user: the full file is small.
        const p = await fetchVerified(d.file, `${d.id}.coqd.part`);
        await engine.moveFile(p, paths.overlay(d.id));
      }
    } else {
      progress(d.id, 1, 1);
      const p = await fetchVerified(d.file, `${d.id}.coqd.part`);
      await engine.moveFile(p, paths.overlay(d.id));
    }
    await saveState({ ...local!, overlays: { ...local!.overlays, [d.id]: d.stamp } });
    updated = 'updated';
  }

  const changed = status !== 'up_to_date' || updated === 'updated';
  return { status, patches, overlays: updated, changed, ...(reason ? { reason } : {}) };
}

/**
 * Open the synced data in the engine: the cards dataset + index, then every overlay recorded in state.json (prices, ...).
 * Call after `sync` (when `changed`) and at app start. Overlays that fail to attach are skipped and returned in `failed`.
 */
export async function openLocal(deps: SyncDeps, dir: string): Promise<{ attached: string[]; failed: string[] }> {
  const paths = dataPaths(dir);
  const state = parseState(await deps.readText(paths.state));
  if (!state) throw new Error('no synced data: run sync first');
  await deps.engine.open(paths.coqd, paths.coqi);
  const attached: string[] = [];
  const failed: string[] = [];
  for (const id of Object.keys(state.overlays)) {
    try {
      await deps.engine.attachOverlay(id, paths.overlay(id));
      attached.push(id);
    } catch {
      failed.push(id);
    }
  }
  return { attached, failed };
}
