use crate::{
    AbsoluteLength, App, Bounds, DefiniteLength, Edges, LayoutEngine, LayoutId, Length, Pixels,
    Point, Size, Style, Window,
    layout::{AvailableSpace, ExternalLayoutOverride, LayoutMeasureFn},
    point, size,
};
use collections::{FxHashMap, FxHashSet};
use stacksafe::stacksafe;
use std::{any::Any, fmt::Debug, ops::Range};
use taffy::{
    TaffyTree, TraversePartialTree as _,
    geometry::{Point as TaffyPoint, Rect as TaffyRect, Size as TaffySize},
    style::AvailableSpace as TaffyAvailableSpace,
    tree::NodeId,
};

struct NodeContext {
    measure: LayoutMeasureFn,
}

pub struct TaffyLayoutEngine {
    taffy: TaffyTree<NodeContext>,
    absolute_layout_bounds: FxHashMap<LayoutId, Bounds<Pixels>>,
    external_styles: FxHashMap<LayoutId, Style>,
    computed_layouts: FxHashSet<LayoutId>,
    layout_bounds_scratch_space: Vec<LayoutId>,
    node_id_scratch: Vec<NodeId>,
}

const EXPECT_MESSAGE: &str = "we should avoid taffy layout errors by construction if possible";

impl TaffyLayoutEngine {
    pub fn new() -> Self {
        let mut taffy = TaffyTree::new();
        taffy.enable_rounding();
        TaffyLayoutEngine {
            taffy,
            absolute_layout_bounds: FxHashMap::default(),
            external_styles: FxHashMap::default(),
            computed_layouts: FxHashSet::default(),
            layout_bounds_scratch_space: Vec::new(),
            node_id_scratch: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.taffy.clear();
        self.absolute_layout_bounds.clear();
        self.external_styles.clear();
        self.computed_layouts.clear();
        self.node_id_scratch.clear();
    }

    /// Override the computed layout bounds for a given node for this frame.
    ///
    /// Embedders that integrate an external layout engine (e.g., React Native's Yoga)
    /// can call this to provide absolute, window-relative bounds for a layout node.
    /// When present, `layout_bounds` will return these bounds instead of querying Taffy.
    pub fn set_external_bounds(&mut self, id: LayoutId, bounds: Bounds<Pixels>) {
        self.absolute_layout_bounds.insert(id, bounds);
    }

    /// Apply a batch of externally-computed layout overrides.
    ///
    /// Each override contains absolute bounds along with optional style metadata that
    /// should be associated with the node for the current frame.
    pub fn apply_external_overrides<'a>(
        &mut self,
        overrides: impl IntoIterator<Item = &'a ExternalLayoutOverride>,
    ) {
        for override_entry in overrides {
            self.absolute_layout_bounds
                .insert(override_entry.layout_id, override_entry.bounds);
            if let Some(style) = &override_entry.style {
                self.external_styles
                    .insert(override_entry.layout_id, style.clone());
            }
        }
    }

    pub fn request_layout(
        &mut self,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
        children: &[LayoutId],
    ) -> LayoutId {
        let taffy_style = style.to_taffy(rem_size, scale_factor);

        if children.is_empty() {
            self.taffy
                .new_leaf(taffy_style)
                .expect(EXPECT_MESSAGE)
                .into()
        } else {
            self.node_id_scratch.clear();
            self.node_id_scratch
                .extend(children.iter().copied().map(NodeId::from));
            let node_id = self
                .taffy
                .new_with_children(taffy_style, &self.node_id_scratch)
                .expect(EXPECT_MESSAGE);
            self.node_id_scratch.clear();
            node_id.into()
        }
    }

    pub fn request_measured_layout(
        &mut self,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
        measure: LayoutMeasureFn,
    ) -> LayoutId {
        let taffy_style = style.to_taffy(rem_size, scale_factor);

        self.taffy
            .new_leaf_with_context(taffy_style, NodeContext { measure })
            .expect(EXPECT_MESSAGE)
            .into()
    }

