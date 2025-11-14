use super::ffi::{
    calculate_layout, create_node, free_node, layout, set_children, set_measure, set_style,
    YogaAvailableDimension, YogaAvailableDimensionKind, YogaAvailableSize, YogaMeasureHandle,
    YogaMeasureInput, YogaNodeHandle, YogaSize,
};
use super::style_conversion::convert_style_to_yoga;
use crate::{
    layout::LayoutMeasureFn, App, AvailableSpace, Bounds, ExternalLayoutOverride, LayoutEngine,
    LayoutId, Pixels, Point, Size, Style, Window,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// Thread-local context for providing Window and App to measure callbacks.
///
/// This is necessary because Yoga's measure callbacks are raw C function pointers
/// that cannot capture Rust closures. We use thread-local storage to provide
/// the necessary context during layout computation.
thread_local! {
    static MEASURE_CONTEXT: RefCell<Option<MeasureContext>> = RefCell::new(None);
}

struct MeasureContext {
    window_ptr: *mut Window,
    app_ptr: *mut App,
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
        }
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
        _id: LayoutId,
    ) -> impl FnMut(YogaMeasureInput, YogaMeasureInput) -> YogaSize + Send + 'static {
        move |width: YogaMeasureInput, height: YogaMeasureInput| -> YogaSize {
            MEASURE_CONTEXT.with(|ctx| {
                let context = ctx.borrow();
                let Some(ref _measure_ctx) = *context else {
                    return YogaSize::default();
                };

                // TODO: Actually invoke GPUI measure function
                // This requires accessing the stored measure function for this layout ID
                // which is tricky from within this closure

                YogaSize {
                    width: width.value,
                    height: height.value,
                }
            })
        }
    }
}

impl LayoutEngine for YogaLayoutEngine {
    fn clear(&mut self) {
        // Free all Yoga nodes
        for (_, node) in self.nodes.drain() {
            free_node(node);
        }
        self.computed_bounds.clear();
        self.children_map.clear();
        self.measure_handles.clear();
        self.measure_functions.clear();
        self.external_bounds.clear();
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
        }
    }

    fn request_layout(
        &mut self,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
        children: &[LayoutId],
    ) -> LayoutId {
        // Convert GPUI style to Yoga style
        let yoga_style = convert_style_to_yoga(&style, rem_size, scale_factor);

        // Create Yoga node
        let node = create_node();
        set_style(node, &yoga_style);

        // Set children (convert LayoutIds to YogaNodeHandles)
        let child_nodes: Vec<YogaNodeHandle> = children
            .iter()
            .filter_map(|child_id| self.nodes.get(child_id).copied())
            .collect();
        set_children(node, &child_nodes);

        // Generate LayoutId and store mappings
        let layout_id = LayoutId::from_raw(self.next_id);
        self.next_id += 1;

        self.nodes.insert(layout_id, node);
        self.children_map.insert(layout_id, children.to_vec());

        layout_id
    }

    fn request_measured_layout(
        &mut self,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
        measure: LayoutMeasureFn,
    ) -> LayoutId {
        // Convert GPUI style to Yoga style
        let yoga_style = convert_style_to_yoga(&style, rem_size, scale_factor);

        // Create Yoga node
        let node = create_node();
        set_style(node, &yoga_style);

        // Generate LayoutId first (needed for callback closure)
        let layout_id = LayoutId::from_raw(self.next_id);
        self.next_id += 1;

        // Store the measure function for later use
        self.measure_functions.insert(layout_id, measure);

        // Create and register measure callback
        let measure_callback = Self::create_measure_callback(layout_id);
        let measure_handle = set_measure(node, measure_callback);

        self.nodes.insert(layout_id, node);
        self.measure_handles.insert(layout_id, measure_handle);
        self.children_map.insert(layout_id, Vec::new());

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
            });
        });

        // Run Yoga layout computation
        calculate_layout(node, &yoga_available);

        // Clear measure context
        MEASURE_CONTEXT.with(|ctx| {
            *ctx.borrow_mut() = None;
        });

        // Extract bounds recursively, starting from origin (0, 0)
        let scale_factor = 1.0; // TODO: Get actual scale factor from window
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
        self.computed_bounds
            .get(&id)
            .copied()
            .unwrap_or_default()
    }

    fn set_external_bounds(&mut self, id: LayoutId, bounds: Bounds<Pixels>) {
        self.external_bounds.insert(id, bounds);
    }

    fn apply_external_overrides(&mut self, overrides: &[ExternalLayoutOverride]) {
        for override_entry in overrides {
            self.external_bounds
                .insert(override_entry.layout_id, override_entry.bounds);
        }
    }
}

impl Default for YogaLayoutEngine {
    fn default() -> Self {
        Self::new()
    }
}
