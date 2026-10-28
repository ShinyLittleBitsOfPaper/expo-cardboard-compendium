//! C ABI: one global engine, one JSON entry point. `coqe_call("open", {"coqd":..,"coqs":..})` first.
//! Returns a malloc'd UTF-8 JSON string `{"ok":...}` or `{"error":"..."}`; free it with `coqe_free`.

use crate::Engine;
use serde_json::{json, Value};
use std::ffi::{c_char, CStr, CString};
use std::path::Path;
use std::sync::Mutex;

static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);

fn dispatch(method: &str, args: &str) -> Result<Value, String> {
    let a: Value = if args.is_empty() { json!({}) } else { serde_json::from_str(args).map_err(|e| format!("bad args: {e}"))? };
    if let Some(r) = crate::files::call(method, &a) {
        return r; // stateless helpers: no engine (or its lock) needed
    }
    let mut g = ENGINE.lock().unwrap_or_else(|p| p.into_inner());
    match method {
        "open" => {
            let coqd = a.get("coqd").and_then(Value::as_str).ok_or("missing coqd")?;
            let coqs = a.get("coqs").and_then(Value::as_str).map(Path::new);
            let e = Engine::open(Path::new(coqd), coqs)?;
            let info = e.info();
            *g = Some(e);
            Ok(info)
        }
        "close" => {
            *g = None;
            Ok(Value::Null)
        }
        _ => g.as_mut().ok_or("engine not open")?.call(method, &a),
    }
}

pub fn call_json(method: &str, args: &str) -> String {
    let r = std::panic::catch_unwind(|| dispatch(method, args)).unwrap_or_else(|_| Err("engine panicked".into()));
    match r {
        Ok(v) => json!({"ok": v}),
        Err(e) => json!({"error": e}),
    }
    .to_string()
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