    // Used to understand performance
    #[allow(dead_code)]
    fn count_all_children(&self, parent: LayoutId) -> anyhow::Result<u32> {
        let mut count = 0;

        for child in self.taffy.children(NodeId::from(parent))? {
            // Count this child.
            count += 1;

            // Count all of this child's children.
            count += self.count_all_children(child.into())?
        }

        Ok(count)
    }

    // Used to understand performance
    #[allow(dead_code)]
    fn max_depth(&self, depth: u32, parent: LayoutId) -> anyhow::Result<u32> {
        println!(
            "{parent:?} at depth {depth} has {} children",
            self.taffy.child_count(NodeId::from(parent))
        );

        let mut max_child_depth = 0;

        for child in self.taffy.children(NodeId::from(parent))? {
            max_child_depth = std::cmp::max(max_child_depth, self.max_depth(0, child.into())?);
        }

        Ok(depth + 1 + max_child_depth)
    }

    // Used to understand performance
    #[allow(dead_code)]
    fn get_edges(&self, parent: LayoutId) -> anyhow::Result<Vec<(LayoutId, LayoutId)>> {
        let mut edges = Vec::new();

        for child in self.taffy.children(NodeId::from(parent))? {
            edges.push((parent, child.into()));

            edges.extend(self.get_edges(child.into())?);
        }

        Ok(edges)
    }

    #[stacksafe]
    pub fn compute_layout(
        &mut self,
        id: LayoutId,
        available_space: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Leaving this here until we have a better instrumentation approach.
        // println!("Laying out {} children", self.count_all_children(id)?);
        // println!("Max layout depth: {}", self.max_depth(0, id)?);

        // Output the edges (branches) of the tree in Mermaid format for visualization.
        // println!("Edges:");
        // for (a, b) in self.get_edges(id)? {
        //     println!("N{} --> N{}", u64::from(a), u64::from(b));
        // }
        //

        if !self.computed_layouts.insert(id) {
            let mut stack = &mut self.layout_bounds_scratch_space;
            stack.push(id);
            while let Some(id) = stack.pop() {
                self.absolute_layout_bounds.remove(&id);
                stack.extend(
                    self.taffy
                        .children(id.into())
                        .expect(EXPECT_MESSAGE)
                        .into_iter()
                        .map(LayoutId::from),
                );
            }
        }

        let scale_factor = window.scale_factor();

        let transform = |v: AvailableSpace| match v {
            AvailableSpace::Definite(pixels) => {
                AvailableSpace::Definite(Pixels(pixels.0 * scale_factor))
            }
            AvailableSpace::MinContent => AvailableSpace::MinContent,
            AvailableSpace::MaxContent => AvailableSpace::MaxContent,
        };
        let available_space = size(
            transform(available_space.width),
            transform(available_space.height),
        );

        self.taffy
            .compute_layout_with_measure(
                id.into(),
                available_space.into(),
                |known_dimensions, available_space, _id, node_context, _style| {
                    let Some(node_context) = node_context else {
                        return taffy::geometry::Size::default();
                    };

                    let known_dimensions = Size {
                        width: known_dimensions.width.map(|e| Pixels(e / scale_factor)),
                        height: known_dimensions.height.map(|e| Pixels(e / scale_factor)),
                    };

                    let available_space: Size<AvailableSpace> = available_space.into();
                    let untransform = |ev: AvailableSpace| match ev {
                        AvailableSpace::Definite(pixels) => {
                            AvailableSpace::Definite(Pixels(pixels.0 / scale_factor))
                        }
                        AvailableSpace::MinContent => AvailableSpace::MinContent,
                        AvailableSpace::MaxContent => AvailableSpace::MaxContent,
                    };
                    let available_space = size(
                        untransform(available_space.width),
                        untransform(available_space.height),
                    );

                    let a: Size<Pixels> =
                        (node_context.measure)(known_dimensions, available_space, window, cx);
                    size(a.width.0 * scale_factor, a.height.0 * scale_factor).into()
                },
            )
            .expect(EXPECT_MESSAGE);
    }

