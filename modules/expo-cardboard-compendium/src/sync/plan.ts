// Pure sync planning: given the manifest and what is on the device, decide what to download. No I/O.

import type { Dataset, Link, Manifest } from './manifest';
import { resolve, SUPPORTED_MANIFEST_VERSION } from './manifest';

/** Highest reader version of each file kind this build of the module can read. Keep in step with the Rust crates. */
export interface ModuleFormat {
  coqd: number; // bases and overlays
  coqi: number; // search index
}
export const MODULE_FORMAT: ModuleFormat = { coqd: 3, coqi: 2 };

/** What is on the device (persisted by the sync layer in `state.json`). */
export interface LocalState {
  epoch: number;
  cardsStamp: string;
  cardsSha256: string;
  /** Overlay id -> stamp of the file on disk. */
  overlays: Record<string, string>;
}

export type FullReason = 'no_local_data' | 'epoch_changed' | 'hash_mismatch' | 'chain_broken' | 'patches_not_smaller' | 'fallback';

/** One base patch with the index delta that goes with it (null: the index is rebuilt on the device). */
export interface CardStep {
  patch: Link;
  indexDelta: Link | null;
}

export type CardsPlan = { kind: 'up_to_date' } | { kind: 'full'; reason: FullReason } | { kind: 'patch'; steps: CardStep[] };

export type OverlayPlan =
  | { kind: 'up_to_date' }
  | { kind: 'full'; reason: 'no_local_data' | 'epoch_changed' | 'chain_broken' | 'patches_not_smaller' | 'patching_unsupported' }
  | { kind: 'patch'; steps: Link[] };

export type Plan =
  | { status: 'needs_app_update'; required: Partial<ModuleFormat> & { manifest?: number } }
  | { status: 'ok'; cards: CardsPlan; overlays: Record<string, OverlayPlan> };

export interface PlanOptions {
  /** The engine can apply `.coqp` to an overlay (S4c engine work). Until then overlays always come as the (small) full file. */
  patchOverlays?: boolean;
}

/** Follow `from -> to` links from `start` to `target`; null if the chain is broken or loops. */
function walk<T extends { from: string; to: string }>(links: T[], start: string, target: string): T[] | null {
  const steps: T[] = [];
  const seen = new Set<string>([start]);
  let cur = start;
  while (cur !== target) {
    const next = links.find((l) => l.from === cur);
    if (!next || seen.has(next.to)) return null;
    steps.push(next);
    seen.add(next.to);
    cur = next.to;
  }
  return steps;
}

export function planSync(m: Manifest, local: LocalState | null, fmt: ModuleFormat = MODULE_FORMAT, o: PlanOptions = {}): Plan {
  if (m.manifest_version > SUPPORTED_MANIFEST_VERSION) return { status: 'needs_app_update', required: { manifest: m.manifest_version } };
  const r = resolve(m);
  const required: Partial<ModuleFormat> = {};
  for (const d of [r.cards, r.index, ...r.overlays]) {
    const key = d.role === 'index' ? 'coqi' : 'coqd';
    if (d.min_client_format > fmt[key]) required[key] = Math.max(required[key] ?? 0, d.min_client_format);
  }
  if (Object.keys(required).length > 0) return { status: 'needs_app_update', required };
  const overlays: Record<string, OverlayPlan> = {};
  for (const d of r.overlays) overlays[d.id] = planOverlay(m, d, local, o);
  return { status: 'ok', cards: planCards(m, local), overlays };
}

/** Base links paired with the index delta of the same step. */
export function cardSteps(cards: Dataset, index: Dataset, links: Link[]): CardStep[] {
  return links.map((patch) => ({ patch, indexDelta: index.patches.find((x) => x.from === patch.from && x.to === patch.to) ?? null }));
}

function planCards(m: Manifest, local: LocalState | null): CardsPlan {
  const { cards, index } = resolve(m);
  if (!local) return { kind: 'full', reason: 'no_local_data' };
  if (local.epoch !== m.epoch) return { kind: 'full', reason: 'epoch_changed' };
  if (local.cardsStamp === cards.stamp) {
    return local.cardsSha256 === cards.file.sha256 ? { kind: 'up_to_date' } : { kind: 'full', reason: 'hash_mismatch' };
  }
  const links = walk(cards.patches, local.cardsStamp, cards.stamp);
  if (!links) return { kind: 'full', reason: 'chain_broken' };
  const steps = cardSteps(cards, index, links);
  // A missing index delta means the index is rebuilt on the device, so count only what would be downloaded.
  const patchBytes = steps.reduce((n, s) => n + s.patch.patch.size + (s.indexDelta?.patch.size ?? 0), 0);
  if (patchBytes >= cards.file.size + index.file.size) return { kind: 'full', reason: 'patches_not_smaller' };
  return { kind: 'patch', steps };
}

function planOverlay(m: Manifest, d: Dataset, local: LocalState | null, o: PlanOptions): OverlayPlan {
  const have = local?.overlays[d.id];
  if (!local || have === undefined) return { kind: 'full', reason: 'no_local_data' };
  if (local.epoch !== m.epoch) return { kind: 'full', reason: 'epoch_changed' };
  if (have === d.stamp) return { kind: 'up_to_date' };
  if (!o.patchOverlays) return { kind: 'full', reason: 'patching_unsupported' };
  const steps = walk(d.patches, have, d.stamp);
  if (!steps) return { kind: 'full', reason: 'chain_broken' };
  if (steps.reduce((n, s) => n + s.patch.size, 0) >= d.file.size) return { kind: 'full', reason: 'patches_not_smaller' };
  return { kind: 'patch', steps };
}

/** Bytes a plan would download (for "download 5 MB?" prompts). */
export function planBytes(m: Manifest, p: Plan): number {
  if (p.status !== 'ok') return 0;
  const { cards, index, overlays } = resolve(m);
  let n = 0;
  if (p.cards.kind === 'full') n += cards.file.size + index.file.size;
  if (p.cards.kind === 'patch') n += p.cards.steps.reduce((a, s) => a + s.patch.patch.size + (s.indexDelta?.patch.size ?? 0), 0);
  for (const d of overlays) {
    const op = p.overlays[d.id];
    if (op?.kind === 'full') n += d.file.size;
    if (op?.kind === 'patch') n += op.steps.reduce((a, s) => a + s.patch.size, 0);
  }
  return n;
}
