use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use parking_lot::Mutex;
use once_cell::sync::Lazy;
use wasmtime::{Engine, Store, Config};
use wasmtime::component::{Component, Linker};

#[repr(C)]
pub type ZedWasmHandle = u64;

type HostSendCb = extern "C" fn(*const u8, usize, *mut std::ffi::c_void);

struct HostState {
    host_send_cb: Option<(HostSendCb, *mut std::ffi::c_void)>,
}

struct Entry {
    store: Store<HostState>,
    world: engine_bindings::Engine,
}

static ENGINE: Lazy<Engine> = Lazy::new(|| {
    let mut cfg = Config::new();
    // Best-effort enable proposals commonly needed for Kotlin/Wasm
    let _ = cfg.wasm_gc(true);
    let _ = cfg.wasm_exceptions(true);
    Engine::new(&cfg).expect("wasmtime engine")
});

static NEXT: AtomicU64 = AtomicU64::new(1);
static INSTANCES: Lazy<Mutex<HashMap<u64, Entry>>> = Lazy::new(|| Mutex::new(HashMap::new()));

// ---------------- WIT/component bindings ----------------
mod engine_bindings {
    wasmtime::component::bindgen!({
        path: "src/zedline_wit",
        world: "engine",
        async: false,
        trappable_imports: true,
    });
}

use engine_bindings as wit;

impl wit::host::Host for HostState {
    fn log(&mut self, msg: String) -> wasmtime::Result<()> {
        log::info!(target: "zed_wasm_host", "guest log: {}", msg);
        Ok(())
    }
    fn host_send_call(&mut self, call: wit::Call) -> wasmtime::Result<()> {
        if let Some((f, ud)) = self.host_send_cb {
            unsafe {
                f(
                    call.service.as_ptr(), call.service.len(),
                    call.method.as_ptr(), call.method.len(),
                    call.payload_json.as_ptr(), call.payload_json.len(),
                    ud,
                );
            }
        }
        Ok(())
    }
}

#[no_mangle]
pub extern "C" fn zed_wasm_instance_create(
    wasm_ptr: *const u8,
    wasm_len: usize,
    cb: Option<HostSendCb>,
    user_data: *mut std::ffi::c_void,
) -> ZedWasmHandle {
    if wasm_ptr.is_null() || wasm_len == 0 { return 0; }
    let bytes = unsafe { std::slice::from_raw_parts(wasm_ptr, wasm_len) };
    let component = match Component::from_binary(&ENGINE, bytes) {
        Ok(c) => c,
        Err(e) => { log::warn!("component load failed: {e}"); return 0; }
    };

    let mut linker: Linker<HostState> = Linker::new(&ENGINE);
    let _ = wit::Engine::add_to_linker(&mut linker, |state| state);

    let host_cb = cb.map(|f| (f, user_data));
    let mut store = Store::new(&ENGINE, HostState { host_send_cb: host_cb });
    let world = match wit::Engine::instantiate(&mut store, &component, &linker) {
        Ok(w) => w,
        Err(e) => { log::warn!("instantiate failed: {e}"); return 0; }
    };

    let handle = NEXT.fetch_add(1, Ordering::Relaxed);
    INSTANCES.lock().insert(handle, Entry { store, world });
    handle
}

#[no_mangle]
pub extern "C" fn zed_wasm_instance_destroy(handle: ZedWasmHandle) {
    let _ = INSTANCES.lock().remove(&handle);
}

#[no_mangle]
pub extern "C" fn zed_wasm_instance_guest_recv_response(
    handle: ZedWasmHandle,
    ok: i32,
    payload_json_ptr: *const u8,
    payload_json_len: usize,
    error_ptr: *const u8,
    error_len: usize,
) -> i32 {
    if payload_json_ptr.is_null() { return 1; }
    let payload = unsafe { std::slice::from_raw_parts(payload_json_ptr, payload_json_len) };
    let err_opt = if error_ptr.is_null() || error_len == 0 {
        None
    } else {
        Some(unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(error_ptr, error_len)) }.to_string())
    };
    let mut table = INSTANCES.lock();
    if let Some(entry) = table.get_mut(&handle) {
            let resp = wit::Response { ok: ok != 0, payload_json: String::from_utf8_lossy(payload).to_string(), error: err_opt };
        match entry.world.call_guest_recv_response(&mut entry.store, resp) {
            Ok(()) => 0,
            Err(e) => { log::warn!("guest-recv failed: {e}"); 1 }
        }
    } else {
        1
    }
}
