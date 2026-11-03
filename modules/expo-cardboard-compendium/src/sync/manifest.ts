// Types and validation for manifest.json v2 (written by `coqc manifest`; see docs/manifest.md).

export interface Asset {
  name: string;
  url: string;
  size: number;
  sha256: string;
}
export type Role = 'base' | 'index' | 'overlay';
/** One link of a dataset's patch chain. */
export interface Link {
  from: string;
  to: string;
  at: string;
  patch: Asset;
}
/** The shipped file is compressed; these describe the decompressed bytes (what index deltas refer to). */
export interface Raw {
  encoding: 'zstd';
  size: number;
  sha256: string;
}
export interface Dataset {
  id: string;
  role: Role;
  /** Base this index/overlay belongs to; null for a base. */
  of: string | null;
  stamp: string;
  format: number;
  /** Smallest reader version able to open it; above this build's (see plan.ts) => "update the app". */
  min_client_format: number;
  file: Asset;
  raw: Raw | null;
  patches: Link[];
}
export interface Manifest {
  manifest_version: number;
  generated_at: string;
  run: string;
  epoch: number;
  rebase: boolean;
  datasets: Dataset[];
}

/** Manifest versions this module understands. A newer one means the app must be updated. */
export const SUPPORTED_MANIFEST_VERSION = 2;

/**
 * The datasets this module syncs: the one `base` dataset (any id: `cards`, `books`, ...), its index, and any overlays (prices, ...)
 * of it. Roles decide, ids do not. The field is still called `cards` for the primary dataset; local file names are fixed (dataPaths).
 */
export interface Resolved {
  cards: Dataset;
  index: Dataset;
  overlays: Dataset[];
}
export function resolve(m: Manifest): Resolved {
  const bases = m.datasets.filter((d) => d.role === 'base');
  const cards = bases[0] ?? bad('datasets (no base dataset)');
  if (bases.length > 1) bad('datasets (more than one base dataset)');
  const index = m.datasets.find((d) => d.role === 'index' && d.of === cards.id) ?? bad(`datasets (no index of "${cards.id}")`);
  return { cards, index, overlays: m.datasets.filter((d) => d.role === 'overlay' && d.of === cards.id) };
}

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

function link(v: unknown, p: string): Link {
  const o = obj(v, p);
  return { from: str(o.from, `${p}.from`), to: str(o.to, `${p}.to`), at: str(o.at, `${p}.at`), patch: asset(o.patch, `${p}.patch`) };
}

/** Validate untrusted JSON into a Manifest (throws `invalid manifest: <path>`); extra fields are ignored. */
export function parseManifest(json: unknown): Manifest {
  const m = obj(json, 'root');
  const datasets = arr(m.datasets, 'datasets').map((x, i): Dataset => {
    const p = `datasets[${i}]`;
    const o = obj(x, p);
    const role = o.role;
    if (role !== 'base' && role !== 'index' && role !== 'overlay') bad(`${p}.role`);
    const of = o.of == null ? null : str(o.of, `${p}.of`);
    if ((role === 'base') !== (of === null)) bad(`${p}.of`);
    let raw: Raw | null = null;
    if (role === 'index') {
      const r = obj(o.raw, `${p}.raw`);
      if (r.encoding !== 'zstd') bad(`${p}.raw.encoding`);
      raw = { encoding: 'zstd', size: num(r.size, `${p}.raw.size`), sha256: str(r.sha256, `${p}.raw.sha256`) };
    }
    return {
      id: str(o.id, `${p}.id`),
      role: role as Role,
      of,
      stamp: str(o.stamp, `${p}.stamp`),
      format: num(o.format, `${p}.format`),
      min_client_format: num(o.min_client_format, `${p}.min_client_format`),
      file: asset(o.file, `${p}.file`),
      raw,
      patches: arr(o.patches, `${p}.patches`).map((l, k) => link(l, `${p}.patches[${k}]`)),
    };
  });
  if (new Set(datasets.map((d) => d.id)).size !== datasets.length) bad('datasets (repeated id)');
  const out: Manifest = {
    manifest_version: num(m.manifest_version, 'manifest_version'),
    generated_at: str(m.generated_at, 'generated_at'),
    run: str(m.run, 'run'),
    epoch: num(m.epoch, 'epoch'),
    rebase: typeof m.rebase === 'boolean' ? m.rebase : bad('rebase'),
    datasets,
  };
  resolve(out);
  return out;
}
