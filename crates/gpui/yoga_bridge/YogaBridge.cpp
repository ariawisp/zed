// SPDX-License-Identifier: MIT

#include "YogaBridge.h"

#include "gpui/src/yoga/ffi.rs.h"

#include <algorithm>
#include <vector>
#include <yoga/Yoga.h>

namespace gpui::yoga {
namespace {

YGNodeRef from_handle(YogaNodeHandle handle) {
  return reinterpret_cast<YGNodeRef>(static_cast<uintptr_t>(handle.raw));
}

YogaNodeHandle to_handle(YGNodeRef node) {
  return YogaNodeHandle{reinterpret_cast<std::uint64_t>(node)};
}

YGValue to_yg_value(const YogaValue &value) {
  switch (value.unit) {
  case YogaValueUnit::Point:
    return {value.value, YGUnitPoint};
  case YogaValueUnit::Percent:
    return {value.value, YGUnitPercent};
  case YogaValueUnit::Auto:
    return {0.0f, YGUnitAuto};
  case YogaValueUnit::Undefined:
  default:
    return {YGUndefined, YGUnitUndefined};
  }
}

YGDisplay to_yg_display(YogaDisplay display) {
  switch (display) {
  case YogaDisplay::None:
    return YGDisplayNone;
  case YogaDisplay::Flex:
  default:
    return YGDisplayFlex;
  }
}

YGPositionType to_yg_position_type(YogaPositionType type) {
  switch (type) {
  case YogaPositionType::Absolute:
    return YGPositionTypeAbsolute;
  case YogaPositionType::Relative:
  default:
    return YGPositionTypeRelative;
  }
}

YGOverflow to_yg_overflow(YogaOverflow overflow) {
  switch (overflow) {
  case YogaOverflow::Hidden:
    return YGOverflowHidden;
  case YogaOverflow::Scroll:
    return YGOverflowScroll;
  case YogaOverflow::Visible:
  default:
    return YGOverflowVisible;
  }
}

YGFlexDirection to_yg_flex_direction(YogaFlexDirection direction) {
  switch (direction) {
  case YogaFlexDirection::ColumnReverse:
    return YGFlexDirectionColumnReverse;
  case YogaFlexDirection::Row:
    return YGFlexDirectionRow;
  case YogaFlexDirection::RowReverse:
    return YGFlexDirectionRowReverse;
  case YogaFlexDirection::Column:
  default:
    return YGFlexDirectionColumn;
  }
}

YGWrap to_yg_wrap(YogaWrap value) {
  switch (value) {
  case YogaWrap::Wrap:
    return YGWrapWrap;
  case YogaWrap::WrapReverse:
    return YGWrapWrapReverse;
  case YogaWrap::NoWrap:
  default:
    return YGWrapNoWrap;
  }
}

YGAlign to_yg_align(YogaAlign value) {
  switch (value) {
  case YogaAlign::Auto:
    return YGAlignAuto;
  case YogaAlign::FlexStart:
    return YGAlignFlexStart;
  case YogaAlign::Center:
    return YGAlignCenter;
  case YogaAlign::FlexEnd:
    return YGAlignFlexEnd;
  case YogaAlign::Stretch:
    return YGAlignStretch;
  case YogaAlign::Baseline:
    return YGAlignBaseline;
  case YogaAlign::SpaceBetween:
    return YGAlignSpaceBetween;
  case YogaAlign::SpaceAround:
    return YGAlignSpaceAround;
  default:
    return YGAlignAuto;
  }
}

YGJustify to_yg_justify(YogaJustify value) {
  switch (value) {
  case YogaJustify::Center:
    return YGJustifyCenter;
  case YogaJustify::FlexEnd:
    return YGJustifyFlexEnd;
  case YogaJustify::SpaceBetween:
    return YGJustifySpaceBetween;
  case YogaJustify::SpaceAround:
    return YGJustifySpaceAround;
  case YogaJustify::SpaceEvenly:
    return YGJustifySpaceEvenly;
  case YogaJustify::FlexStart:
  default:
    return YGJustifyFlexStart;
  }
}

inline float value_or_undefined(const YogaAvailableDimension &dimension) {
  return dimension.kind == YogaAvailableDimensionKind::Definite
             ? dimension.value
             : YGUndefined;
}

struct MeasureContext {
  std::uint64_t id;
};

void drop_measure_context(YGNodeRef node) {
  if (!node) {
    return;
  }
  auto *ctx = static_cast<MeasureContext *>(YGNodeGetContext(node));
  if (ctx) {
    yoga_drop_measure(ctx->id);
    delete ctx;
    YGNodeSetContext(node, nullptr);
  }
}

void release_measure_recursive(YGNodeRef node) {
  if (!node) {
    return;
  }
  drop_measure_context(node);
  const uint32_t child_count = YGNodeGetChildCount(node);
  for (uint32_t i = 0; i < child_count; i++) {
    release_measure_recursive(YGNodeGetChild(node, i));
  }
}

void apply_edge_value(YGNodeRef node, YGEdge edge, const YogaValue &value,
                      void (*set_point)(YGNodeRef, YGEdge, float),
                      void (*set_percent)(YGNodeRef, YGEdge, float),
                      void (*set_auto)(YGNodeRef, YGEdge)) {
  switch (value.unit) {
  case YogaValueUnit::Percent:
    set_percent(node, edge, value.value);
    break;
  case YogaValueUnit::Point:
    set_point(node, edge, value.value);
    break;
  case YogaValueUnit::Auto:
    if (set_auto) {
      set_auto(node, edge);
    } else {
      set_point(node, edge, YGUndefined);
    }
    break;
  case YogaValueUnit::Undefined:
  default:
    set_point(node, edge, YGUndefined);
    break;
  }
}

void apply_dimension(YGNodeRef node, const YogaValue &value,
                     void (*set_point)(YGNodeRef, float),
                     void (*set_percent)(YGNodeRef, float),
                     void (*set_auto)(YGNodeRef)) {
  switch (value.unit) {
  case YogaValueUnit::Percent:
    set_percent(node, value.value);
    break;
  case YogaValueUnit::Point:
    set_point(node, value.value);
    break;
  case YogaValueUnit::Auto:
    if (set_auto) {
      set_auto(node);
    } else {
      set_point(node, YGUndefined);
    }
    break;
  case YogaValueUnit::Undefined:
  default:
    set_point(node, YGUndefined);
    break;
  }
}

void apply_style(YGNodeRef node, const YogaStyle &style) {
  YGNodeStyleSetDisplay(node, to_yg_display(style.display));
  YGNodeStyleSetPositionType(node, to_yg_position_type(style.position_type));
  YGNodeStyleSetOverflow(node, to_yg_overflow(style.overflow));
  YGNodeStyleSetFlexDirection(node, to_yg_flex_direction(style.flex_direction));
  YGNodeStyleSetFlexWrap(node, to_yg_wrap(style.flex_wrap));
  YGNodeStyleSetJustifyContent(node, to_yg_justify(style.justify_content));
  YGNodeStyleSetAlignItems(node, to_yg_align(style.align_items));
  YGNodeStyleSetAlignContent(node, to_yg_align(style.align_content));
  YGNodeStyleSetAlignSelf(node, to_yg_align(style.align_self));

  apply_edge_value(node, YGEdgeLeft, style.margin.left, YGNodeStyleSetMargin,
                   YGNodeStyleSetMarginPercent, YGNodeStyleSetMarginAuto);
  apply_edge_value(node, YGEdgeTop, style.margin.top, YGNodeStyleSetMargin,
                   YGNodeStyleSetMarginPercent, YGNodeStyleSetMarginAuto);
  apply_edge_value(node, YGEdgeRight, style.margin.right, YGNodeStyleSetMargin,
                   YGNodeStyleSetMarginPercent, YGNodeStyleSetMarginAuto);
  apply_edge_value(node, YGEdgeBottom, style.margin.bottom, YGNodeStyleSetMargin,
                   YGNodeStyleSetMarginPercent, YGNodeStyleSetMarginAuto);

  apply_edge_value(node, YGEdgeLeft, style.padding.left, YGNodeStyleSetPadding,
                   YGNodeStyleSetPaddingPercent, nullptr);
  apply_edge_value(node, YGEdgeTop, style.padding.top, YGNodeStyleSetPadding,
                   YGNodeStyleSetPaddingPercent, nullptr);
  apply_edge_value(node, YGEdgeRight, style.padding.right, YGNodeStyleSetPadding,
                   YGNodeStyleSetPaddingPercent, nullptr);
  apply_edge_value(node, YGEdgeBottom, style.padding.bottom,
                   YGNodeStyleSetPadding, YGNodeStyleSetPaddingPercent, nullptr);

  YGNodeStyleSetBorder(node, YGEdgeLeft, style.border.left.value);
  YGNodeStyleSetBorder(node, YGEdgeTop, style.border.top.value);
  YGNodeStyleSetBorder(node, YGEdgeRight, style.border.right.value);
  YGNodeStyleSetBorder(node, YGEdgeBottom, style.border.bottom.value);

  apply_edge_value(node, YGEdgeLeft, style.inset.left, YGNodeStyleSetPosition,
                   YGNodeStyleSetPositionPercent, nullptr);
  apply_edge_value(node, YGEdgeTop, style.inset.top, YGNodeStyleSetPosition,
                   YGNodeStyleSetPositionPercent, nullptr);
  apply_edge_value(node, YGEdgeRight, style.inset.right, YGNodeStyleSetPosition,
                   YGNodeStyleSetPositionPercent, nullptr);
  apply_edge_value(node, YGEdgeBottom, style.inset.bottom,
                   YGNodeStyleSetPosition, YGNodeStyleSetPositionPercent, nullptr);

  apply_dimension(node, style.size.width, YGNodeStyleSetWidth,
                  YGNodeStyleSetWidthPercent, YGNodeStyleSetWidthAuto);
  apply_dimension(node, style.size.height, YGNodeStyleSetHeight,
                  YGNodeStyleSetHeightPercent, YGNodeStyleSetHeightAuto);
  apply_dimension(node, style.min_size.width, YGNodeStyleSetMinWidth,
                  YGNodeStyleSetMinWidthPercent, nullptr);
  apply_dimension(node, style.min_size.height, YGNodeStyleSetMinHeight,
                  YGNodeStyleSetMinHeightPercent, nullptr);
  apply_dimension(node, style.max_size.width, YGNodeStyleSetMaxWidth,
                  YGNodeStyleSetMaxWidthPercent, nullptr);
  apply_dimension(node, style.max_size.height, YGNodeStyleSetMaxHeight,
                  YGNodeStyleSetMaxHeightPercent, nullptr);

  if (style.has_flex_basis) {
    apply_dimension(node, style.flex_basis, YGNodeStyleSetFlexBasis,
                    YGNodeStyleSetFlexBasisPercent,
                    YGNodeStyleSetFlexBasisAuto);
  } else {
    YGNodeStyleSetFlexBasisAuto(node);
  }

  if (style.has_flex_grow) {
    YGNodeStyleSetFlexGrow(node, style.flex_grow);
  }
  if (style.has_flex_shrink) {
    YGNodeStyleSetFlexShrink(node, style.flex_shrink);
  }

  if (style.has_aspect_ratio) {
    YGNodeStyleSetAspectRatio(node, style.aspect_ratio);
  } else {
    YGNodeStyleSetAspectRatio(node, YGUndefined);
  }

  if (style.gap.width.unit != YogaValueUnit::Undefined) {
    YGNodeStyleSetGap(node, YGGutterColumn,
                      to_yg_value(style.gap.width).value);
  }
  if (style.gap.height.unit != YogaValueUnit::Undefined) {
    YGNodeStyleSetGap(node, YGGutterRow, to_yg_value(style.gap.height).value);
  }
}

YGSize measure_proxy(YGNodeConstRef node, float width, YGMeasureMode width_mode,
                     float height, YGMeasureMode height_mode) {
  auto *ctx = static_cast<MeasureContext *>(YGNodeGetContext(node));
  if (!ctx) {
    return YGSize{0.0f, 0.0f};
  }

  YogaMeasureInput width_input{width, static_cast<YogaMeasureMode>(width_mode)};
  YogaMeasureInput height_input{height,
                                static_cast<YogaMeasureMode>(height_mode)};
  YogaSize result = yoga_measure(ctx->id, width_input, height_input);
  return YGSize{result.width, result.height};
}

} // namespace

YogaNodeHandle yoga_create_node() {
  return to_handle(YGNodeNew());
}

void yoga_free_node(YogaNodeHandle handle) {
  YGNodeRef node = from_handle(handle);
  if (!node) {
    return;
  }
  release_measure_recursive(node);
  YGNodeFreeRecursive(node);
}

void yoga_set_style(YogaNodeHandle handle, const YogaStyle &style) {
  YGNodeRef node = from_handle(handle);
  if (!node) {
    return;
  }
  apply_style(node, style);
}

void yoga_set_children(YogaNodeHandle parent_handle,
                       rust::Slice<const YogaNodeHandle> children) {
  YGNodeRef parent = from_handle(parent_handle);
  if (!parent) {
    return;
  }

  std::vector<YGNodeRef> refs;
  refs.reserve(children.size());
  for (const auto &child : children) {
    refs.push_back(from_handle(child));
  }

  if (refs.empty()) {
    YGNodeRemoveAllChildren(parent);
    return;
  }

  YGNodeSetChildren(parent, refs.data(),
                    static_cast<std::uint32_t>(refs.size()));
}

void yoga_mark_dirty(YogaNodeHandle handle) {
  YGNodeRef node = from_handle(handle);
  if (!node) {
    return;
  }
  YGNodeMarkDirty(node);
}

void yoga_set_measure(YogaNodeHandle handle, std::uint64_t measure_id) {
  YGNodeRef node = from_handle(handle);
  if (!node) {
    return;
  }

  if (measure_id == 0) {
    YGNodeSetMeasureFunc(node, nullptr);
    drop_measure_context(node);
    return;
  }
  drop_measure_context(node);

  auto *ctx = new MeasureContext{measure_id};
  YGNodeSetContext(node, ctx);
  YGNodeSetMeasureFunc(node, measure_proxy);
}

void yoga_clear_measure(YogaNodeHandle handle) {
  YGNodeRef node = from_handle(handle);
  if (!node) {
    return;
  }
  YGNodeSetMeasureFunc(node, nullptr);
  drop_measure_context(node);
}

void yoga_calculate_layout(YogaNodeHandle handle,
                           const YogaAvailableSize &available) {
  YGNodeRef node = from_handle(handle);
  if (!node) {
    return;
  }
  float width = value_or_undefined(available.width);
  float height = value_or_undefined(available.height);
  YGNodeCalculateLayout(node, width, height, YGDirectionLTR);
}

YogaLayout yoga_layout(YogaNodeHandle handle) {
  YGNodeRef node = from_handle(handle);
  if (!node) {
    return YogaLayout{0.0f, 0.0f, 0.0f, 0.0f};
  }

  return YogaLayout{
      YGNodeLayoutGetLeft(node),
      YGNodeLayoutGetTop(node),
      YGNodeLayoutGetWidth(node),
      YGNodeLayoutGetHeight(node),
  };
}

} // namespace gpui::yoga
