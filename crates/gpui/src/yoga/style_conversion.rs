use super::ffi::{
    YogaAlign, YogaDisplay, YogaEdges, YogaFlexDirection, YogaJustify, YogaOverflow,
    YogaPositionType, YogaStyle, YogaStyleSize, YogaValue, YogaValueUnit, YogaWrap,
};
use crate::{
    AbsoluteLength, AlignContent, AlignItems, AlignSelf, DefiniteLength, Display, Edges,
    FlexDirection, FlexWrap, JustifyContent, Length, Overflow, Pixels, Position, Size, Style,
};

/// Convert GPUI Style to Yoga YogaStyle.
///
/// This function maps GPUI's flexbox styles to Yoga's format, handling:
/// - Display types (Block → Flex, Grid → Flex with warning)
/// - Positioning (Relative, Absolute)
/// - Flexbox properties (direction, wrap, align, justify)
/// - Sizing (with rem and percentage support)
/// - Edges (margin, padding, border, inset)
///
/// ## Grid Fallback
///
/// Yoga doesn't support CSS Grid. When `Display::Grid` is detected, we:
/// 1. Log a warning to the user
/// 2. Convert to `Display::Flex` with wrapping behavior
/// 3. Attempt to approximate grid behavior using flex properties
///
/// This is lossy but allows apps to function. For true grid support, use TaffyLayoutEngine.
pub fn convert_style_to_yoga(style: &Style, rem_size: Pixels, scale_factor: f32) -> YogaStyle {
    // Handle Grid fallback
    let (display, flex_wrap) = match style.display {
        Display::Grid => {
            log::warn!(
                "Grid layout not supported by Yoga, converting to Flex with wrap. \
                 Layout may differ from expected. Consider using TaffyLayoutEngine for Grid support."
            );
            // Convert grid to flex-wrap to approximate grid behavior
            (YogaDisplay::Flex, YogaWrap::Wrap)
        }
        Display::Flex => (YogaDisplay::Flex, convert_flex_wrap(style.flex_wrap)),
        Display::Block => {
            // Block is similar to flex-direction: column in Yoga
            (YogaDisplay::Flex, convert_flex_wrap(style.flex_wrap))
        }
        Display::None => (YogaDisplay::None, YogaWrap::NoWrap),
    };

    YogaStyle {
        display,
        position_type: convert_position(style.position),
        overflow: convert_overflow(style.overflow.x), // Yoga uses single overflow value
        flex_direction: convert_flex_direction(style.flex_direction),
        flex_wrap,
        justify_content: convert_justify_content(style.justify_content),
        align_items: convert_align_items(style.align_items),
        align_self: convert_align_self(style.align_self),
        align_content: convert_align_content(style.align_content),

        // Edges
        margin: convert_edges(&style.margin, rem_size, scale_factor),
        padding: convert_edges_definite(&style.padding, rem_size, scale_factor),
        border: convert_edges_absolute(&style.border_widths, rem_size, scale_factor),
        inset: convert_edges(&style.inset, rem_size, scale_factor),

        // Sizing
        size: convert_size(&style.size, rem_size, scale_factor),
        min_size: convert_size(&style.min_size, rem_size, scale_factor),
        max_size: convert_size(&style.max_size, rem_size, scale_factor),
        gap: convert_gap(&style.gap, rem_size, scale_factor),

        // Flex properties
        flex_basis: convert_length(&style.flex_basis, rem_size, scale_factor),
        flex_grow: style.flex_grow,
        flex_shrink: style.flex_shrink,
        has_flex_grow: style.flex_grow > 0.0,
        has_flex_shrink: style.flex_shrink != 1.0,
        has_flex_basis: !matches!(style.flex_basis, Length::Auto),

        // Aspect ratio
        aspect_ratio: style.aspect_ratio.unwrap_or(f32::NAN),
        has_aspect_ratio: style.aspect_ratio.is_some(),
    }
}

fn convert_position(position: Position) -> YogaPositionType {
    match position {
        Position::Relative => YogaPositionType::Relative,
        Position::Absolute => YogaPositionType::Absolute,
    }
}

fn convert_overflow(overflow: Overflow) -> YogaOverflow {
    match overflow {
        Overflow::Visible => YogaOverflow::Visible,
        Overflow::Hidden | Overflow::Clip => YogaOverflow::Hidden,
        Overflow::Scroll => YogaOverflow::Scroll,
    }
}