    pub fn layout_bounds(&mut self, id: LayoutId, scale_factor: f32) -> Bounds<Pixels> {
        if let Some(layout) = self.absolute_layout_bounds.get(&id).cloned() {
            return layout;
        }

        let layout = self.taffy.layout(id.into()).expect(EXPECT_MESSAGE);
        let mut bounds = Bounds {
            origin: point(
                Pixels(layout.location.x / scale_factor),
                Pixels(layout.location.y / scale_factor),
            ),
            size: size(
                Pixels(layout.size.width / scale_factor),
                Pixels(layout.size.height / scale_factor),
            ),
        };

        if let Some(parent_id) = self.taffy.parent(NodeId::from(id)) {
            let parent_bounds = self.layout_bounds(parent_id.into(), scale_factor);
            bounds.origin += parent_bounds.origin;
        }
        self.absolute_layout_bounds.insert(id, bounds);

        bounds
    }
}

impl LayoutEngine for TaffyLayoutEngine {
    fn clear(&mut self) {
        TaffyLayoutEngine::clear(self);
    }

    fn remove_node(&mut self, layout_id: LayoutId) {
        if self.taffy.remove(layout_id.into()).is_ok() {
            self.absolute_layout_bounds.remove(&layout_id);
            self.external_styles.remove(&layout_id);
            self.computed_layouts.remove(&layout_id);
        }
    }

    fn request_layout(
        &mut self,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
        children: &[LayoutId],
    ) -> LayoutId {
        TaffyLayoutEngine::request_layout(self, style, rem_size, scale_factor, children)
    }

    fn request_measured_layout(
        &mut self,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
        measure: LayoutMeasureFn,
    ) -> LayoutId {
        TaffyLayoutEngine::request_measured_layout(self, style, rem_size, scale_factor, measure)
    }

    fn compute_layout(
        &mut self,
        id: LayoutId,
        available_space: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    ) {
        TaffyLayoutEngine::compute_layout(self, id, available_space, window, cx);
    }

    fn layout_bounds(&mut self, id: LayoutId, scale_factor: f32) -> Bounds<Pixels> {
        TaffyLayoutEngine::layout_bounds(self, id, scale_factor)
    }

    fn set_external_bounds(&mut self, id: LayoutId, bounds: Bounds<Pixels>) {
        TaffyLayoutEngine::set_external_bounds(self, id, bounds);
    }

