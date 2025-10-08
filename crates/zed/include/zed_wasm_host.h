// Minimal C ABI for Zed-hosted Wasmtime instances (core Wasm, envelope bridge)
#pragma once
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef uint64_t zed_wasm_handle;
typedef void (*zed_host_call_cb)(
  const char* service, size_t service_len,
  const char* method, size_t method_len,
  const char* payload_json, size_t payload_len,
  void* user_data
);

// Create a Wasm instance from the given module bytes. Defines the host import
// `zedline.host_send(ptr:i32, len:i32)` which forwards to `cb(bytes,len,user_data)`.
// Returns a non-zero handle on success, or 0 on failure.
zed_wasm_handle zed_wasm_instance_create(
  const uint8_t* wasm_ptr,
  size_t wasm_len,
  zed_host_call_cb cb,
  void* user_data
);

// Destroy a previously-created instance.
void zed_wasm_instance_destroy(zed_wasm_handle handle);

// Send an envelope to the guest by calling an exported `guest_recv(ptr,len)` function.
// Not yet implemented (returns non-zero on failure). Subject to change once alloc is standardized.
int zed_wasm_instance_guest_recv_response(
  zed_wasm_handle handle,
  int ok,
  const char* payload_json,
  size_t payload_len,
  const char* error_ptr,
  size_t error_len
);

#ifdef __cplusplus
} // extern "C"
#endif
