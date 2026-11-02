import { call } from './native';

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
/** Scope searches to your own cards (collection, a tag, a deck): register Scryfall ids under a name. */
export const registerSet = (name: string, ids: string[]) =>
  call<{ resolved: number; unknown: number }>('registerSet', { name, ids });
/** Apply a dataset patch (.coqp) in place; the index is stale until applyIndexDelta or the next search. */
export const applyPatch = (path: string) => call<EngineInfo>('applyPatch', { path });
/** Apply the matching index delta (.coqid) after applyPatch. If it fails, the index is rebuilt on demand. */
export const applyIndexDelta = (path: string) => call<EngineInfo>('applyIndexDelta', { path });

// --- Data sync (docs/roadmap.md, Milestone 2) -------------------------------------------------------------------
export { dataPaths, DEFAULT_MANIFEST_URL } from './sync/run';
export type { SyncOptions, SyncResult } from './sync/run';
export { sync, openSynced } from './sync/sync';
