Examples

- Minimal guest Wasm component (typed WIT, no JSON):
  - Location: `examples/guest-hello-redwood` (outside the `zed/` workspace).
  - Build (requires `wasm32-wasip2` and `wasm-tools`):
    - `rustup target add wasm32-wasip2`
    - `./scripts/dev/build-guest.sh`
    - Output component: `examples/guest-hello-redwood/modules/app.wasm`
    - Note: The script wraps core Wasm via `wasm-tools component new` if needed; if the artifact is already a component it’s copied as-is.
  - Run:
    - `./scripts/dev/hello-redwood.sh` (builds guest and runs Zed)

Notes
- The guest calls host `redwood-create-view` and `redwood-apply` (typed WIT) with a minimal frame
  (create text, set text, attach to root). The preview bridge renders via GPUI.
- Logs show Wasm instance creation and `redwood.apply` being invoked.
