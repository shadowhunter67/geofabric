//! WebAssembly interface of the browser lowering: JSON in, JSON out, no wasm-bindgen (nothing but `rustup target add wasm32-unknown-unknown`).
//!
//! Request:  `{"plan": <PlanGraph JSON, IR 0.3>, "sources": {"<uri>": {"covering": ["bbox.xmin","bbox.ymin","bbox.xmax","bbox.ymax"] | null}}, "max_rows": 100000}`
//! Response: `{"ok": true, "sql": "...", "files": ["<uri>", ...]}` or `{"ok": false, "kind": "invalid"|"unsupported", "message": "..."}`.
//!
//! The pure function [`lower_browser_json`] is what the tests run natively; the `extern "C"` functions are the thin memory shim a page needs:
//! the page calls `gf_alloc`, writes the request, calls `gf_lower_browser`, reads the response from the returned (pointer << 32 | length) and
//! calls `gf_free` on both buffers.

use crate::browser::{lower_browser, BrowserSource};
use crate::LowerError;
use geo_ir::Plan;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
struct Request {
    plan: Plan,
    sources: BTreeMap<String, BrowserSource>,
    max_rows: u64,
}

fn failure(kind: &str, message: String) -> String {
    serde_json::json!({"ok": false, "kind": kind, "message": message}).to_string()
}

/// The whole interface as a pure function.
pub fn lower_browser_json(request: &str) -> String {
    let req: Request = match serde_json::from_str(request) {
        Ok(r) => r,
        Err(e) => return failure("invalid", format!("bad lowering request: {e}")),
    };
    match lower_browser(&req.plan, &req.sources, req.max_rows) {
        Ok((sql, files)) => serde_json::json!({"ok": true, "sql": sql, "files": files}).to_string(),
        Err(LowerError::Invalid(m)) => failure("invalid", m),
        Err(LowerError::Unsupported(m)) => failure("unsupported", m),
    }
}

/// Allocate `len` zeroed bytes the page can write into (an exactly-sized boxed slice, so freeing needs only the length).
#[no_mangle]
pub extern "C" fn gf_alloc(len: usize) -> *mut u8 {
    Box::into_raw(vec![0u8; len.max(1)].into_boxed_slice()) as *mut u8
}

/// Free a buffer from `gf_alloc` (`len` = the length that was asked for) or a response from `gf_lower_browser` (`len` = its length).
///
/// # Safety
/// `ptr`/`len` must be exactly what `gf_alloc`/`gf_lower_browser` handed out, and the buffer must not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn gf_free(ptr: *mut u8, len: usize) {
    drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len.max(1))));
}

/// Lower the request at `ptr..ptr+len`. Returns `(response_ptr << 32) | response_len` (wasm32); the response buffer's capacity equals
/// its length, so the page frees it with `gf_free(ptr, len)`.
///
/// # Safety
/// `ptr..ptr+len` must be a valid, initialised buffer owned by the caller.
#[no_mangle]
pub unsafe extern "C" fn gf_lower_browser(ptr: *const u8, len: usize) -> u64 {
    let input = std::slice::from_raw_parts(ptr, len);
    let out = match std::str::from_utf8(input) {
        Ok(s) => lower_browser_json(s),
        Err(_) => failure("invalid", "lowering request is not UTF-8".into()),
    };
    let bytes = out.into_bytes().into_boxed_slice();
    let l = bytes.len() as u64;
    let p = Box::into_raw(bytes) as *mut u8 as usize as u64;
    (p << 32) | l
}
