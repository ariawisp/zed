use crate::{App, Bounds, Pixels, Size, Style, Window};
use stacksafe::StackSafe;

/// Type alias for layout measure callbacks stored on layout nodes.
pub type LayoutMeasureFn = StackSafe<
    Box<
        dyn FnMut(
            Size<Option<Pixels>>,
            Size<AvailableSpace>,
            &mut Window,
            &mut App,
        ) -> Size<Pixels>,
    >,
>;

/// Represents an externally-computed layout override for a node in the layout tree.
///
/// External embedders (e.g., React Native) can provide authoritative layout information
/// for specific [`LayoutId`]s. During commit processing, a batch of overrides can be
/// pushed into the layout engine so subsequent GPUI layout queries observe the
/// externally-computed bounds and style metadata.
#[derive(Clone, Debug)]
pub struct ExternalLayoutOverride {
    /// The layout node to override.
    pub layout_id: LayoutId,
    /// Absolute, window-relative bounds for the node.
    pub bounds: Bounds<Pixels>,
    /// Optional style metadata describing padding/margin/etc for the node.
    pub style: Option<Style>,
}

/// The space available for an element to be laid out in
#[derive(Copy, Clone, Default, Debug, Eq, PartialEq)]
pub enum AvailableSpace {
    /// The amount of space available is the specified number of pixels
    Definite(Pixels),
    /// The amount of space available is indefinite and the node should be laid out under a min-content constraint
    #[default]
    MinContent,
    /// The amount of space available is indefinite and the node should be laid out under a max-content constraint
    MaxContent,
}

impl AvailableSpace {
    /// Returns a `Size` with both width and height set to `AvailableSpace::MinContent`.
    ///
    /// This function is useful when you want to create a `Size` with the minimum content constraints
    /// for both dimensions.
    pub const fn min_size() -> Size<Self> {
        Size {
            width: Self::MinContent,
            height: Self::MinContent,
        }
    }
}

impl From<Pixels> for AvailableSpace {
    fn from(pixels: Pixels) -> Self {
        AvailableSpace::Definite(pixels)
    }
}

impl From<Size<Pixels>> for Size<AvailableSpace> {
    fn from(size: Size<Pixels>) -> Self {
        Size {
            width: AvailableSpace::Definite(size.width),
            height: AvailableSpace::Definite(size.height),
        }
    }
}

/// A unique identifier for a layout node.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub struct LayoutId(u64);

impl LayoutId {
    /// Construct a layout id from its raw representation.
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Returns the raw identifier backing this layout id.
    pub const fn to_raw(self) -> u64 {
        self.0
    }
}

impl From<u64> for LayoutId {
    fn from(raw: u64) -> Self {
        Self(raw)
    }
}

impl From<LayoutId> for u64 {
    fn from(value: LayoutId) -> Self {
        value.0
    }
}

/// Trait implemented by layout backends (Taffy, Yoga, etc.) that GPUI can target.
pub trait LayoutEngine: 'static {
    /// Remove cached state and return the engine to a pristine state.
    fn clear(&mut self);

    /// Remove a node from the layout tree.
    fn remove_node(&mut self, _layout_id: LayoutId) {
        let _ = _layout_id;
    }

    /// Add a node with optional children to the tree, returning its id.
    fn request_layout(
        &mut self,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
        children: &[LayoutId],
    ) -> LayoutId;

    /// Add a custom-measured node to the tree.
    fn request_measured_layout(
        &mut self,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
        measure: LayoutMeasureFn,
    ) -> LayoutId;

    /// Compute layout results for the given subtree.
    fn compute_layout(
        &mut self,
        id: LayoutId,
        available_space: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    );

    /// Fetch the computed bounds for a node.
    fn layout_bounds(&mut self, id: LayoutId, scale_factor: f32) -> Bounds<Pixels>;

    /// Override the computed bounds for a node.
    fn set_external_bounds(&mut self, id: LayoutId, bounds: Bounds<Pixels>);

    /// Apply a batch of external overrides.
    fn apply_external_overrides(&mut self, overrides: &[ExternalLayoutOverride]);
}

/// Create the default layout engine used by Windows.
pub(crate) fn default_layout_engine() -> Box<dyn LayoutEngine> {
    #[cfg(feature = "yoga")]
    {
        Box::new(crate::yoga::YogaLayoutEngine::new())
    }
    #[cfg(not(feature = "yoga"))]
    {
        Box::new(crate::taffy::TaffyLayoutEngine::new())
    }
}
