Zed Redwood Host Vendor Artifacts

This directory is used during local development to provide Zed with the
Kotlin/Native Redwood host library and C header so Zed can statically link
the preview host bridge.

Usage (macOS ARM64)
- Build the static library in the redwood-host-gpui module:
  - From the `redwood/` directory: `./gradlew :redwood-host-gpui:linkReleaseStaticMacosArm64`
  - Optionally export to this directory: `./gradlew :redwood-host-gpui:exportToZedVendor`
- Run Zed with the vendor directory on the link search path:
  - In the workspace root: `REDWOOD_HOST_LIB_DIR=$(pwd)/zed/vendor/redwood_host/macos-arm64 cargo run -p zed`

Notes
- The `exportToZedVendor` task copies both `libredwood_host_gpui.a` and
  `redwood_host.h` into this directory.
- For other platforms, adjust the target and subdirectory naming accordingly
  (e.g., `macos-x64`, `linux-x64`). Ensure `REDWOOD_HOST_LIB_DIR` points at the
  directory containing the static library file.

