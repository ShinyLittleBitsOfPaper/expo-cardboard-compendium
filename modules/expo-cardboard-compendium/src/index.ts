import { call } from './native';
export { setSlowCallObserver } from './native';
export type { SlowCall } from './native';

export type Row = number; // printing row: stable and append-only across patches
export type Card = Record<string, any>; // Scryfall card JSON
export interface EngineInfo {
  printings: number;
  oracleCards: number;
  datasetSha256: string;
  index: 'coqi' | 'built' | 'delta' | 'stale';
  sets: Record<string, number>;
  /** Attached overlays (prices, ...): file hash, row count, source date. */
  overlays: Record<string, { sha256: string; rows: number; sourceDate: number }>;
}
export type Prices = Record<'usd' | 'usd_foil' | 'usd_etched' | 'eur' | 'eur_foil' | 'eur_etched', string | null>;
export interface SearchOptions {
  limit?: number;
  offset?: number;
  scope?: string; // name from registerSet
  cards?: boolean; // include card JSON for the returned rows
  prices?: boolean; // with `cards`: add a `prices` object to each card (needs the prices overlay)
}
export interface SearchResult {
  total: number;
  rows: Row[];
  cards?: Card[];
}

/** Paths are plain filesystem paths (not file:// URIs). `coqi` is created/refreshed if missing or stale. */
export const open = (coqd: string, coqi?: string) => call<EngineInfo>('open', { coqd, coqi });
export const close = () => call<null>('close');
export const info = () => call<EngineInfo>('info');
export const search = (query: string, o: SearchOptions = {}) => call<SearchResult>('search', { query, ...o });
/** Records (MTG: card JSON) for primary-table rows; null for a removed row. */
export const getRecords = (rows: Row[], o: { prices?: boolean } = {}) => call<(Card | null)[]>('getRecords', { rows, ...o });
/** MTGJSON retail prices of rows, Scryfall-shaped (`usd`, `usd_foil`, `usd_etched` = TCGplayer; `eur`, `eur_foil`, `eur_etched` = Cardmarket; strings, null = none). Needs the prices overlay. */
export const getPrices = (rows: Row[]) => call<(Prices | null)[]>('getPrices', { rows });
/** One record by its natural key (MTG: Scryfall id), null when unknown. */
export const getByKey = (key: string) => call<Card | null>('getByKey', { key });
export const getCards = getRecords;
export const getByScryfallId = getByKey;
/**
 * Attach an overlay dataset (a `.coqd` such as prices.coqd) so searches can use it (`usd>1`, `eur<=0.5`, `usdfoil`, ...).
 * Re-attach after a sync replaced the file. Rejected when the overlay belongs to another card epoch.
 */
export const attachOverlay = (id: string, path: string) => call<EngineInfo>('attachOverlay', { id, path });
export const detachOverlay = (id: string) => call<EngineInfo>('detachOverlay', { id });
/**
 * Art hashes (docs/phash-design.md). Attach the `arthash` overlay (`attachOverlay('arthash', path)`), hash the photo once with
 * `hashImage`, then rank the printings you already narrowed down by name with `matchArtHash`. Use it to break ties between
 * printings, never to reject a candidate: `confident` is true only when the best art is within `maxDistance` bits (default 24)
 * and beats the next-best DISTINCT art by `margin` bits (default 2); printings sharing one art tie, and `decisive` is false when
 * the candidates share a single art (nothing to decide). Rows without a hash come back in `unhashed`.
 */
export type ArtMatch = {
  ranked: { row: Row; distance: number }[];
  unhashed: Row[];
  best: number | null;
  margin: number | null;
  decisive: boolean;
  confident: boolean;
};
/** Photo-side hashes (16 hex digits each) of an image file; `card: true` when it is a whole straightened card (the art box is cut out first). */
export const hashImage = (path: string, o: { card?: boolean } = {}) => call<{ hashes: string[] }>('hashImage', { path, ...o });
export const matchArtHash = (
  rows: Row[],
  query: { hashes: string[] } | { path: string; card?: boolean },
  o: { maxDistance?: number; margin?: number } = {},
) => call<ArtMatch>('matchArtHash', { rows, ...query, ...o });
/** Rows of many Scryfall ids at once, in order; null for an unknown id. */
export const rowsOf = (ids: string[]) => call<(Row | null)[]>('rowsOf', { ids });
/** Cards of many Scryfall ids at once, in order; null for an unknown id. `prices` adds `card.prices` (MTGJSON, needs the prices overlay). */
export const getByKeys = (ids: string[], o: { prices?: boolean } = {}) => call<(Card | null)[]>('getByKeys', { ids, ...o });
export const getByScryfallIds = getByKeys;
/** Like registerSet but from rows (e.g. from `rowsOf`): `unknown` counts rows that do not exist or were removed. */
export const registerRows = (name: string, rows: Row[]) => call<{ resolved: number; unknown: number }>('registerRows', { name, rows });
export const dropSet = (name: string) => call<EngineInfo>('dropSet', { name });
/** Persist all named sets to a file, and load them back after `open`. Sets from another dataset epoch (or a bad file) load as nothing: re-register from your own data. */
export const saveSets = (path: string) => call<null>('saveSets', { path });
export const loadSets = (path: string) => call<{ loaded: number }>('loadSets', { path });
/** Scope searches to your own cards (collection, a tag, a deck): register Scryfall ids under a name. */
export const registerSet = (name: string, ids: string[]) =>
  call<{ resolved: number; unknown: number }>('registerSet', { name, ids });
/** Apply a dataset patch (.coqp) in place; the index is stale until applyIndexDelta or the next search. */
export const applyPatch = (path: string) => call<EngineInfo>('applyPatch', { path });
/** Apply the matching index delta (.coqid) after applyPatch. If it fails, the index is rebuilt on demand. */
export const applyIndexDelta = (path: string) => call<EngineInfo>('applyIndexDelta', { path });

// --- Data sync (docs/roadmap.md, Milestone 2) -------------------------------------------------------------------
export { dataPaths, DEFAULT_MANIFEST_URL } from './sync/run';
export type { SyncOptions, SyncResult, SyncProgress } from './sync/run';
export { planBytes } from './sync/plan';
export { sync, openSynced, attachSyncedOverlays } from './sync/sync';
export { adaptAppQuery } from './sync/appQuery';
export type { AppQuery, AppQueryOptions } from './sync/appQuery';
