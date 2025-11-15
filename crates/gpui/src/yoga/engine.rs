use super::ffi::{
    YogaAvailableDimension, YogaAvailableDimensionKind, YogaAvailableSize, YogaMeasureHandle,
    YogaMeasureInput, YogaMeasureMode, YogaNodeHandle, YogaSize, calculate_layout, clear_measure,
    create_node, free_node, layout, mark_dirty, set_children, set_measure, set_style,
};
use super::style_conversion::convert_style_to_yoga;
use crate::{
    App, AvailableSpace, Bounds, ExternalLayoutOverride, LayoutEngine, LayoutId, Pixels, Point,
    Size, Style, Window, layout::LayoutMeasureFn,
};
use stacksafe::internal;
use std::{
    any::Any,
    cell::RefCell,
    collections::{HashMap, HashSet},
};

// Thread-local context for providing Window and App to measure callbacks.
//
// This is necessary because Yoga's measure callbacks are raw C function pointers
// that cannot capture Rust closures. We use thread-local storage to provide
// the necessary context during layout computation.
thread_local! {
    static MEASURE_CONTEXT: RefCell<Option<MeasureContext>> = RefCell::new(None);
}

struct MeasureContext {
    window_ptr: *mut Window,
    app_ptr: *mut App,
    engine_ptr: *mut YogaLayoutEngine,
    scale_factor: f32,
}

unsafe impl Send for MeasureContext {}

/// Yoga-based layout engine implementing GPUI's LayoutEngine trait.
///
/// This engine uses Facebook's Yoga flexbox layout algorithm instead of Taffy,
/// providing identical layout semantics to React Native.
pub struct YogaLayoutEngine {
    /// Map from GPUI LayoutId to Yoga node handles
    nodes: HashMap<LayoutId, YogaNodeHandle>,

    /// Counter for generating unique LayoutIds
    next_id: u64,

    /// Computed bounds in window coordinates (after layout calculation)
    computed_bounds: HashMap<LayoutId, Bounds<Pixels>>,

    /// Track parent-child relationships for recursive bounds extraction
    children_map: HashMap<LayoutId, Vec<LayoutId>>,

    /// Track which nodes have measure callbacks
    measure_handles: HashMap<LayoutId, YogaMeasureHandle>,

    /// Track GPUI measure functions for nodes that need custom measurement
    measure_functions: HashMap<LayoutId, LayoutMeasureFn>,

    /// Store external bounds overrides (for React Native integration)
    external_bounds: HashMap<LayoutId, Bounds<Pixels>>,

    /// Style metadata tracked for overrides so RN tags can mirror Taffy
    external_styles: HashMap<LayoutId, Style>,
}

