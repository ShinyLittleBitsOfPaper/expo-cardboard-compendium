import { requireNativeModule } from 'expo-modules-core';

// One native entry point: call(method, argsJson) -> '{"ok":...}' | '{"error":"..."}'.
const Native = requireNativeModule<{ call(method: string, args: string): Promise<string> }>('ExpoCardboardCompendium');

/**
 * A native call that waited long for the engine lock or ran long under it. The engine serialises ALL calls behind one lock, so a slow call
 * (search, open, price lookup...) stalls every other lookup; `holder` is the method that held the lock when this call queued (a best guess at
 * what it waited for). Only slow calls are reported (waited >= 100 ms or ran >= 250 ms).
 */
export type SlowCall = { method: string; waitMs: number; runMs: number; holder: string };
let slowCallObserver: ((c: SlowCall) => void) | null = null;
/** Register (or clear with null) the one listener for slow calls, e.g. to write them to the app's debug log. */
export function setSlowCallObserver(fn: ((c: SlowCall) => void) | null): void {
  slowCallObserver = fn;
}

export async function call<T>(method: string, args: object = {}): Promise<T> {
  const r = JSON.parse(await Native.call(method, JSON.stringify(args)));
  if (r.t && slowCallObserver) {
    try {
      slowCallObserver({ method: r.t.method ?? method, waitMs: r.t.wait, runMs: r.t.run, holder: r.t.holder ?? '' });
    } catch {
      // an observer must never break a call
    }
  }
  if (r.error !== undefined) throw new Error(r.error);
  return r.ok as T;
}
