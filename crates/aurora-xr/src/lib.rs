//! OpenXR support (VR headsets): runtime discovery, head and controller
//! poses, and the per-eye views the renderer draws.
//!
//! The OpenXR loader is opened at run time, so a PC without a VR runtime
//! starts the viewer normally and simply reports that VR is unavailable.

#[cfg(not(target_os = "macos"))]
mod runtime;
#[cfg(not(target_os = "macos"))]
mod vulkan;

#[cfg(not(target_os = "macos"))]
pub use runtime::{XrRuntime, XrSystemInfo};
#[cfg(not(target_os = "macos"))]
pub use vulkan::XrGpu;

/// Why VR could not start.
#[derive(Debug, thiserror::Error)]
pub enum XrError {
    #[error("no OpenXR runtime found ({0})")]
    NoLoader(String),
    #[error("the OpenXR runtime does not support Vulkan (XR_KHR_vulkan_enable2)")]
    NoVulkan,
    #[error("no headset connected")]
    NoHeadset,
    #[error("OpenXR error: {0}")]
    Runtime(String),
    #[error("VR is not supported on this platform")]
    Unsupported,
}
