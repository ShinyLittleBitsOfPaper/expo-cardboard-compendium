// Production SyncDeps: expo-file-system for downloads and small text files, the engine's own file helpers for the rest.
// NOT verified on a device yet: the expo-file-system calls below follow the SDK 54+ File/Directory API, and the streamed
// download (expo/fetch + File.open().writeBytes) is new in 0.0.4; if it fails before the first byte it falls back to downloadFileAsync.

import { fetch as expoFetch } from 'expo/fetch';
import { Directory, File } from 'expo-file-system';
import { call } from '../native';
import type { SyncDeps } from './run';

const uri = (p: string) => (p.startsWith('file://') ? p : `file://${p}`);

export const nativeDeps: SyncDeps = {
  async fetchJson(url) {
    const r = await fetch(url, { headers: { 'Cache-Control': 'no-cache' } });
    if (!r.ok) throw new Error(`${url}: HTTP ${r.status}`);
    return r.json();
  },
  async download(url, destPath, onBytes) {
    const f = new File(uri(destPath));
    if (f.exists) f.delete();
    // Stream through expo/fetch so progress can be reported (File.downloadFileAsync has no progress callback). Any
    // failure of the streaming path before the first byte lands falls back to the plain download (no progress).
    if (onBytes) {
      let wrote = 0;
      try {
        const r = await expoFetch(url);
        if (!r.ok) throw new Error(`${url}: HTTP ${r.status}`);
        const reader = r.body?.getReader();
        if (!reader) throw new Error('no response body stream');
        f.create({ overwrite: true });
        const h = f.open();
        try {
          for (;;) {
            const { done, value } = await reader.read();
            if (done) break;
            h.writeBytes(value);
            wrote += value.byteLength;
            onBytes(wrote);
          }
        } finally {
          h.close();
        }
        return;
      } catch (e) {
        if (wrote > 0) throw e; // mid-stream failure: let the caller's fallback handle it
        if (f.exists) f.delete();
      }
    }
    await File.downloadFileAsync(url, f);
  },
  async readText(path) {
    const f = new File(uri(path));
    return f.exists ? f.text() : null;
  },
  async writeText(path, text) {
    const f = new File(uri(path));
    f.create({ overwrite: true });
    f.write(text);
  },
  async mkdirs(dir) {
    new Directory(uri(dir)).create({ intermediates: true, idempotent: true });
  },
  engine: {
    open: (coqd, coqi) => call('open', { coqd, coqi }),
    info: () => call('info'),
    applyPatch: (path) => call('applyPatch', { path }),
    applyIndexDelta: (path) => call('applyIndexDelta', { path }),
    sha256File: (path) => call('sha256File', { path }),
    unzstFile: (path, out) => call('unzstFile', { path, out }),
    patchFile: (path, patch, out) => call('patchFile', { path, patch, out }),
    attachOverlay: (id, path) => call('attachOverlay', { id, path }),
    moveFile: (from, to) => call('moveFile', { from, to }),
    removeFile: (path) => call('removeFile', { path }),
  },
};