fn convert_flex_direction(direction: FlexDirection) -> YogaFlexDirection {
    match direction {
        FlexDirection::Row => YogaFlexDirection::Row,
        FlexDirection::Column => YogaFlexDirection::Column,
        FlexDirection::RowReverse => YogaFlexDirection::RowReverse,
        FlexDirection::ColumnReverse => YogaFlexDirection::ColumnReverse,
    }
}

fn convert_flex_wrap(wrap: FlexWrap) -> YogaWrap {
    match wrap {
        FlexWrap::NoWrap => YogaWrap::NoWrap,
        FlexWrap::Wrap => YogaWrap::Wrap,
        FlexWrap::WrapReverse => YogaWrap::WrapReverse,
    }
}

fn convert_justify_content(justify: Option<JustifyContent>) -> YogaJustify {
    match justify {
        Some(JustifyContent::Start) | Some(JustifyContent::FlexStart) => YogaJustify::FlexStart,
        Some(JustifyContent::Center) => YogaJustify::Center,
        Some(JustifyContent::End) | Some(JustifyContent::FlexEnd) => YogaJustify::FlexEnd,
        Some(JustifyContent::SpaceBetween) => YogaJustify::SpaceBetween,
        Some(JustifyContent::SpaceAround) => YogaJustify::SpaceAround,
        Some(JustifyContent::SpaceEvenly) => YogaJustify::SpaceEvenly,
        Some(JustifyContent::Stretch) => YogaJustify::FlexStart, // Yoga doesn't have stretch for justify
        None => YogaJustify::FlexStart,
    }
}

fn convert_align_items(align: Option<AlignItems>) -> YogaAlign {
    match align {
        Some(AlignItems::Start) | Some(AlignItems::FlexStart) => YogaAlign::FlexStart,
        Some(AlignItems::Center) => YogaAlign::Center,
        Some(AlignItems::End) | Some(AlignItems::FlexEnd) => YogaAlign::FlexEnd,
        Some(AlignItems::Stretch) => YogaAlign::Stretch,
        Some(AlignItems::Baseline) => YogaAlign::Baseline,
        None => YogaAlign::Stretch, // Default for align-items
    }
}

fn convert_align_self(align: Option<AlignSelf>) -> YogaAlign {
    match align {
        Some(AlignSelf::Start) | Some(AlignSelf::FlexStart) => YogaAlign::FlexStart,
        Some(AlignSelf::Center) => YogaAlign::Center,
        Some(AlignSelf::End) | Some(AlignSelf::FlexEnd) => YogaAlign::FlexEnd,
        Some(AlignSelf::Stretch) => YogaAlign::Stretch,
        Some(AlignSelf::Baseline) => YogaAlign::Baseline,
        None => YogaAlign::Auto,
    }
}

fn convert_align_content(align: Option<AlignContent>) -> YogaAlign {
    match align {
        Some(AlignContent::Start) | Some(AlignContent::FlexStart) => YogaAlign::FlexStart,
        Some(AlignContent::Center) => YogaAlign::Center,
        Some(AlignContent::End) | Some(AlignContent::FlexEnd) => YogaAlign::FlexEnd,
        Some(AlignContent::Stretch) => YogaAlign::Stretch,
        Some(AlignContent::SpaceBetween) => YogaAlign::SpaceBetween,
        Some(AlignContent::SpaceAround) => YogaAlign::SpaceAround,
        Some(AlignContent::SpaceEvenly) => YogaAlign::SpaceAround, // Yoga doesn't have space-evenly for align-content
        None => YogaAlign::FlexStart,
    }
}

fn convert_length(length: &Length, rem_size: Pixels, scale_factor: f32) -> YogaValue {
    match length {
        Length::Auto => YogaValue {
            value: 0.0,
            unit: YogaValueUnit::Auto,
        },
        Length::Definite(definite) => convert_definite_length(definite, rem_size, scale_factor),
    }
}

fn convert_definite_length(
    length: &DefiniteLength,
    rem_size: Pixels,
    scale_factor: f32,
) -> YogaValue {
    match length {
        DefiniteLength::Absolute(abs) => convert_absolute_length(abs, rem_size, scale_factor),
        DefiniteLength::Fraction(fraction) => YogaValue {
            value: *fraction * 100.0, // Yoga uses 0-100 for percentages
            unit: YogaValueUnit::Percent,
        },
    }
}

fn convert_absolute_length(
    length: &AbsoluteLength,
    rem_size: Pixels,
    scale_factor: f32,
) -> YogaValue {
    let pixels = match length {
        AbsoluteLength::Pixels(px) => *px,
        AbsoluteLength::Rems(rems) => rems.to_pixels(rem_size),
    };

    YogaValue {
        value: pixels.0 * scale_factor,
        unit: YogaValueUnit::Point,
    }
}

