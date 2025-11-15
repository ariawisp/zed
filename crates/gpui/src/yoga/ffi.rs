//! CXX bridge to Yoga layout engine.
//!
//! This module provides Rust bindings to Facebook's Yoga flexbox layout engine
//! via C++ FFI using the CXX crate.
#![allow(unused_unsafe)]

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};

#[cxx::bridge(namespace = "gpui::yoga")]
mod ffi {
    #[derive(Debug, Copy, Clone, Eq, PartialEq)]
    pub struct YogaNodeHandle {
        pub raw: u64,
    }

    #[derive(Debug, Copy, Clone, Eq, PartialEq)]
    pub enum YogaValueUnit {
        Undefined = 0,
        Point = 1,
        Percent = 2,
        Auto = 3,
    }

    #[derive(Debug, Copy, Clone)]
    pub struct YogaValue {
        pub value: f32,
        pub unit: YogaValueUnit,
    }

    #[derive(Debug, Copy, Clone)]
    pub struct YogaEdges {
        pub left: YogaValue,
        pub top: YogaValue,
        pub right: YogaValue,
        pub bottom: YogaValue,
    }

    #[derive(Debug, Copy, Clone)]
    pub struct YogaStyleSize {
        pub width: YogaValue,
        pub height: YogaValue,
    }

    #[derive(Debug, Copy, Clone, Eq, PartialEq)]
    pub enum YogaDisplay {
        Flex = 0,
        None = 1,
    }

    #[derive(Debug, Copy, Clone, Eq, PartialEq)]
    pub enum YogaPositionType {
        Relative = 0,
        Absolute = 1,
    }

    #[derive(Debug, Copy, Clone, Eq, PartialEq)]
    pub enum YogaOverflow {
        Visible = 0,
        Hidden = 1,
        Scroll = 2,
    }

    #[derive(Debug, Copy, Clone, Eq, PartialEq)]
    pub enum YogaFlexDirection {
        Column = 0,
        ColumnReverse = 1,
        Row = 2,
        RowReverse = 3,
    }

    #[derive(Debug, Copy, Clone, Eq, PartialEq)]
    pub enum YogaWrap {
        NoWrap = 0,
        Wrap = 1,
        WrapReverse = 2,
    }

    #[derive(Debug, Copy, Clone, Eq, PartialEq)]
    pub enum YogaAlign {
        Auto = 0,
        FlexStart = 1,
        Center = 2,
        FlexEnd = 3,
        Stretch = 4,
        Baseline = 5,
        SpaceBetween = 6,
        SpaceAround = 7,
    }

    #[derive(Debug, Copy, Clone, Eq, PartialEq)]
    pub enum YogaJustify {
        FlexStart = 0,
        Center = 1,
        FlexEnd = 2,
        SpaceBetween = 3,
        SpaceAround = 4,
        SpaceEvenly = 5,
    }

    #[derive(Debug, Copy, Clone)]
    pub struct YogaStyle {
        pub display: YogaDisplay,
        pub position_type: YogaPositionType,
        pub overflow: YogaOverflow,
        pub flex_direction: YogaFlexDirection,
        pub flex_wrap: YogaWrap,
        pub justify_content: YogaJustify,
        pub align_items: YogaAlign,
        pub align_self: YogaAlign,
        pub align_content: YogaAlign,
        pub margin: YogaEdges,
        pub padding: YogaEdges,
        pub border: YogaEdges,
        pub inset: YogaEdges,
        pub size: YogaStyleSize,
        pub min_size: YogaStyleSize,
        pub max_size: YogaStyleSize,
        pub gap: YogaStyleSize,
        pub flex_basis: YogaValue,
        pub flex_grow: f32,
        pub flex_shrink: f32,
        pub aspect_ratio: f32,
        pub has_flex_grow: bool,
        pub has_flex_shrink: bool,
        pub has_flex_basis: bool,
        pub has_aspect_ratio: bool,
    }

    #[derive(Debug, Copy, Clone, Eq, PartialEq)]
    pub enum YogaAvailableDimensionKind {
        Undefined = 0,
        MinContent = 1,
        MaxContent = 2,
        Definite = 3,
    }

    #[derive(Debug, Copy, Clone)]
    pub struct YogaAvailableDimension {
        pub kind: YogaAvailableDimensionKind,
        pub value: f32,
    }

    #[derive(Debug, Copy, Clone)]
    pub struct YogaAvailableSize {
        pub width: YogaAvailableDimension,
        pub height: YogaAvailableDimension,
    }

    #[derive(Debug, Copy, Clone, Eq, PartialEq)]
    pub enum YogaMeasureMode {
        Undefined = 0,
        Exactly = 1,
        AtMost = 2,
    }

    #[derive(Debug, Copy, Clone)]
    pub struct YogaMeasureInput {
        pub value: f32,
        pub mode: YogaMeasureMode,
    }

