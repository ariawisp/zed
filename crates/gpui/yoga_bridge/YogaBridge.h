// SPDX-License-Identifier: MIT
#pragma once

#include <cstdint>
#include "gpui/src/yoga/ffi.rs.h"
#include <rust/cxx.h>

namespace gpui::yoga {

YogaNodeHandle yoga_create_node();
void yoga_free_node(YogaNodeHandle node);
void yoga_set_style(YogaNodeHandle node, const YogaStyle &style);
void yoga_set_children(YogaNodeHandle parent,
                       rust::Slice<const YogaNodeHandle> children);
void yoga_mark_dirty(YogaNodeHandle node);
void yoga_set_measure(YogaNodeHandle node, std::uint64_t measure_id);
void yoga_clear_measure(YogaNodeHandle node);
void yoga_calculate_layout(YogaNodeHandle node,
                           const YogaAvailableSize &available);
YogaLayout yoga_layout(YogaNodeHandle node);

} // namespace gpui::yoga
