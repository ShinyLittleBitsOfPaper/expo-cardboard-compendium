// Types and validation for manifest.json (written by `coqc manifest`; see docs/roadmap.md, Milestone 1).

export interface Asset {
  name: string;
  url: string;
  size: number;
  sha256: string;
}
export interface IndexAsset extends Asset {
  encoding: 'zstd';
  raw_size: number;
  raw_sha256: string;
}
export interface CardPatch {
  from: string;
  to: string;
  at: string;
  patch: Asset;
  index_delta: Asset | null;
}
export interface PricePatch {
  from: string;
  to: string;
  at: string;
  patch: Asset;
}
export interface MinFormat {
  coqd: number;
  coqi: number;
  coqr: number;
}
export interface Manifest {
  manifest_version: number;
  generated_at: string;
  run: string;
  epoch: number;
  rebase: boolean;
  min_client_format: MinFormat;
  cards: { stamp: string; format: number; min_reader: number; file: Asset; index: IndexAsset; patches: CardPatch[] };
  prices: { stamp: string; format: number; min_reader: number; file: Asset; patches: PricePatch[] };
}

/** Manifest versions this module understands. A newer one means the app must be updated. */
export const SUPPORTED_MANIFEST_VERSION = 1;

const bad = (path: string): never => {
  throw new Error(`invalid manifest: ${path}`);
};
const str = (v: unknown, p: string): string => (typeof v === 'string' && v.length > 0 ? v : bad(p));
const num = (v: unknown, p: string): number => (typeof v === 'number' && Number.isFinite(v) && v >= 0 ? v : bad(p));
const arr = (v: unknown, p: string): unknown[] => (Array.isArray(v) ? v : bad(p));
const obj = (v: unknown, p: string): Record<string, unknown> =>
  v !== null && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : bad(p);

function asset(v: unknown, p: string): Asset {
  const o = obj(v, p);
  const sha256 = str(o.sha256, `${p}.sha256`);
  if (!/^[0-9a-f]{64}$/.test(sha256)) bad(`${p}.sha256`);
  const url = str(o.url, `${p}.url`);
  if (!url.startsWith('https://')) bad(`${p}.url (https only)`);
  return { name: str(o.name, `${p}.name`), url, size: num(o.size, `${p}.size`), sha256 };
}

/** Validate untrusted JSON into a Manifest (throws `invalid manifest: <path>`); extra fields are ignored. */
export function parseManifest(json: unknown): Manifest {
  const m = obj(json, 'root');
  const version = num(m.manifest_version, 'manifest_version');
  const mf = obj(m.min_client_format, 'min_client_format');
  const cards = obj(m.cards, 'cards');
  const prices = obj(m.prices, 'prices');
  const idx = obj(cards.index, 'cards.index');
  if (idx.encoding !== 'zstd') bad('cards.index.encoding');
  return {
    manifest_version: version,
    generated_at: str(m.generated_at, 'generated_at'),
    run: str(m.run, 'run'),
    epoch: num(m.epoch, 'epoch'),
    rebase: typeof m.rebase === 'boolean' ? m.rebase : bad('rebase'),
    min_client_format: {
      coqd: num(mf.coqd, 'min_client_format.coqd'),
      coqi: num(mf.coqi, 'min_client_format.coqi'),
      coqr: num(mf.coqr, 'min_client_format.coqr'),
    },
    cards: {
      stamp: str(cards.stamp, 'cards.stamp'),
      format: num(cards.format, 'cards.format'),
      min_reader: num(cards.min_reader, 'cards.min_reader'),
      file: asset(cards.file, 'cards.file'),
      index: {
        ...asset(idx, 'cards.index'),
        encoding: 'zstd',
        raw_size: num(idx.raw_size, 'cards.index.raw_size'),
        raw_sha256: str(idx.raw_sha256, 'cards.index.raw_sha256'),
      },
      patches: arr(cards.patches, 'cards.patches').map((x, i) => {
        const o = obj(x, `cards.patches[${i}]`);
        return {
          from: str(o.from, `cards.patches[${i}].from`),
          to: str(o.to, `cards.patches[${i}].to`),
          at: str(o.at, `cards.patches[${i}].at`),
          patch: asset(o.patch, `cards.patches[${i}].patch`),
          index_delta: o.index_delta == null ? null : asset(o.index_delta, `cards.patches[${i}].index_delta`),
        };
      }),
    },
    prices: {
      stamp: str(prices.stamp, 'prices.stamp'),
      format: num(prices.format, 'prices.format'),
      min_reader: num(prices.min_reader, 'prices.min_reader'),
      file: asset(prices.file, 'prices.file'),
      patches: arr(prices.patches, 'prices.patches').map((x, i) => {
        const o = obj(x, `prices.patches[${i}]`);
        return {
          from: str(o.from, `prices.patches[${i}].from`),
          to: str(o.to, `prices.patches[${i}].to`),
          at: str(o.at, `prices.patches[${i}].at`),
          patch: asset(o.patch, `prices.patches[${i}].patch`),
        };
      }),
    },
  };
}
