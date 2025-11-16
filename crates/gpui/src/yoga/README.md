# GPUI Yoga Layout Engine

⚠️ **EXPERIMENTAL - NOT CURRENTLY USED**

This module implements a Yoga-based layout engine for GPUI, intended for **Scenario C** where GPUI owns the root of the component tree and embeds React Native views.

## Status

- ✅ **Implementation Complete**: YogaLayoutEngine fully implements LayoutEngine trait
- ✅ **Compiles Successfully**: Can be enabled with `yoga` feature flag
- ❌ **Not Currently Used**: react-native-gpui uses Fabric's Yoga instance instead (Scenario B)

## Use Cases

### Scenario C (Future - GPUI Root)
```rust
// GPUI owns root, embeds React Native
div()
  .child(editor())  // Native GPUI
  .child(react_native_panel())  // RN embedded
```

In this scenario:
- GPUI runs YogaLayoutEngine to compute layout for entire tree
- React Native views register as Yoga nodes in GPUI's tree
- Single Yoga instance owned by GPUI

### NOT for Scenario B (Current - React Native Root)
```jsx
// React Native owns root, embeds GPUI components
<View>
  <Text>RN Text</Text>
  <NativeButton />  {/* GPUI component */}
</View>
```

In this scenario:
- Fabric owns Yoga instance and computes layout
- GPUI components should register as Fabric components
- No YogaLayoutEngine needed (see react-native-gpui Fabric integration)

## Implementation

- **Engine**: `src/yoga/engine.rs` - YogaLayoutEngine implementing LayoutEngine trait
- **FFI**: `src/yoga/ffi.rs` - Rust↔C++ bindings to React Native's bundled Yoga
- **Style Conversion**: `src/yoga/style_conversion.rs` - GPUI Style → Yoga mapping
- **C++ Bridge**: `yoga_bridge/YogaBridge.cpp` - C++ wrapper around Yoga C API

## When to Use

Enable this when:
1. Building a pure GPUI app that wants to embed React Native views
2. Integrating react-native-gpui into Zed (GPUI owns root)
3. You want consistent Yoga layout semantics in GPUI without React Native

Do NOT use this for standard react-native-gpui apps where React Native owns the root.

## Future Work

- Bidirectional integration: Allow RN views to be children of GPUI elements
- Measure callbacks for RN views in GPUI trees
- Testing with real mixed GPUI+RN component trees
