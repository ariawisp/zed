//! Yoga layout engine integration for GPUI.
//!
//! This module provides a Yoga-based implementation of GPUI's LayoutEngine trait,
//! allowing GPUI to use Facebook's Yoga flexbox layout algorithm instead of Taffy.
//!
//! ## Usage
//!
//! Enable the `yoga` feature in your Cargo.toml:
//!
//! ```toml
//! gpui = { version = "0.2", features = ["yoga"] }
//! ```
//!
//! When this feature is enabled, GPUI will use `YogaLayoutEngine` as the default
//! layout engine instead of `TaffyLayoutEngine`.
//!
//! ## Architecture
//!
//! - `ffi`: CXX-based FFI bindings to the C++ Yoga library
//! - `engine`: YogaLayoutEngine implementation of LayoutEngine trait
//! - `style_conversion`: Converts GPUI Style to Yoga's format
//!
//! ## Limitations
//!
//! - **No CSS Grid support**: Yoga only supports flexbox. Grid layouts are converted
//!   to flex with wrapping, which is lossy.
//! - **Single overflow value**: Yoga doesn't support independent x/y overflow

mod engine;
mod ffi;
mod style_conversion;

pub use engine::YogaLayoutEngine;
#[allow(unused_imports)]
pub use ffi::{
    YogaAlign, YogaAvailableDimension, YogaAvailableDimensionKind, YogaAvailableSize, YogaDisplay,
    YogaEdges, YogaFlexDirection, YogaJustify, YogaLayout, YogaMeasureInput, YogaMeasureMode,
    YogaNodeHandle, YogaOverflow, YogaPositionType, YogaSize, YogaStyle, YogaStyleSize, YogaValue,
    YogaValueUnit, YogaWrap, free_node, set_children,
};