impl YogaLayoutEngine {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            next_id: 1,
            computed_bounds: HashMap::new(),
            children_map: HashMap::new(),
            measure_handles: HashMap::new(),
            measure_functions: HashMap::new(),
            external_bounds: HashMap::new(),
            external_styles: HashMap::new(),
        }
    }

    fn next_layout_id(&mut self) -> LayoutId {
        let id = LayoutId::from_raw(self.next_id);
        self.next_id += 1;
        id
    }

    fn allocate_node(
        &mut self,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
    ) -> (LayoutId, YogaNodeHandle) {
        let yoga_style = convert_style_to_yoga(&style, rem_size, scale_factor);
        let node = create_node();
        set_style(node, &yoga_style);
        let layout_id = self.next_layout_id();
        self.nodes.insert(layout_id, node);
        self.children_map.entry(layout_id).or_insert_with(Vec::new);
        (layout_id, node)
    }

    fn apply_children(&mut self, parent: LayoutId, children: &[LayoutId]) -> bool {
        let Some(&parent_node) = self.nodes.get(&parent) else {
            return false;
        };
        let mut child_nodes = Vec::with_capacity(children.len());
        for child_id in children {
            if let Some(&child_node) = self.nodes.get(child_id) {
                child_nodes.push(child_node);
            } else {
                return false;
            }
        }
        set_children(parent_node, &child_nodes);
        self.children_map.insert(parent, children.to_vec());
        true
    }

    /// Extract layout bounds recursively from Yoga's computed layout.
    ///
    /// This traverses the Yoga node tree and converts Yoga's local coordinates
    /// to window-absolute coordinates by accumulating parent offsets.
    fn extract_bounds_recursive(
        &mut self,
        id: LayoutId,
        parent_origin: Point<Pixels>,
        scale_factor: f32,
    ) {
        let Some(&node) = self.nodes.get(&id) else {
            return;
        };

        // Get Yoga's computed layout for this node
        let yoga_layout = layout(node);

        // Convert to GPUI bounds (local to parent)
        let local_bounds = Bounds {
            origin: Point {
                x: Pixels(yoga_layout.left / scale_factor),
                y: Pixels(yoga_layout.top / scale_factor),
            },
            size: Size {
                width: Pixels(yoga_layout.width / scale_factor),
                height: Pixels(yoga_layout.height / scale_factor),
            },
        };

        // Convert to window-absolute bounds
        let window_bounds = Bounds {
            origin: Point {
                x: parent_origin.x + local_bounds.origin.x,
                y: parent_origin.y + local_bounds.origin.y,
            },
            size: local_bounds.size,
        };

        self.computed_bounds.insert(id, window_bounds);

        // Recurse for children (clone to avoid borrow conflict)
        if let Some(children) = self.children_map.get(&id).cloned() {
            for child_id in children {
                self.extract_bounds_recursive(child_id, window_bounds.origin, scale_factor);
            }
        }
    }

    /// Create a Yoga measure callback that invokes the GPUI measure function.
    fn create_measure_callback(
        id: LayoutId,
    ) -> impl FnMut(YogaMeasureInput, YogaMeasureInput) -> YogaSize + Send + 'static {
        move |width: YogaMeasureInput, height: YogaMeasureInput| -> YogaSize {
            MEASURE_CONTEXT.with(|ctx| {
                let context = ctx.borrow();
                let Some(ref measure_ctx) = *context else {
                    log::warn!("Yoga measure callback invoked without context for {:?}", id);
                    return YogaSize::default();
                };

                // SAFETY: compute_layout installs a MeasureContext with valid pointers before
                // running yoga_calculate_layout and clears it afterwards.
                let window = unsafe { &mut *measure_ctx.window_ptr };
                let cx = unsafe { &mut *measure_ctx.app_ptr };
                let engine = unsafe { &mut *measure_ctx.engine_ptr };
                let Some(measure_fn) = engine.measure_functions.get_mut(&id) else {
                    log::warn!(
                        "Yoga measure callback missing registered function for {:?}",
                        id
                    );
                    return YogaSize::default();
                };

                let scale_factor = measure_ctx.scale_factor;
                let known_dimensions = Size {
                    width: yoga_input_to_known_dimension(width, scale_factor),
                    height: yoga_input_to_known_dimension(height, scale_factor),
                };
                let available_space = Size {
                    width: yoga_input_to_available_space(width, scale_factor),
                    height: yoga_input_to_available_space(height, scale_factor),
                };

                internal::with_protected(|| {
                    let measured = measure_fn(known_dimensions, available_space, window, cx);
                    YogaSize {
                        width: measured.width.0 * scale_factor,
                        height: measured.height.0 * scale_factor,
                    }
                })()
            })
        }
    }

    /// Allocate a standalone Yoga node that can be managed externally.
    pub fn create_external_node(
        &mut self,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
    ) -> LayoutId {
        let (layout_id, _) = self.allocate_node(style, rem_size, scale_factor);
        layout_id
    }

    /// Update the Yoga style for a node and mark it dirty.
    pub fn set_node_style(
        &mut self,
        layout_id: LayoutId,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
    ) -> bool {
        let Some(&node) = self.nodes.get(&layout_id) else {
            return false;
        };
        let yoga_style = convert_style_to_yoga(&style, rem_size, scale_factor);
        set_style(node, &yoga_style);
        mark_dirty(node);
        true
    }

    /// Replace the children of a node.
    pub fn set_node_children(&mut self, layout_id: LayoutId, children: &[LayoutId]) -> bool {
        if !self.apply_children(layout_id, children) {
            return false;
        }
        if let Some(&node) = self.nodes.get(&layout_id) {
            mark_dirty(node);
        }
        true
    }

    /// Attach or clear a custom measure callback for the node.
    pub fn set_node_measure(
        &mut self,
        layout_id: LayoutId,
        measure: Option<LayoutMeasureFn>,
    ) -> bool {
        let Some(&node) = self.nodes.get(&layout_id) else {
            return false;
        };

        if let Some(handle) = self.measure_handles.remove(&layout_id) {
            drop(handle);
            clear_measure(node);
        }
        self.measure_functions.remove(&layout_id);

        if let Some(measure_fn) = measure {
            self.measure_functions.insert(layout_id, measure_fn);
            let measure_callback = Self::create_measure_callback(layout_id);
            let measure_handle = set_measure(node, measure_callback);
            self.measure_handles.insert(layout_id, measure_handle);
        }

        mark_dirty(node);
        true
    }
}

