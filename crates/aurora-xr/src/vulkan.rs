//! The GPU device for VR: OpenXR (XR_KHR_vulkan_enable2) creates the Vulkan
//! instance and device so the headset compositor can read the eye images,
//! and wgpu is built on top of them.

use std::ffi::c_char;

use ash::vk;
use ash::vk::Handle;
use openxr as xr;
use wgpu::hal::{self, api::Vulkan};

use crate::{XrError, XrRuntime};

type HalInstance = <Vulkan as hal::Api>::Instance;

/// wgpu objects created on the headset's GPU, plus the raw Vulkan handles
/// the OpenXR session needs.
pub struct XrGpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub vk_instance: vk::Instance,
    pub vk_physical_device: vk::PhysicalDevice,
    pub vk_device: vk::Device,
    pub queue_family: u32,
}

fn rt(e: impl std::fmt::Display) -> XrError {
    XrError::Runtime(e.to_string())
}

impl XrRuntime {
    /// Creates the wgpu device through the OpenXR runtime, with the features
    /// and limits the renderer asks for.
    pub fn create_gpu(&self, features: wgpu::Features, limits: &wgpu::Limits) -> Result<XrGpu, XrError> {
        let api_version = vk::API_VERSION_1_2.clamp(self.info.vulkan_min, self.info.vulkan_max.max(self.info.vulkan_min));
        // SAFETY: loads the system Vulkan loader; used only through ash.
        let entry = unsafe { ash::Entry::load() }.map_err(rt)?;
        let flags = wgpu::InstanceFlags::empty();
        let extensions = HalInstance::desired_extensions(&entry, api_version, flags).map_err(rt)?;
        let ext_ptrs: Vec<*const c_char> = extensions.iter().map(|e| e.as_ptr()).collect();
        let app = vk::ApplicationInfo::default().api_version(api_version);
        let create_info = vk::InstanceCreateInfo::default().application_info(&app).enabled_extension_names(&ext_ptrs);
        // SAFETY: the function pointer comes from the loaded Vulkan entry and
        // matches vkGetInstanceProcAddr; create_info outlives the call.
        let gipa: xr::sys::platform::VkGetInstanceProcAddr = unsafe { std::mem::transmute(entry.static_fn().get_instance_proc_addr) };
        let raw_instance = unsafe { self.instance.create_vulkan_instance(self.system, gipa, &create_info as *const _ as *const _) }
            .map_err(rt)?
            .map_err(|r| XrError::Runtime(format!("vkCreateInstance: {r}")))?;
        let vk_instance = vk::Instance::from_raw(raw_instance as u64);
        // SAFETY: the instance was just created with this entry.
        let ash_instance = unsafe { ash::Instance::load(entry.static_fn(), vk_instance) };
        // SAFETY: wgpu-hal takes ownership of a valid instance created with
        // exactly these extensions.
        let hal_instance = unsafe {
            HalInstance::from_raw(entry.clone(), ash_instance.clone(), api_version, 0, None, extensions, flags, Default::default(), false, None)
        }
        .map_err(rt)?;
        // SAFETY: asks the runtime which physical device drives the headset.
        let raw_phd = unsafe { self.instance.vulkan_graphics_device(self.system, raw_instance) }.map_err(rt)?;
        let vk_physical_device = vk::PhysicalDevice::from_raw(raw_phd as u64);
        let exposed = hal_instance.expose_adapter(vk_physical_device).ok_or_else(|| rt("the headset's GPU is not usable by wgpu"))?;

        // SAFETY: plain property query on a valid physical device.
        let families = unsafe { ash_instance.get_physical_device_queue_family_properties(vk_physical_device) };
        let queue_family = families
            .iter()
            .position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE))
            .ok_or_else(|| rt("no graphics queue"))? as u32;
        let dev_exts = exposed.adapter.required_device_extensions(features);
        let dev_ext_ptrs: Vec<*const c_char> = dev_exts.iter().map(|e| e.as_ptr()).collect();
        let mut phd_features = exposed.adapter.physical_device_features(&dev_exts, features);
        let priorities = [1.0f32];
        let queues = [vk::DeviceQueueCreateInfo::default().queue_family_index(queue_family).queue_priorities(&priorities)];
        let info = vk::DeviceCreateInfo::default().queue_create_infos(&queues).enabled_extension_names(&dev_ext_ptrs);
        let info = phd_features.add_to_device_create(info);
        // SAFETY: as above; the create info and its chains outlive the call.
        let raw_device = unsafe { self.instance.create_vulkan_device(self.system, gipa, raw_phd, &info as *const _ as *const _) }
            .map_err(rt)?
            .map_err(|r| XrError::Runtime(format!("vkCreateDevice: {r}")))?;
        let vk_device = vk::Device::from_raw(raw_device as u64);
        // SAFETY: the device was created on this instance.
        let ash_device = unsafe { ash::Device::load(ash_instance.fp_v1_0(), vk_device) };
        // SAFETY: wgpu-hal takes ownership of the device, created with these
        // extensions and features on queue family `queue_family`.
        let open = unsafe {
            exposed
                .adapter
                .device_from_raw(ash_device, None, &dev_exts, features, limits, &wgpu::MemoryHints::Performance, queue_family, 0)
        }
        .map_err(rt)?;
        // SAFETY: the hal objects are valid and handed over exactly once.
        let instance = unsafe { wgpu::Instance::from_hal::<Vulkan>(hal_instance) };
        let adapter = unsafe { instance.create_adapter_from_hal::<Vulkan>(exposed) };
        let (device, queue) = unsafe {
            adapter.create_device_from_hal::<Vulkan>(
                open,
                &wgpu::DeviceDescriptor {
                    label: Some("aurora-xr"),
                    required_features: features,
                    required_limits: limits.clone(),
                    experimental_features: wgpu::ExperimentalFeatures::disabled(),
                    memory_hints: wgpu::MemoryHints::Performance,
                    trace: wgpu::Trace::Off,
                },
            )
        }
        .map_err(rt)?;
        Ok(XrGpu { instance, adapter, device, queue, vk_instance, vk_physical_device, vk_device, queue_family })
    }
}
