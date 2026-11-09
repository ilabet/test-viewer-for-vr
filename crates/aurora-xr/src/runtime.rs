use openxr as xr;

use crate::XrError;

/// The headset the OpenXR runtime reports, before any session exists.
#[derive(Debug, Clone)]
pub struct XrSystemInfo {
    pub runtime_name: String,
    pub system_name: String,
    /// Recommended per-eye render size.
    pub eye_size: (u32, u32),
    /// Lowest and highest Vulkan API versions the runtime accepts.
    pub vulkan_min: u32,
    pub vulkan_max: u32,
}

/// An OpenXR instance bound to a head-mounted display.
pub struct XrRuntime {
    pub instance: xr::Instance,
    pub system: xr::SystemId,
    pub info: XrSystemInfo,
}

impl XrRuntime {
    /// Opens the OpenXR loader and finds a headset. Fails cleanly (no panic)
    /// when no runtime or headset is present.
    pub fn open(app_name: &str) -> Result<Self, XrError> {
        // SAFETY: loading the system OpenXR loader library; it is only used
        // through the generated bindings.
        let entry = unsafe { xr::Entry::load(&()) }.map_err(|e| XrError::NoLoader(e.to_string()))?;
        let available = entry.enumerate_extensions().map_err(|e| XrError::Runtime(e.to_string()))?;
        if !available.khr_vulkan_enable2 {
            return Err(XrError::NoVulkan);
        }
        let mut enabled = xr::ExtensionSet::default();
        enabled.khr_vulkan_enable2 = true;
        let instance = entry
            .create_instance(
                &xr::ApplicationInfo {
                    application_name: app_name,
                    application_version: 0,
                    engine_name: "Aurora",
                    engine_version: 0,
                    api_version: xr::Version::new(1, 0, 0),
                },
                &enabled,
                &[],
                &(),
            )
            .map_err(|e| XrError::Runtime(e.to_string()))?;
        let props = instance.properties().map_err(|e| XrError::Runtime(e.to_string()))?;
        let system = instance.system(xr::FormFactor::HEAD_MOUNTED_DISPLAY).map_err(|e| match e {
            xr::sys::Result::ERROR_FORM_FACTOR_UNAVAILABLE => XrError::NoHeadset,
            e => XrError::Runtime(e.to_string()),
        })?;
        let sys_props = instance.system_properties(system).map_err(|e| XrError::Runtime(e.to_string()))?;
        let views = instance
            .enumerate_view_configuration_views(system, xr::ViewConfigurationType::PRIMARY_STEREO)
            .map_err(|e| XrError::Runtime(e.to_string()))?;
        let eye = views.first().ok_or(XrError::NoHeadset)?;
        let reqs = instance
            .graphics_requirements::<xr::Vulkan>(system)
            .map_err(|e| XrError::Runtime(e.to_string()))?;
        let info = XrSystemInfo {
            runtime_name: props.runtime_name,
            system_name: sys_props.system_name,
            eye_size: (eye.recommended_image_rect_width, eye.recommended_image_rect_height),
            vulkan_min: vk_version(reqs.min_api_version_supported),
            vulkan_max: vk_version(reqs.max_api_version_supported),
        };
        log::info!(
            "VR: {} on {} ({}x{} per eye)",
            info.system_name,
            info.runtime_name,
            info.eye_size.0,
            info.eye_size.1
        );
        Ok(Self { instance, system, info })
    }
}

fn vk_version(v: xr::Version) -> u32 {
    ash::vk::make_api_version(0, v.major() as u32, v.minor() as u32, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_runtime_is_an_error_not_a_panic() {
        // CI machines and most desktops have no OpenXR runtime: opening must
        // return an error (or succeed on a VR PC), never panic.
        let _ = XrRuntime::open("aurora-test");
    }
}
