//! Environment module exposing locale and window metrics.
//! Provides cached getters and subscriptions for changes that
//! downstream crates can use (e.g., react-native-gpui turbomodules).

use crate::subscription::SubscriberSet;
use crate::{App, Global, Subscription};
use once_cell::sync::Lazy;
use std::sync::RwLock;

/// Locale information for the current user environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocaleInfo {
    /// BCP-47 identifier such as "en-US".
    pub identifier: String,
}

/// Logical window metrics describing size and scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowMetrics {
    /// Window content width in logical pixels.
    pub width: f32,
    /// Window content height in logical pixels.
    pub height: f32,
    /// Backing scale factor (device pixel ratio).
    pub scale: f32,
    /// Font scale multiplier (defaults to 1.0 on desktop).
    pub font_scale: f32,
}

struct EnvironmentState {
    locale: LocaleInfo,
    window_metrics: WindowMetrics,
    locale_observers: SubscriberSet<(), Box<dyn FnMut(&mut App)>>,
    window_metrics_observers: SubscriberSet<(), Box<dyn FnMut(&mut App)>>,
}

impl Global for EnvironmentState {}

/// Initialize environment defaults and register the global state.
///
/// This sets an initial locale and empty window metrics. The first
/// call happens during App initialization; metrics are updated by the
/// window sizing update loop.
pub fn init(cx: &mut App) {
    // Set defaults only once.
    if cx.try_global::<EnvironmentState>().is_none() {
        let locale = LocaleInfo {
            identifier: query_system_locale(),
        };
        let metrics = WindowMetrics {
            width: 0.0,
            height: 0.0,
            scale: 1.0,
            font_scale: 1.0,
        };
        let state = EnvironmentState {
            locale,
            window_metrics: metrics,
            locale_observers: SubscriberSet::new(),
            window_metrics_observers: SubscriberSet::new(),
        };
        cx.set_global(state);
        // Initialize static caches too, so non-App callers can read.
        {
            let mut l = CACHED_LOCALE.write().unwrap();
            *l = Some(cx.global::<EnvironmentState>().locale.clone());
        }
        {
            let mut m = CACHED_METRICS.write().unwrap();
            *m = Some(cx.global::<EnvironmentState>().window_metrics);
        }
    }
}

/// Returns the current locale.
pub fn current_locale(cx: &App) -> LocaleInfo {
    cx.global::<EnvironmentState>().locale.clone()
}

/// Returns the current window metrics as last observed.
pub fn current_window_metrics(cx: &App) -> WindowMetrics {
    cx.global::<EnvironmentState>().window_metrics
}

/// Update the current locale and notify observers if it changed.
pub fn set_locale(cx: &mut App, new_locale: LocaleInfo) {
    let state = cx.global_mut::<EnvironmentState>();
    if state.locale != new_locale {
        state.locale = new_locale;
        {
            let mut l = CACHED_LOCALE.write().unwrap();
            *l = Some(state.locale.clone());
        }
        state.locale_observers.clone().retain(&(), |callback| {
            (callback)(cx);
            true
        });
    }
}

/// Update the current window metrics and notify observers if changed.
pub fn set_window_metrics(cx: &mut App, new_metrics: WindowMetrics) {
    let state = cx.global_mut::<EnvironmentState>();
    if state.window_metrics != new_metrics {
        state.window_metrics = new_metrics;
        {
            let mut m = CACHED_METRICS.write().unwrap();
            *m = Some(state.window_metrics);
        }
        state
            .window_metrics_observers
            .clone()
            .retain(&(), |callback| {
                (callback)(cx);
                true
            });
    }
}

// Static caches so non-App callers (e.g., host code) can read quickly.
static CACHED_LOCALE: Lazy<RwLock<Option<LocaleInfo>>> = Lazy::new(|| RwLock::new(None));
static CACHED_METRICS: Lazy<RwLock<Option<WindowMetrics>>> = Lazy::new(|| RwLock::new(None));

/// Returns the last cached locale without requiring an App reference.
pub fn cached_locale() -> Option<LocaleInfo> {
    CACHED_LOCALE.read().unwrap().clone()
}

/// Returns the last cached window metrics without requiring an App reference.
pub fn cached_window_metrics() -> Option<WindowMetrics> {
    CACHED_METRICS.read().unwrap().clone()
}

/// Subscribe to locale changes. Callback can read `current_locale(cx)`.
pub fn on_locale_changed<F>(cx: &App, mut callback: F) -> Subscription
where
    F: 'static + FnMut(&mut App),
{
    let state = cx.global::<EnvironmentState>();
    let (subscription, activate) = state.locale_observers.insert(
        (),
        Box::new(move |cx| {
            callback(cx);
        }),
    );
    activate();
    subscription
}

/// Subscribe to window metrics changes. Callback can read `current_window_metrics(cx)`.
pub fn on_window_metrics_changed<F>(cx: &App, mut callback: F) -> Subscription
where
    F: 'static + FnMut(&mut App),
{
    let state = cx.global::<EnvironmentState>();
    let (subscription, activate) = state.window_metrics_observers.insert(
        (),
        Box::new(move |cx| {
            callback(cx);
        }),
    );
    activate();
    subscription
}

#[cfg(target_os = "windows")]
fn query_system_locale() -> String {
    use windows::Win32::Globalization::GetUserDefaultLocaleName;
    use windows::Win32::System::SystemServices::LOCALE_NAME_MAX_LENGTH;
    let mut buf = vec![0u16; LOCALE_NAME_MAX_LENGTH as usize];
    unsafe { GetUserDefaultLocaleName(&mut buf) };
    let s = String::from_utf16_lossy(&buf);
    s.trim_matches(char::from(0)).to_string()
}

#[cfg(target_os = "macos")]
fn query_system_locale() -> String {
    use core_foundation::array::CFArray;
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;
    use core_foundation_sys::locale::CFLocaleCopyPreferredLanguages;
    unsafe {
        let arr: CFArray<CFString> =
            CFArray::wrap_under_create_rule(CFLocaleCopyPreferredLanguages());
        if let Some(first) = arr.get(0) {
            first.to_string()
        } else {
            "en-US".to_string()
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn query_system_locale() -> String {
    use std::env;
    use std::ffi::OsString;
    if let Some(locale) = env::var_os("LC_ALL").or_else(|| env::var_os("LC_CTYPE")) {
        locale.to_string_lossy().to_string()
    } else {
        OsString::from("C").to_string_lossy().to_string()
    }
}