fn convert_edges(edges: &Edges<Length>, rem_size: Pixels, scale_factor: f32) -> YogaEdges {
    YogaEdges {
        left: convert_length(&edges.left, rem_size, scale_factor),
        top: convert_length(&edges.top, rem_size, scale_factor),
        right: convert_length(&edges.right, rem_size, scale_factor),
        bottom: convert_length(&edges.bottom, rem_size, scale_factor),
    }
}

fn convert_edges_definite(
    edges: &Edges<DefiniteLength>,
    rem_size: Pixels,
    scale_factor: f32,
) -> YogaEdges {
    YogaEdges {
        left: convert_definite_length(&edges.left, rem_size, scale_factor),
        top: convert_definite_length(&edges.top, rem_size, scale_factor),
        right: convert_definite_length(&edges.right, rem_size, scale_factor),
        bottom: convert_definite_length(&edges.bottom, rem_size, scale_factor),
    }
}

fn convert_edges_absolute(
    edges: &Edges<AbsoluteLength>,
    rem_size: Pixels,
    scale_factor: f32,
) -> YogaEdges {
    YogaEdges {
        left: convert_absolute_length(&edges.left, rem_size, scale_factor),
        top: convert_absolute_length(&edges.top, rem_size, scale_factor),
        right: convert_absolute_length(&edges.right, rem_size, scale_factor),
        bottom: convert_absolute_length(&edges.bottom, rem_size, scale_factor),
    }
}

fn convert_size(size: &Size<Length>, rem_size: Pixels, scale_factor: f32) -> YogaStyleSize {
    YogaStyleSize {
        width: convert_length(&size.width, rem_size, scale_factor),
        height: convert_length(&size.height, rem_size, scale_factor),
    }
}

fn convert_gap(gap: &Size<DefiniteLength>, rem_size: Pixels, scale_factor: f32) -> YogaStyleSize {
    YogaStyleSize {
        width: convert_definite_length(&gap.width, rem_size, scale_factor),
        height: convert_definite_length(&gap.height, rem_size, scale_factor),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_simple_flex_row() {
        let style = Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            ..Default::default()
        };

        let yoga_style = convert_style_to_yoga(&style, Pixels(16.0), 1.0);

        assert_eq!(yoga_style.display, YogaDisplay::Flex);
        assert_eq!(yoga_style.flex_direction, YogaFlexDirection::Row);
    }

    #[test]
    fn test_convert_absolute_positioning() {
        let style = Style {
            position: Position::Absolute,
            ..Default::default()
        };

        let yoga_style = convert_style_to_yoga(&style, Pixels(16.0), 1.0);

        assert_eq!(yoga_style.position_type, YogaPositionType::Absolute);
    }

    #[test]
    fn test_convert_percentage_width() {
        let style = Style {
            size: Size {
                width: Length::Definite(DefiniteLength::Fraction(0.5)),
                height: Length::Auto,
            },
            ..Default::default()
        };

        let yoga_style = convert_style_to_yoga(&style, Pixels(16.0), 1.0);

        assert_eq!(yoga_style.size.width.unit, YogaValueUnit::Percent);
        assert_eq!(yoga_style.size.width.value, 50.0);
    }

    #[test]
    fn test_convert_rem_to_pixels() {
        let style = Style {
            padding: Edges {
                top: DefiniteLength::Absolute(AbsoluteLength::Rems(crate::geometry::Rems(1.0))),
                left: DefiniteLength::Absolute(AbsoluteLength::Rems(crate::geometry::Rems(1.0))),
                right: DefiniteLength::Absolute(AbsoluteLength::Rems(crate::geometry::Rems(1.0))),
                bottom: DefiniteLength::Absolute(AbsoluteLength::Rems(crate::geometry::Rems(1.0))),
            },
            ..Default::default()
        };

        let yoga_style = convert_style_to_yoga(&style, Pixels(16.0), 1.0);

        // 1 rem = 16 pixels at rem_size=16.0
        assert_eq!(yoga_style.padding.top.value, 16.0);
        assert_eq!(yoga_style.padding.top.unit, YogaValueUnit::Point);
    }

    #[test]
    fn test_grid_fallback_warns() {
        let style = Style {
            display: Display::Grid,
            ..Default::default()
        };

        let yoga_style = convert_style_to_yoga(&style, Pixels(16.0), 1.0);

        // Grid should be converted to Flex with wrap
        assert_eq!(yoga_style.display, YogaDisplay::Flex);
        assert_eq!(yoga_style.flex_wrap, YogaWrap::Wrap);
    }
}
