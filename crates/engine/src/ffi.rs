//! C ABI: one global engine, one JSON entry point. `coqe_call("open", {"coqd":..,"coqi":..})` first.
//! Returns a malloc'd UTF-8 JSON string `{"ok":...}` or `{"error":"..."}`; free it with `coqe_free`.

use crate::Engine;
use serde_json::{json, Value};
use std::ffi::{c_char, CStr, CString};
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);
/// Method of the call that currently holds `ENGINE` (empty when idle). Read by callers when they queue, so a slow call can say who it waited for.
static HOLDER: Mutex<String> = Mutex::new(String::new());

/// A call that waited this long for the engine lock, or ran this long under it, gets a `t` entry in its reply (see `timing_json`).
const SLOW_WAIT_MS: u64 = 100;
const SLOW_RUN_MS: u64 = 250;

struct Timing {
    wait_ms: u64,
    run_ms: u64,
    holder: String,
}

/// The `t` object of a reply for a slow call, or None. `holder` is the method that held the lock when this call queued (a best guess at what it
/// waited for: another call may have taken over before it got the lock). Pure so it can be tested.
fn timing_json(method: &str, wait_ms: u64, run_ms: u64, holder: &str) -> Option<Value> {
    (wait_ms >= SLOW_WAIT_MS || run_ms >= SLOW_RUN_MS).then(|| json!({"method": method, "wait": wait_ms, "run": run_ms, "holder": holder}))
}

fn dispatch(method: &str, args: &str) -> (Result<Value, String>, Option<Timing>) {
    let a: Value = match if args.is_empty() { Ok(json!({})) } else { serde_json::from_str(args).map_err(|e| format!("bad args: {e}")) } {
        Ok(a) => a,
        Err(e) => return (Err(e), None),
    };
    if let Some(r) = crate::files::call(method, &a) {
        return (r, None); // stateless helpers: no engine (or its lock) needed
    }
    let holder = HOLDER.lock().map(|h| h.clone()).unwrap_or_default();
    let queued = Instant::now();
    let mut g = ENGINE.lock().unwrap_or_else(|p| p.into_inner());
    let wait_ms = queued.elapsed().as_millis() as u64;
    *HOLDER.lock().unwrap_or_else(|p| p.into_inner()) = method.to_string();
    let started = Instant::now();
    let r = (|| match method {
        "open" => {
            let coqd = a.get("coqd").and_then(Value::as_str).ok_or("missing coqd")?;
            let coqi = a.get("coqi").and_then(Value::as_str).map(Path::new);
            // Free the engine being replaced BEFORE building the new one: both used to be resident at once (old store + new file
            // + new store + index build), a memory peak right after every card-data sync. The lock is held for the whole open,
            // so no call can see the closed engine; if the new open fails the engine stays closed and the caller must re-open.
            *g = None;
            let e = Engine::open(Path::new(coqd), coqi)?;
            let info = e.info();
            *g = Some(e);
            Ok(info)
        }
        "close" => {
            *g = None;
            Ok(Value::Null)
        }
        _ => g.as_mut().ok_or("engine not open")?.call(method, &a),
    })();
    let run_ms = started.elapsed().as_millis() as u64;
    HOLDER.lock().unwrap_or_else(|p| p.into_inner()).clear();
    (r, Some(Timing { wait_ms, run_ms, holder }))
}

pub fn call_json(method: &str, args: &str) -> String {
    let (r, t) = std::panic::catch_unwind(|| dispatch(method, args)).unwrap_or_else(|_| (Err("engine panicked".into()), None));
    let mut out = match r {
        Ok(v) => json!({"ok": v}),
        Err(e) => json!({"error": e}),
    };
    if let Some(t) = t.and_then(|t| timing_json(method, t.wait_ms, t.run_ms, &t.holder)) {
        out["t"] = t;
    }
    out.to_string()
}

/// # Safety
/// `method` and `args` must be valid NUL-terminated UTF-8 strings (`args` may be NULL).
#[no_mangle]
pub unsafe extern "C" fn coqe_call(method: *const c_char, args: *const c_char) -> *mut c_char {
    let m = CStr::from_ptr(method).to_string_lossy();
    let a = if args.is_null() { "".into() } else { CStr::from_ptr(args).to_string_lossy() };
    CString::new(call_json(&m, &a)).unwrap_or_default().into_raw()
}

/// # Safety
/// `p` must come from `coqe_call` and be freed once.
#[no_mangle]
pub unsafe extern "C" fn coqe_free(p: *mut c_char) {
    if !p.is_null() {
        drop(CString::from_raw(p));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_slow_calls_report_timing() {
        assert!(timing_json("getByKeys", 5, 40, "").is_none());
        let t = timing_json("getByKeys", 20_000, 12, "search").unwrap();
        assert_eq!((t["wait"].clone(), t["run"].clone(), t["holder"].clone(), t["method"].clone()), (json!(20_000), json!(12), json!("search"), json!("getByKeys")));
        assert!(timing_json("search", 0, 900, "").is_some());
    }

    #[test]
    fn replies_keep_their_shape() {
        let r: Value = serde_json::from_str(&call_json("nope", "{}")).unwrap();
        assert_eq!(r["error"], "engine not open");
        assert!(r.get("t").is_none());
        let bad: Value = serde_json::from_str(&call_json("x", "{not json")).unwrap();
        assert!(bad["error"].as_str().unwrap().starts_with("bad args"));
    }
}
