import { nativeDeps } from './nativeDeps';
import { runSync, type SyncOptions, type SyncResult } from './run';

let running: Promise<SyncResult> | null = null;

/**
 * Bring the local data up to date with the published manifest: patches when the chain is short and intact, full files
 * otherwise. Concurrent calls share one run. When `changed` is true, (re)open the engine with `dataPaths(dir)` and
 * re-register your sets (rows are stable across patches, but a full download replaces the files).
 * Offline or failing sync never damages the existing data: files are verified before they replace anything.
 */
export function sync(o: SyncOptions): Promise<SyncResult> {
  running ??= runSync(nativeDeps, o).finally(() => {
    running = null;
  });
  return running;
}