impl LayoutEngine for YogaLayoutEngine {
    fn clear(&mut self) {
        let mut child_ids: HashSet<LayoutId> = HashSet::new();
        for children in self.children_map.values() {
            child_ids.extend(children.iter().copied());
        }
        for (id, node) in self.nodes.drain() {
            if child_ids.contains(&id) {
                continue;
            }
            free_node(node);
        }
        self.computed_bounds.clear();
        self.children_map.clear();
        self.measure_handles.clear();
        self.measure_functions.clear();
        self.external_bounds.clear();
        self.external_styles.clear();
        self.next_id = 1;
    }

    fn remove_node(&mut self, layout_id: LayoutId) {
        if let Some(node) = self.nodes.remove(&layout_id) {
            free_node(node);
            self.computed_bounds.remove(&layout_id);
            self.children_map.remove(&layout_id);
            self.measure_handles.remove(&layout_id);
            self.measure_functions.remove(&layout_id);
            self.external_bounds.remove(&layout_id);
            self.external_styles.remove(&layout_id);
        }
    }

    fn request_layout(
        &mut self,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
        children: &[LayoutId],
    ) -> LayoutId {
        let (layout_id, _) = self.allocate_node(style, rem_size, scale_factor);
        self.apply_children(layout_id, children);
        layout_id
    }

    fn request_measured_layout(
        &mut self,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
        measure: LayoutMeasureFn,
    ) -> LayoutId {
        let (layout_id, _) = self.allocate_node(style, rem_size, scale_factor);
        self.apply_children(layout_id, &[]);
        let _ = self.set_node_measure(layout_id, Some(measure));
        layout_id
    }

    fn compute_layout(
        &mut self,
        id: LayoutId,
        available_space: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(&node) = self.nodes.get(&id) else {
            return;
        };

        self.computed_bounds.clear();

        // Convert GPUI AvailableSpace to Yoga's format
        let yoga_available = YogaAvailableSize {
            width: match available_space.width {
                AvailableSpace::Definite(px) => YogaAvailableDimension {
                    kind: YogaAvailableDimensionKind::Definite,
                    value: px.0,
                },
                AvailableSpace::MinContent => YogaAvailableDimension {
                    kind: YogaAvailableDimensionKind::MinContent,
                    value: 0.0,
                },
                AvailableSpace::MaxContent => YogaAvailableDimension {
                    kind: YogaAvailableDimensionKind::MaxContent,
                    value: 0.0,
                },
            },
            height: match available_space.height {
                AvailableSpace::Definite(px) => YogaAvailableDimension {
                    kind: YogaAvailableDimensionKind::Definite,
                    value: px.0,
                },
                AvailableSpace::MinContent => YogaAvailableDimension {
                    kind: YogaAvailableDimensionKind::MinContent,
                    value: 0.0,
                },
                AvailableSpace::MaxContent => YogaAvailableDimension {
                    kind: YogaAvailableDimensionKind::MaxContent,
                    value: 0.0,
                },
            },
        };

        // Set up measure context for callbacks
        MEASURE_CONTEXT.with(|ctx| {
            *ctx.borrow_mut() = Some(MeasureContext {
                window_ptr: window as *mut Window,
                app_ptr: cx as *mut App,
                engine_ptr: self as *mut YogaLayoutEngine,
                scale_factor: window.scale_factor(),
            });
        });

        // Run Yoga layout computation
        calculate_layout(node, &yoga_available);

        // Clear measure context
        MEASURE_CONTEXT.with(|ctx| {
            *ctx.borrow_mut() = None;
        });

        // Extract bounds recursively, starting from origin (0, 0)
        let scale_factor = window.scale_factor();
        self.extract_bounds_recursive(id, Point::default(), scale_factor);

        // Apply external overrides if any
        for (layout_id, bounds) in &self.external_bounds {
            self.computed_bounds.insert(*layout_id, *bounds);
        }
    }