    fn apply_external_overrides(&mut self, overrides: &[ExternalLayoutOverride]) {
        TaffyLayoutEngine::apply_external_overrides(self, overrides.iter());
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl From<NodeId> for LayoutId {
    fn from(node_id: NodeId) -> Self {
        LayoutId::from_raw(node_id.into())
    }
}

impl From<LayoutId> for NodeId {
    fn from(layout_id: LayoutId) -> Self {
        NodeId::from(layout_id.to_raw())
    }
}

trait ToTaffy<Output> {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> Output;
}

impl ToTaffy<taffy::style::Style> for Style {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::Style {
        use taffy::style_helpers::{fr, length, minmax, repeat};

        fn to_grid_line(
            placement: &Range<crate::GridPlacement>,
        ) -> taffy::Line<taffy::GridPlacement> {
            taffy::Line {
                start: placement.start.into(),
                end: placement.end.into(),
            }
        }

        fn to_grid_repeat<T: taffy::style::CheapCloneStr>(
            unit: &Option<u16>,
        ) -> Vec<taffy::GridTemplateComponent<T>> {
            // grid-template-columns: repeat(<number>, minmax(0, 1fr));
            unit.map(|count| vec![repeat(count, vec![minmax(length(0.0), fr(1.0))])])
                .unwrap_or_default()
        }

        taffy::style::Style {
            display: self.display.into(),
            overflow: self.overflow.into(),
            scrollbar_width: self.scrollbar_width.to_taffy(rem_size, scale_factor),
            position: self.position.into(),
            inset: self.inset.to_taffy(rem_size, scale_factor),
            size: self.size.to_taffy(rem_size, scale_factor),
            min_size: self.min_size.to_taffy(rem_size, scale_factor),
            max_size: self.max_size.to_taffy(rem_size, scale_factor),
            aspect_ratio: self.aspect_ratio,
            margin: self.margin.to_taffy(rem_size, scale_factor),
            padding: self.padding.to_taffy(rem_size, scale_factor),
            border: self.border_widths.to_taffy(rem_size, scale_factor),
            align_items: self.align_items.map(|x| x.into()),
            align_self: self.align_self.map(|x| x.into()),
            align_content: self.align_content.map(|x| x.into()),
            justify_content: self.justify_content.map(|x| x.into()),
            gap: self.gap.to_taffy(rem_size, scale_factor),
            flex_direction: self.flex_direction.into(),
            flex_wrap: self.flex_wrap.into(),
            flex_basis: self.flex_basis.to_taffy(rem_size, scale_factor),
            flex_grow: self.flex_grow,
            flex_shrink: self.flex_shrink,
            grid_template_rows: to_grid_repeat(&self.grid_rows),
            grid_template_columns: to_grid_repeat(&self.grid_cols),
            grid_row: self
                .grid_location
                .as_ref()
                .map(|location| to_grid_line(&location.row))
                .unwrap_or_default(),
            grid_column: self
                .grid_location
                .as_ref()
                .map(|location| to_grid_line(&location.column))
                .unwrap_or_default(),
            ..Default::default()
        }
    }
}

impl ToTaffy<f32> for AbsoluteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> f32 {
        match self {
            AbsoluteLength::Pixels(pixels) => {
                let pixels: f32 = pixels.into();
                pixels * scale_factor
            }
            AbsoluteLength::Rems(rems) => {
                let pixels: f32 = (*rems * rem_size).into();
                pixels * scale_factor
            }
        }
    }
}

impl ToTaffy<taffy::style::LengthPercentageAuto> for Length {
    fn to_taffy(
        &self,
        rem_size: Pixels,
        scale_factor: f32,
    ) -> taffy::prelude::LengthPercentageAuto {
        match self {
            Length::Definite(length) => length.to_taffy(rem_size, scale_factor),
            Length::Auto => taffy::prelude::LengthPercentageAuto::auto(),
        }
    }
}

impl ToTaffy<taffy::style::Dimension> for Length {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::prelude::Dimension {
        match self {
            Length::Definite(length) => length.to_taffy(rem_size, scale_factor),
            Length::Auto => taffy::prelude::Dimension::auto(),
        }
    }
}

impl ToTaffy<taffy::style::LengthPercentage> for DefiniteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::LengthPercentage {
        match self {
            DefiniteLength::Absolute(length) => match length {
                AbsoluteLength::Pixels(pixels) => {
                    let pixels: f32 = pixels.into();
                    taffy::style::LengthPercentage::length(pixels * scale_factor)
                }
                AbsoluteLength::Rems(rems) => {
                    let pixels: f32 = (*rems * rem_size).into();
                    taffy::style::LengthPercentage::length(pixels * scale_factor)
                }
            },
            DefiniteLength::Fraction(fraction) => {
                taffy::style::LengthPercentage::percent(*fraction)
            }
        }
    }
}

impl ToTaffy<taffy::style::LengthPercentageAuto> for DefiniteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::LengthPercentageAuto {
        match self {
            DefiniteLength::Absolute(length) => match length {
                AbsoluteLength::Pixels(pixels) => {
                    let pixels: f32 = pixels.into();
                    taffy::style::LengthPercentageAuto::length(pixels * scale_factor)
                }
                AbsoluteLength::Rems(rems) => {
                    let pixels: f32 = (*rems * rem_size).into();
                    taffy::style::LengthPercentageAuto::length(pixels * scale_factor)
                }
            },
            DefiniteLength::Fraction(fraction) => {
                taffy::style::LengthPercentageAuto::percent(*fraction)
            }
        }
    }
}

