pub mod call_settings;

#[cfg(feature = "rtc")]
mod call_impl;
#[cfg(feature = "rtc")]
pub use call_impl::*;

#[cfg(not(feature = "rtc"))]
mod call_stub;
#[cfg(not(feature = "rtc"))]
pub use call_stub::*;
