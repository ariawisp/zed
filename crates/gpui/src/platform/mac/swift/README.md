GPUIAppKit Swift Skeleton

This Swift Package builds a tiny dynamic library that owns minimal AppKit glue
and exposes a C ABI for Rust to call. It is a proof-of-shape to migrate the
macOS platform layer to Swift while keeping GPUI core + Metal in Rust.

Contents
- C header: `GPUIFFI/include/gpui_macos_ffi.h` (callback table + a few commands)
- Swift target: `GPUIAppKit` exporting `gpui_macos_*` functions

Build
- Prereqs: Xcode toolchain, Swift 6.2+, macOS v26+
- Build the dylib:
  - `cd crates/gpui/src/platform/mac/swift`
  - `swift build -c release`
  - Output: `.build/release/libGPUIAppKit.dylib`

Run with GPUI (optional)
- Set env var so gpui can `dlopen` the dylib:
  - `export GPUI_SWIFT_LIB="$(pwd)/.build/release/libGPUIAppKit.dylib"`
- Build gpui with feature flag:
  - `cargo run -p gpui --features macos-swift` (or run examples)
  - Note: With `macos-swift` enabled, GPUI uses the Swift path exclusively. If the dylib is not found, the process will exit with an error.

Notes
- `gpui_macos_run()` in this skeleton triggers lifecycle callbacks but does not
  enter the AppKit run loop. It’s deliberate to let you try wiring without
  blocking. Flip to `NSApp.run()` once more FFI is in place.
- Window creation exposes a `CAMetalLayer*` pointer, which Rust can consume
  without extra copies for rendering.
