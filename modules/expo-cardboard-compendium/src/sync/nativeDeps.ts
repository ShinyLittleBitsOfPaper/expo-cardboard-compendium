// Production SyncDeps: expo-file-system for downloads and small text files, the engine's own file helpers for the rest.
// NOT verified on a device yet: the expo-file-system calls below follow the SDK 54+ File/Directory API.

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
  async download(url, destPath) {
    const f = new File(uri(destPath));
    if (f.exists) f.delete();
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
    open: (coqd, coqs) => call('open', { coqd, coqs }),
    info: () => call('info'),
    applyPatch: (path) => call('applyPatch', { path }),
    applyIndexDelta: (path) => call('applyIndexDelta', { path }),
    sha256File: (path) => call('sha256File', { path }),
    unzstFile: (path, out) => call('unzstFile', { path, out }),
    moveFile: (from, to) => call('moveFile', { from, to }),
    removeFile: (path) => call('removeFile', { path }),
  },
};