    #[derive(Debug, Copy, Clone, Default)]
    pub struct YogaSize {
        pub width: f32,
        pub height: f32,
    }

    #[derive(Debug, Copy, Clone, Default)]
    pub struct YogaLayout {
        pub left: f32,
        pub top: f32,
        pub width: f32,
        pub height: f32,
    }

    unsafe extern "C++" {
        include!("gpui/yoga_bridge/YogaBridge.h");

        fn yoga_create_node() -> YogaNodeHandle;
        fn yoga_free_node(node: YogaNodeHandle);
        fn yoga_set_style(node: YogaNodeHandle, style: &YogaStyle);
        fn yoga_set_children(node: YogaNodeHandle, children: &[YogaNodeHandle]);
        fn yoga_mark_dirty(node: YogaNodeHandle);
        fn yoga_set_measure(node: YogaNodeHandle, measure_id: u64);
        fn yoga_clear_measure(node: YogaNodeHandle);
        fn yoga_calculate_layout(node: YogaNodeHandle, size: &YogaAvailableSize);
        fn yoga_layout(node: YogaNodeHandle) -> YogaLayout;
    }

    extern "Rust" {
        fn yoga_measure(
            measure_id: u64,
            width: &YogaMeasureInput,
            height: &YogaMeasureInput,
        ) -> YogaSize;
        fn yoga_drop_measure(measure_id: u64);
    }
}

pub use ffi::{
    YogaAlign, YogaAvailableDimension, YogaAvailableDimensionKind, YogaAvailableSize, YogaDisplay,
    YogaEdges, YogaFlexDirection, YogaJustify, YogaLayout, YogaMeasureInput, YogaMeasureMode,
    YogaNodeHandle, YogaOverflow, YogaPositionType, YogaSize, YogaStyle, YogaStyleSize, YogaValue,
    YogaValueUnit, YogaWrap,
};

type MeasureCallback =
    Box<dyn FnMut(YogaMeasureInput, YogaMeasureInput) -> YogaSize + Send + 'static>;

static NEXT_MEASURE_ID: AtomicU64 = AtomicU64::new(1);
static MEASURE_CALLBACKS: Lazy<Mutex<HashMap<u64, MeasureCallback>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Create a new Yoga node.
pub fn create_node() -> YogaNodeHandle {
    unsafe { ffi::yoga_create_node() }
}

/// Free a Yoga node and all of its descendants.
pub fn free_node(node: YogaNodeHandle) {
    unsafe { ffi::yoga_free_node(node) }
}

/// Apply an updated style to the node.
pub fn set_style(node: YogaNodeHandle, style: &YogaStyle) {
    unsafe { ffi::yoga_set_style(node, style) }
}

/// Replace the node's children with the provided handles.
pub fn set_children(node: YogaNodeHandle, children: &[YogaNodeHandle]) {
    unsafe { ffi::yoga_set_children(node, children) }
}

/// Mark a node as dirty.
pub fn mark_dirty(node: YogaNodeHandle) {
    unsafe { ffi::yoga_mark_dirty(node) }
}

/// Handle returned by `set_measure` to track measure callback registration.
pub struct YogaMeasureHandle(u64);

impl Drop for YogaMeasureHandle {
    fn drop(&mut self) {
        yoga_drop_measure(self.0);
    }
}

/// Register a measure callback for the node. Returns a handle for bookkeeping.
pub fn set_measure<F>(node: YogaNodeHandle, callback: F) -> YogaMeasureHandle
where
    F: FnMut(YogaMeasureInput, YogaMeasureInput) -> YogaSize + Send + 'static,
{
    let id = NEXT_MEASURE_ID.fetch_add(1, Ordering::Relaxed);
    {
        let mut callbacks = MEASURE_CALLBACKS.lock();
        callbacks.insert(id, Box::new(callback));
    }
    unsafe { ffi::yoga_set_measure(node, id) }
    YogaMeasureHandle(id)
}

/// Calculate layout for the node and its descendants.
pub fn calculate_layout(node: YogaNodeHandle, available: &YogaAvailableSize) {
    unsafe { ffi::yoga_calculate_layout(node, available) }
}

/// Get the computed layout for a node.
pub fn layout(node: YogaNodeHandle) -> YogaLayout {
    unsafe { ffi::yoga_layout(node) }
}

// C++ → Rust callback bridge

#[unsafe(no_mangle)]
pub fn yoga_measure(
    measure_id: u64,
    width: &YogaMeasureInput,
    height: &YogaMeasureInput,
) -> YogaSize {
    let mut callbacks = MEASURE_CALLBACKS.lock();
    if let Some(callback) = callbacks.get_mut(&measure_id) {
        callback(*width, *height)
    } else {
        YogaSize::default()
    }
}

#[unsafe(no_mangle)]
pub fn yoga_drop_measure(measure_id: u64) {
    let mut callbacks = MEASURE_CALLBACKS.lock();
    callbacks.remove(&measure_id);
}