    fn layout_bounds(&mut self, id: LayoutId, _scale_factor: f32) -> Bounds<Pixels> {
        // Check external override first (for React Native integration)
        if let Some(&bounds) = self.external_bounds.get(&id) {
            return bounds;
        }

        // Otherwise return computed bounds
        self.computed_bounds.get(&id).copied().unwrap_or_default()
    }

    fn set_external_bounds(&mut self, id: LayoutId, bounds: Bounds<Pixels>) {
        self.external_bounds.insert(id, bounds);
    }

    fn apply_external_overrides(&mut self, overrides: &[ExternalLayoutOverride]) {
        for override_entry in overrides {
            self.external_bounds
                .insert(override_entry.layout_id, override_entry.bounds);
            if let Some(style) = &override_entry.style {
                self.external_styles
                    .insert(override_entry.layout_id, style.clone());
            } else {
                self.external_styles.remove(&override_entry.layout_id);
            }
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl Default for YogaLayoutEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn yoga_input_to_known_dimension(input: YogaMeasureInput, scale_factor: f32) -> Option<Pixels> {
    if input.mode == YogaMeasureMode::Exactly {
        Some(Pixels(input.value / scale_factor))
    } else {
        None
    }
}

fn yoga_input_to_available_space(input: YogaMeasureInput, scale_factor: f32) -> AvailableSpace {
    if input.mode == YogaMeasureMode::Undefined {
        AvailableSpace::MaxContent
    } else {
        AvailableSpace::Definite(Pixels(input.value / scale_factor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AbsoluteLength, AlignContent, AlignSelf, AppContext, AvailableSpace, Bounds, Context,
        DefiniteLength, Display, FlexDirection, IntoElement, JustifyContent, Length, Pixels,
        Position, Render, Size, Style, TestAppContext, Window, div, layout::LayoutMeasureFn,
        taffy::TaffyLayoutEngine,
    };
    use stacksafe::StackSafe;

    struct EmptyView;

    impl Render for EmptyView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[gpui::test]
    fn measured_nodes_match_taffy(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| cx.new(|_| EmptyView))
                .unwrap()
        });

        window
            .update(cx, |_, window, cx| {
                let mut taffy = TaffyLayoutEngine::new();
                let mut yoga = YogaLayoutEngine::new();

                let rem_size = window.rem_size();
                let scale = window.scale_factor();
                let mut measured_style = Style::default();
                measured_style.size.width = length_px(50.0);
                measured_style.max_size.height = length_px(40.0);
                measured_style.align_self = Some(AlignSelf::Center);

                let measured_taffy = taffy.request_measured_layout(
                    measured_style.clone(),
                    rem_size,
                    scale,
                    make_measure_fn(),
                );
                let measured_yoga = yoga.request_measured_layout(
                    measured_style,
                    rem_size,
                    scale,
                    make_measure_fn(),
                );

                let mut root = Style::default();
                root.display = Display::Flex;
                root.flex_direction = FlexDirection::Row;
                root.size = Size {
                    width: length_px(200.0),
                    height: length_px(120.0),
                };
                root.justify_content = Some(JustifyContent::SpaceBetween);
                root.padding.left = definite_px(8.0);
                root.padding.right = definite_px(12.0);

                let root_taffy =
                    taffy.request_layout(root.clone(), rem_size, scale, &[measured_taffy]);
                let root_yoga = yoga.request_layout(root, rem_size, scale, &[measured_yoga]);

                let available = Size {
                    width: AvailableSpace::Definite(Pixels(160.0)),
                    height: AvailableSpace::MaxContent,
                };

                taffy.compute_layout(root_taffy, available, window, cx);
                yoga.compute_layout(root_yoga, available, window, cx);

                let taffy_bounds = taffy.layout_bounds(measured_taffy, window.scale_factor());
                let yoga_bounds = yoga.layout_bounds(measured_yoga, window.scale_factor());
                assert_bounds_close_with_label("measured", taffy_bounds, yoga_bounds);
            })
            .unwrap();
    }

    #[gpui::test]
    fn flex_trees_match_taffy(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| cx.new(|_| EmptyView))
                .unwrap()
        });

