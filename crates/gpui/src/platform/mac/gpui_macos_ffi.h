// Minimal C ABI between Swift AppKit layer and Rust GPUI core
// This header defines a very small callback table and a few commands
// to prove the shape of the integration without moving all code yet.

#ifndef GPUI_MACOS_FFI_H
#define GPUI_MACOS_FFI_H

#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

// Opaque window handle owned by Swift (NSWindow/NSView)
typedef void* GPUI_WindowHandle;

// Minimal window params for skeleton window creation
typedef struct GPUI_WindowParams {
    uint32_t width;
    uint32_t height;
    float scale;          // backing scale factor
    const char* title;    // optional UTF-8 title (nullable)
} GPUI_WindowParams;

// Callbacks Swift invokes to notify Rust about app/window events
typedef void (*gpui_on_app_will_finish_launching_t)(void);
typedef void (*gpui_on_app_did_finish_launching_t)(void);
typedef void (*gpui_on_window_resized_t)(GPUI_WindowHandle handle, uint32_t width, uint32_t height, float scale);

typedef void (*gpui_on_menu_action_t)(void* user_data, int tag);
typedef bool (*gpui_on_validate_menu_t)(void* user_data, int tag);

typedef struct GPUI_Callbacks {
    gpui_on_app_will_finish_launching_t on_app_will_finish_launching;
    gpui_on_app_did_finish_launching_t on_app_did_finish_launching;
    gpui_on_window_resized_t on_window_resized; // optional in this skeleton
    // Input callbacks (optional until wired)
    void (*on_mouse_event)(const struct GPUI_MouseEvent* ev);
    void (*on_key_event)(const struct GPUI_KeyEvent* ev);
    // Menus
    gpui_on_menu_action_t on_menu_action;
    gpui_on_validate_menu_t on_validate_menu;
    // Panels
    void (*on_open_panel_result)(void* user_data, uint64_t request_id, const uint8_t* json, size_t len);
    void (*on_save_panel_result)(void* user_data, uint64_t request_id, const uint8_t* json, size_t len);
    // Drag & drop (file URLs)
    void (*on_file_drop_event)(void* user_data, GPUI_WindowHandle window, int phase, float x, float y, const uint8_t* json, size_t len);
    // Window state callbacks
    void (*on_window_active_changed)(void* user_data, GPUI_WindowHandle window, bool active);
    void (*on_window_moved)(void* user_data, GPUI_WindowHandle window);
    void (*on_hover_changed)(void* user_data, GPUI_WindowHandle window, bool hovered);
    void (*on_window_visibility_changed)(void* user_data, GPUI_WindowHandle window, bool visible);
    void (*on_window_appearance_changed)(void* user_data, GPUI_WindowHandle window);
    // IME / Text input (window-scoped)
    bool (*ime_selected_range)(void* user_data, GPUI_WindowHandle window, unsigned int* loc, unsigned int* len, bool* reversed);
    bool (*ime_marked_range)(void* user_data, GPUI_WindowHandle window, unsigned int* loc, unsigned int* len);
    bool (*ime_text_for_range)(void* user_data, GPUI_WindowHandle window, unsigned int loc, unsigned int len,
                               const unsigned char** out_ptr, size_t* out_len,
                               unsigned int* out_adj_loc, unsigned int* out_adj_len);
    void (*ime_free_text)(const unsigned char* ptr, size_t len);
    void (*ime_replace_text_in_range)(void* user_data, GPUI_WindowHandle window,
                                      bool has_range, unsigned int loc, unsigned int len,
                                      const unsigned char* text, size_t text_len);
    void (*ime_replace_and_mark_text_in_range)(void* user_data, GPUI_WindowHandle window,
                                               bool has_range, unsigned int loc, unsigned int len,
                                               const unsigned char* text, size_t text_len,
                                               bool has_sel, unsigned int sel_loc, unsigned int sel_len);
    void (*ime_unmark_text)(void* user_data, GPUI_WindowHandle window);
    bool (*ime_bounds_for_range)(void* user_data, GPUI_WindowHandle window, unsigned int loc, unsigned int len,
                                 float* x, float* y, float* w, float* h);
} GPUI_Callbacks;

// Input data structures
typedef enum GPUI_MouseType {
    GPUI_MouseMove = 0,
    GPUI_MouseDown = 1,
    GPUI_MouseUp = 2,
    GPUI_MouseDrag = 3,
    GPUI_MouseScroll = 4,
} GPUI_MouseType;

typedef enum GPUI_MouseButton {
    GPUI_MouseButtonLeft = 0,
    GPUI_MouseButtonRight = 1,
    GPUI_MouseButtonMiddle = 2,
} GPUI_MouseButton;

typedef enum GPUI_KeyPhase {
    GPUI_KeyDown = 1,
    GPUI_KeyUp = 2,
    GPUI_FlagsChanged = 3,
} GPUI_KeyPhase;

enum {
    GPUI_ModShift = 1 << 0,
    GPUI_ModPlatform = 1 << 1,
    GPUI_ModControl = 1 << 2,
    GPUI_ModAlt = 1 << 3,
    GPUI_ModFunction = 1 << 4,
    GPUI_ModCapsLock = 1 << 5,
};

typedef struct GPUI_MouseEvent {
    GPUI_WindowHandle window;
    GPUI_MouseType type;
    GPUI_MouseButton button;
    float x, y;     // in points
    float dx, dy;   // movement or scroll delta
    unsigned int click_count;
    unsigned int modifiers; // bitmask of GPUI_Mod*
} GPUI_MouseEvent;

typedef struct GPUI_KeyEvent {
    GPUI_WindowHandle window;
    GPUI_KeyPhase phase;
    unsigned short key_code; // hardware code
    unsigned int unicode;    // first UTF-32 scalar, 0 if none
    unsigned int modifiers;  // bitmask of GPUI_Mod*
    bool is_repeat;
    const char* key;         // lowercased key string (ASCII when possible)
    const char* key_char;    // UTF-8 typed character if any, nullable
} GPUI_KeyEvent;

// Swift-implemented entry points (Rust -> Swift)
void gpui_macos_init(void* user_data, const GPUI_Callbacks* callbacks);
void gpui_macos_run(void);
void gpui_macos_quit(void);

// Create a window with a CAMetalLayer-backed NSView
// Returns the native window handle and the CAMetalLayer* via out-params
void gpui_macos_create_window(const GPUI_WindowParams* params,
                              GPUI_WindowHandle* out_handle,
                              void** out_cametal_layer);

// Menus
void gpui_macos_set_menus(const uint8_t* json, size_t len);
void gpui_macos_set_dock_menu(const uint8_t* json, size_t len);
void gpui_macos_open_panel(const uint8_t* json, size_t len, uint64_t request_id);
void gpui_macos_save_panel(const uint8_t* json, size_t len, uint64_t request_id);
// Cursor
void gpui_macos_set_cursor(int style, bool hide_until_mouse_moves);
// Window commands
void gpui_macos_window_set_title(GPUI_WindowHandle window, const uint8_t* utf8, size_t len);
void gpui_macos_window_minimize(GPUI_WindowHandle window);
void gpui_macos_window_zoom(GPUI_WindowHandle window);
void gpui_macos_window_toggle_fullscreen(GPUI_WindowHandle window);
bool gpui_macos_window_is_fullscreen(GPUI_WindowHandle window);

#ifdef __cplusplus
}
#endif

#endif // GPUI_MACOS_FFI_H
