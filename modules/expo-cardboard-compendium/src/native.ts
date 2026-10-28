import { requireNativeModule } from 'expo-modules-core';

// One native entry point: call(method, argsJson) -> '{"ok":...}' | '{"error":"..."}'.
const Native = requireNativeModule<{ call(method: string, args: string): Promise<string> }>('ExpoCardboardCompendium');

export async function call<T>(method: string, args: object = {}): Promise<T> {
  const r = JSON.parse(await Native.call(method, JSON.stringify(args)));
  if (r.error !== undefined) throw new Error(r.error);
  return r.ok as T;
}
