// Pure sync planning: given the manifest and what is on the device, decide what to download. No I/O.

import type { CardPatch, Manifest, MinFormat, PricePatch } from './manifest';
import { SUPPORTED_MANIFEST_VERSION } from './manifest';

/** Highest reader format of each file type this build of the module can read. Keep in step with the Rust crates. */
export const MODULE_FORMAT: MinFormat = { coqd: 2, coqs: 1, coqr: 1 };

/** What is on the device (persisted by the sync layer in `state.json`). */
export interface LocalState {
  epoch: number;
  cardsStamp: string;
  cardsSha256: string;
  pricesStamp: string | null;
}

export type FullReason = 'no_local_data' | 'epoch_changed' | 'hash_mismatch' | 'chain_broken' | 'patches_not_smaller' | 'fallback';

export type CardsPlan =
  | { kind: 'up_to_date' }
  | { kind: 'full'; reason: FullReason }
  | { kind: 'patch'; steps: CardPatch[] };

export type PricesPlan =
  | { kind: 'up_to_date' }
  | { kind: 'full'; reason: 'no_local_data' | 'chain_broken' | 'patches_not_smaller' | 'patching_unsupported' | 'format_changed' }
  | { kind: 'patch'; steps: PricePatch[] };

export type Plan =
  | { status: 'needs_app_update'; required: Partial<MinFormat> & { manifest?: number } }
  | { status: 'ok'; cards: CardsPlan; prices: PricesPlan };

export interface PlanOptions {
  /** The engine cannot apply `.coqx` yet (Milestone 3): prices then always come as the (small) full file. */
  patchPrices?: boolean;
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

export function planSync(m: Manifest, local: LocalState | null, fmt: MinFormat = MODULE_FORMAT, o: PlanOptions = {}): Plan {
  const required: Partial<MinFormat> & { manifest?: number } = {};
  if (m.manifest_version > SUPPORTED_MANIFEST_VERSION) required.manifest = m.manifest_version;
  if (m.min_client_format.coqd > fmt.coqd) required.coqd = m.min_client_format.coqd;
  if (m.min_client_format.coqs > fmt.coqs) required.coqs = m.min_client_format.coqs;
  if (m.min_client_format.coqr > fmt.coqr) required.coqr = m.min_client_format.coqr;
  if (Object.keys(required).length > 0) return { status: 'needs_app_update', required };

  return { status: 'ok', cards: planCards(m, local), prices: planPrices(m, local, o) };
}

function planCards(m: Manifest, local: LocalState | null): CardsPlan {
  if (!local) return { kind: 'full', reason: 'no_local_data' };
  if (local.epoch !== m.epoch) return { kind: 'full', reason: 'epoch_changed' };
  if (local.cardsStamp === m.cards.stamp) {
    return local.cardsSha256 === m.cards.file.sha256 ? { kind: 'up_to_date' } : { kind: 'full', reason: 'hash_mismatch' };
  }
  const steps = walk(m.cards.patches, local.cardsStamp, m.cards.stamp);
  if (!steps) return { kind: 'full', reason: 'chain_broken' };
  // A missing index delta means the index is rebuilt on the device, so count only what would be downloaded.
  const patchBytes = steps.reduce((n, s) => n + s.patch.size + (s.index_delta?.size ?? 0), 0);
  if (patchBytes >= m.cards.file.size + m.cards.index.size) return { kind: 'full', reason: 'patches_not_smaller' };
  return { kind: 'patch', steps };
}

function planPrices(m: Manifest, local: LocalState | null, o: PlanOptions): PricesPlan {
  if (!local || local.pricesStamp === null) return { kind: 'full', reason: 'no_local_data' };
  if (local.pricesStamp === m.prices.stamp) return { kind: 'up_to_date' };
  if (!o.patchPrices) return { kind: 'full', reason: 'patching_unsupported' };
  const steps = walk(m.prices.patches, local.pricesStamp, m.prices.stamp);
  if (!steps) return { kind: 'full', reason: 'chain_broken' };
  if (steps.reduce((n, s) => n + s.patch.size, 0) >= m.prices.file.size) return { kind: 'full', reason: 'patches_not_smaller' };
  return { kind: 'patch', steps };
}

/** Bytes a plan would download (for "download 5 MB?" prompts). */
export function planBytes(m: Manifest, p: Plan): number {
  if (p.status !== 'ok') return 0;
  let n = 0;
  if (p.cards.kind === 'full') n += m.cards.file.size + m.cards.index.size;
  if (p.cards.kind === 'patch') n += p.cards.steps.reduce((a, s) => a + s.patch.size + (s.index_delta?.size ?? 0), 0);
  if (p.prices.kind === 'full') n += m.prices.file.size;
  if (p.prices.kind === 'patch') n += p.prices.steps.reduce((a, s) => a + s.patch.size, 0);
  return n;
}