        window
            .update(cx, |_, window, cx| {
                let mut taffy = TaffyLayoutEngine::new();
                let mut yoga = YogaLayoutEngine::new();

                let rem = window.rem_size();
                let scale = window.scale_factor();

                // Leaf nodes
                let mut flex_child = Style::default();
                flex_child.size.width = length_px(60.0);
                flex_child.size.height = length_px(20.0);
                flex_child.margin.left = length_px(10.0);
                flex_child.padding.top = definite_px(6.0);
                flex_child.padding.bottom = definite_px(4.0);
                flex_child.padding.left = definite_px(5.0);
                flex_child.align_self = Some(AlignSelf::FlexEnd);

                let mut nested_child_a = Style::default();
                nested_child_a.size = Size {
                    width: length_px(30.0),
                    height: length_px(24.0),
                };
                nested_child_a.margin.right = length_px(8.0);

                let mut nested_child_b = Style::default();
                nested_child_b.flex_grow = 1.0;
                nested_child_b.min_size.width = length_px(40.0);
                nested_child_b.size.height = length_px(50.0);

                let mut absolute_child = Style::default();
                absolute_child.position = Position::Absolute;
                absolute_child.size = Size {
                    width: length_px(32.0),
                    height: length_px(18.0),
                };
                absolute_child.inset.left = Length::Definite(DefiniteLength::Absolute(
                    AbsoluteLength::Pixels(Pixels(60.0)),
                ));
                absolute_child.inset.top = Length::Definite(DefiniteLength::Absolute(
                    AbsoluteLength::Pixels(Pixels(5.0)),
                ));

                let flex_child_taffy = taffy.request_layout(flex_child.clone(), rem, scale, &[]);
                let flex_child_yoga = yoga.request_layout(flex_child, rem, scale, &[]);
                let nested_child_a_taffy =
                    taffy.request_layout(nested_child_a.clone(), rem, scale, &[]);
                let nested_child_a_yoga = yoga.request_layout(nested_child_a, rem, scale, &[]);
                let nested_child_b_taffy =
                    taffy.request_layout(nested_child_b.clone(), rem, scale, &[]);
                let nested_child_b_yoga = yoga.request_layout(nested_child_b, rem, scale, &[]);
                let absolute_child_taffy =
                    taffy.request_layout(absolute_child.clone(), rem, scale, &[]);
                let absolute_child_yoga = yoga.request_layout(absolute_child, rem, scale, &[]);

                let mut nested_container = Style::default();
                nested_container.display = Display::Flex;
                nested_container.flex_direction = FlexDirection::Column;
                nested_container.gap = Size {
                    width: definite_px(6.0),
                    height: definite_px(4.0),
                };
                nested_container.align_content = Some(AlignContent::SpaceAround);
                nested_container.padding.left = definite_px(10.0);
                nested_container.padding.right = definite_px(5.0);
                nested_container.padding.top = definite_px(6.0);
                nested_container.padding.bottom = definite_px(6.0);
                nested_container.size.width = length_px(90.0);

                let nested_container_children = [nested_child_a_taffy, nested_child_b_taffy];
                let nested_container_children_yoga = [nested_child_a_yoga, nested_child_b_yoga];
                let nested_container_taffy = taffy.request_layout(
                    nested_container.clone(),
                    rem,
                    scale,
                    &nested_container_children,
                );
                let nested_container_yoga = yoga.request_layout(
                    nested_container,
                    rem,
                    scale,
                    &nested_container_children_yoga,
                );

                let mut root = Style::default();
                root.display = Display::Flex;
                root.flex_direction = FlexDirection::Row;
                root.justify_content = Some(JustifyContent::SpaceBetween);
                root.align_content = Some(AlignContent::Center);
                root.gap = Size {
                    width: definite_px(12.0),
                    height: definite_px(6.0),
                };
                root.size = Size {
                    width: length_px(240.0),
                    height: length_px(150.0),
                };
                root.padding.top = definite_px(12.0);
                root.padding.bottom = definite_px(8.0);
                root.padding.left = definite_px(16.0);
                root.padding.right = definite_px(10.0);

                let root_children = [
                    flex_child_taffy,
                    nested_container_taffy,
                    absolute_child_taffy,
                ];
                let root_children_yoga =
                    [flex_child_yoga, nested_container_yoga, absolute_child_yoga];
                let root_taffy = taffy.request_layout(root.clone(), rem, scale, &root_children);
                let root_yoga = yoga.request_layout(root, rem, scale, &root_children_yoga);

                let available = Size {
                    width: AvailableSpace::Definite(Pixels(320.0)),
                    height: AvailableSpace::Definite(Pixels(200.0)),
                };

                taffy.compute_layout(root_taffy, available, window, cx);
                yoga.compute_layout(root_yoga, available, window, cx);

                for (label, taffy_id, yoga_id) in [
                    ("root", root_taffy, root_yoga),
                    ("flex_child", flex_child_taffy, flex_child_yoga),
                    (
                        "nested_container",
                        nested_container_taffy,
                        nested_container_yoga,
                    ),
                    ("nested_child_a", nested_child_a_taffy, nested_child_a_yoga),
                    ("nested_child_b", nested_child_b_taffy, nested_child_b_yoga),
                    ("absolute_child", absolute_child_taffy, absolute_child_yoga),
                ] {
                    let t_bounds = taffy.layout_bounds(taffy_id, window.scale_factor());
                    let y_bounds = yoga.layout_bounds(yoga_id, window.scale_factor());
                    assert_bounds_close_with_label(label, t_bounds, y_bounds);
                }
            })
            .unwrap();
    }

    fn make_measure_fn() -> LayoutMeasureFn {
        StackSafe::new(Box::new(|known, available, _, _| Size {
            width: known
                .width
                .or_else(|| definite_from_space(available.width))
                .unwrap_or(Pixels(42.0)),
            height: known
                .height
                .or_else(|| definite_from_space(available.height))
                .unwrap_or(Pixels(24.0)),
        }))
    }

    fn definite_from_space(space: AvailableSpace) -> Option<Pixels> {
        match space {
            AvailableSpace::Definite(px) => Some(px),
            _ => None,
        }
    }

    fn assert_bounds_close_with_label(
        label: &str,
        expected: Bounds<Pixels>,
        actual: Bounds<Pixels>,
    ) {
        let epsilon = 0.001;
        assert!(
            (f32::from(expected.origin.x) - f32::from(actual.origin.x)).abs() < epsilon,
            "{label} x mismatch: {:?} vs {:?}",
            expected,
            actual
        );
        assert!(
            (f32::from(expected.origin.y) - f32::from(actual.origin.y)).abs() < epsilon,
            "{label} y mismatch: {:?} vs {:?}",
            expected,
            actual
        );
        assert!(
            (f32::from(expected.size.width) - f32::from(actual.size.width)).abs() < epsilon,
            "{label} width mismatch: {:?} vs {:?}",
            expected,
            actual
        );
        assert!(
            (f32::from(expected.size.height) - f32::from(actual.size.height)).abs() < epsilon,
            "{label} height mismatch: {:?} vs {:?}",
            expected,
            actual
        );
    }

    fn length_px(value: f32) -> Length {
        Length::Definite(DefiniteLength::Absolute(AbsoluteLength::Pixels(Pixels(
            value,
        ))))
    }

    fn definite_px(value: f32) -> DefiniteLength {
        DefiniteLength::Absolute(AbsoluteLength::Pixels(Pixels(value)))
    }
}
