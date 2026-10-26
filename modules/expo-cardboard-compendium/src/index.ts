import { requireNativeModule } from 'expo-modules-core';

// One native entry point: call(method, argsJson) -> '{"ok":...}' | '{"error":"..."}'.
const Native = requireNativeModule<{ call(method: string, args: string): Promise<string> }>('ExpoCardboardCompendium');

export type Row = number; // printing row: stable and append-only across patches
export type Card = Record<string, any>; // Scryfall card JSON
export interface EngineInfo {
  printings: number;
  oracleCards: number;
  datasetSha256: string;
  index: 'coqs' | 'built' | 'delta' | 'stale';
  sets: Record<string, number>;
}
export interface SearchOptions {
  limit?: number;
  offset?: number;
  scope?: string; // name from registerSet
  cards?: boolean; // include card JSON for the returned rows
}
export interface SearchResult {
  total: number;
  rows: Row[];
  cards?: Card[];
}

async function call<T>(method: string, args: object = {}): Promise<T> {
  const r = JSON.parse(await Native.call(method, JSON.stringify(args)));
  if (r.error !== undefined) throw new Error(r.error);
  return r.ok as T;
}

/** Paths are plain filesystem paths (not file:// URIs). `coqs` is created/refreshed if missing or stale. */
export const open = (coqd: string, coqs?: string) => call<EngineInfo>('open', { coqd, coqs });
export const close = () => call<null>('close');
export const info = () => call<EngineInfo>('info');
export const search = (query: string, o: SearchOptions = {}) => call<SearchResult>('search', { query, ...o });
export const getCards = (rows: Row[]) => call<(Card | null)[]>('getCards', { rows });
export const getByScryfallId = (id: string) => call<Card | null>('getByScryfallId', { id });
/** Scope searches to your own cards (collection, a tag, a deck): register Scryfall ids under a name. */
export const registerSet = (name: string, ids: string[]) =>
  call<{ resolved: number; unknown: number }>('registerSet', { name, ids });
/** Apply a dataset patch (.coqp) in place; the index is stale until applyIndexDelta or the next search. */
export const applyPatch = (path: string) => call<EngineInfo>('applyPatch', { path });
/** Apply the matching index delta (.coqsd) after applyPatch. If it fails, the index is rebuilt on demand. */
export const applyIndexDelta = (path: string) => call<EngineInfo>('applyIndexDelta', { path });