impl ToTaffy<taffy::style::Dimension> for DefiniteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::Dimension {
        match self {
            DefiniteLength::Absolute(length) => match length {
                AbsoluteLength::Pixels(pixels) => {
                    let pixels: f32 = pixels.into();
                    taffy::style::Dimension::length(pixels * scale_factor)
                }
                AbsoluteLength::Rems(rems) => {
                    taffy::style::Dimension::length((*rems * rem_size * scale_factor).into())
                }
            },
            DefiniteLength::Fraction(fraction) => taffy::style::Dimension::percent(*fraction),
        }
    }
}

impl ToTaffy<taffy::style::LengthPercentage> for AbsoluteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::LengthPercentage {
        match self {
            AbsoluteLength::Pixels(pixels) => {
                let pixels: f32 = pixels.into();
                taffy::style::LengthPercentage::length(pixels * scale_factor)
            }
            AbsoluteLength::Rems(rems) => {
                let pixels: f32 = (*rems * rem_size).into();
                taffy::style::LengthPercentage::length(pixels * scale_factor)
            }
        }
    }
}

impl<T, T2> From<TaffyPoint<T>> for Point<T2>
where
    T: Into<T2>,
    T2: Clone + Debug + Default + PartialEq,
{
    fn from(point: TaffyPoint<T>) -> Point<T2> {
        Point {
            x: point.x.into(),
            y: point.y.into(),
        }
    }
}

impl<T, T2> From<Point<T>> for TaffyPoint<T2>
where
    T: Into<T2> + Clone + Debug + Default + PartialEq,
{
    fn from(val: Point<T>) -> Self {
        TaffyPoint {
            x: val.x.into(),
            y: val.y.into(),
        }
    }
}

impl<T, U> ToTaffy<TaffySize<U>> for Size<T>
where
    T: ToTaffy<U> + Clone + Debug + Default + PartialEq,
{
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> TaffySize<U> {
        TaffySize {
            width: self.width.to_taffy(rem_size, scale_factor),
            height: self.height.to_taffy(rem_size, scale_factor),
        }
    }
}

impl<T, U> ToTaffy<TaffyRect<U>> for Edges<T>
where
    T: ToTaffy<U> + Clone + Debug + Default + PartialEq,
{
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> TaffyRect<U> {
        TaffyRect {
            top: self.top.to_taffy(rem_size, scale_factor),
            right: self.right.to_taffy(rem_size, scale_factor),
            bottom: self.bottom.to_taffy(rem_size, scale_factor),
            left: self.left.to_taffy(rem_size, scale_factor),
        }
    }
}

impl<T, U> From<TaffySize<T>> for Size<U>
where
    T: Into<U>,
    U: Clone + Debug + Default + PartialEq,
{
    fn from(taffy_size: TaffySize<T>) -> Self {
        Size {
            width: taffy_size.width.into(),
            height: taffy_size.height.into(),
        }
    }
}

impl<T, U> From<Size<T>> for TaffySize<U>
where
    T: Into<U> + Clone + Debug + Default + PartialEq,
{
    fn from(size: Size<T>) -> Self {
        TaffySize {
            width: size.width.into(),
            height: size.height.into(),
        }
    }
}

impl From<AvailableSpace> for TaffyAvailableSpace {
    fn from(space: AvailableSpace) -> TaffyAvailableSpace {
        match space {
            AvailableSpace::Definite(Pixels(value)) => TaffyAvailableSpace::Definite(value),
            AvailableSpace::MinContent => TaffyAvailableSpace::MinContent,
            AvailableSpace::MaxContent => TaffyAvailableSpace::MaxContent,
        }
    }
}

impl From<TaffyAvailableSpace> for AvailableSpace {
    fn from(space: TaffyAvailableSpace) -> AvailableSpace {
        match space {
            TaffyAvailableSpace::Definite(value) => AvailableSpace::Definite(Pixels(value)),
            TaffyAvailableSpace::MinContent => AvailableSpace::MinContent,
            TaffyAvailableSpace::MaxContent => AvailableSpace::MaxContent,
        }
    }
}
