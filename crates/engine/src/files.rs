//! Stateless file helpers for the sync layer (no open engine needed): streaming SHA-256 and zstd decompression to a
//! file. They exist so the TypeScript side can verify downloads and unpack the `.coqi.zst` without a JS crypto or
//! zstd dependency, and without pulling 20-30 MB files through the JS bridge.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// SHA-256 of a file, streamed. Returns (hex, size).
pub fn sha256_file(path: &Path) -> Result<(String, u64), String> {
    let mut f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (mut h, mut n, mut buf) = (Sha256::new(), 0u64, vec![0u8; 1 << 16]);
    loop {
        let k = f.read(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
        if k == 0 {
            break;
        }
        h.update(&buf[..k]);
        n += k as u64;
    }
    Ok((hex(&h.finalize()), n))
}

/// Decompress a zstd file to `out` (written to `<out>.part`, then renamed). Returns (sha256 hex, size) of the output.
pub fn unzst_file(path: &Path, out: &Path) -> Result<(String, u64), String> {
    let src = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut dec = zstd::Decoder::new(src).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut part = out.as_os_str().to_owned();
    part.push(".part");
    let part = std::path::PathBuf::from(part);
    let res = (|| -> Result<(String, u64), String> {
        let mut dst = File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
        let (mut h, mut n, mut buf) = (Sha256::new(), 0u64, vec![0u8; 1 << 16]);
        loop {
            let k = dec.read(&mut buf).map_err(|e| format!("{}: not valid zstd: {e}", path.display()))?;
            if k == 0 {
                break;
            }
            h.update(&buf[..k]);
            dst.write_all(&buf[..k]).map_err(|e| format!("{}: {e}", part.display()))?;
            n += k as u64;
        }
        dst.sync_all().map_err(|e| e.to_string())?;
        Ok((hex(&h.finalize()), n))
    })();
    match res {
        Ok(r) => {
            std::fs::rename(&part, out).map_err(|e| format!("{}: {e}", out.display()))?;
            Ok(r)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

/// Atomic replace of `to` by `from` (same filesystem). The sync layer uses this to swap in a verified download.
pub fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::rename(from, to).map_err(|e| format!("{} -> {}: {e}", from.display(), to.display()))
}

/// `sha256File {path}`, `unzstFile {path, out}`, `moveFile {from, to}` (atomic replace), `removeFile {path}` (missing is fine); `None` when `method` is not a file helper.
pub fn call(method: &str, a: &Value) -> Option<Result<Value, String>> {
    let s = |k: &str| a.get(k).and_then(Value::as_str).ok_or(format!("missing {k}"));
    match method {
        "sha256File" => Some(s("path").and_then(|p| sha256_file(Path::new(p))).map(|(h, n)| json!({"sha256": h, "size": n}))),
        "unzstFile" => Some(
            s("path").and_then(|p| s("out").and_then(|o| unzst_file(Path::new(p), Path::new(o)))).map(|(h, n)| json!({"sha256": h, "size": n})),
        ),
        "moveFile" => Some(s("from").and_then(|f| s("to").and_then(|t| move_file(Path::new(f), Path::new(t)))).map(|_| Value::Null)),
        "removeFile" => Some(s("path").and_then(|p| match std::fs::remove_file(p) {
            Ok(()) => Ok(Value::Null),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
            Err(e) => Err(format!("{p}: {e}")),
        })),
        _ => None,
    }
}
