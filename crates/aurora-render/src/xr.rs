//! VR headsets through OpenXR (AURORA_VR=1, built with the `openxr`
//! feature): the device is created through the runtime, each eye is drawn
//! offscreen and copied into the headset's image. Without a runtime or a
//! headset the viewer stays on the desktop.

use crate::RenderError;
use crate::main_thread::{OpenedDevice, open_device};

/// AURORA_VR=1 asks for the headset; anything else, or nothing, leaves the
/// viewer on the desktop.
pub fn wanted(var: Option<&str>) -> bool {
    matches!(var.map(str::trim), Some(v) if !matches!(v, "" | "0" | "off" | "false" | "no"))
}

#[cfg(all(feature = "openxr", not(target_os = "macos")))]
mod imp {
    use super::*;
    use crate::main_thread::open_device_with;
    use crate::packet::XrEyeTarget;
    use aurora_xr::{XrAdapter, XrFrame, XrGpu, XrRuntime, XrSession};

    /// The runtime between its discovery and the start of the session.
    pub struct Pending {
        opened: Option<(XrRuntime, XrAdapter)>,
        gpu: Option<(XrRuntime, XrGpu)>,
    }

    impl Pending {
        pub fn open() -> Self {
            let opened = if wanted(std::env::var("AURORA_VR").ok().as_deref()) {
                match XrRuntime::open("Aurora").and_then(|rt| rt.create_adapter().map(|a| (rt, a))) {
                    Ok(o) => Some(o),
                    Err(e) => {
                        log::warn!("VR: {e}; staying on the desktop");
                        None
                    }
                }
            } else {
                None
            };
            Pending { opened, gpu: None }
        }

        /// The instance and adapter of the headset's GPU, used instead of
        /// the ones enumerated.
        pub fn adapter_override(&self) -> Option<(wgpu::Instance, wgpu::Adapter)> {
            self.opened.as_ref().map(|(_, a)| (a.instance.clone(), a.adapter.clone()))
        }

        pub fn open_device(&mut self, adapter: &wgpu::Adapter) -> Result<OpenedDevice, RenderError> {
            let Some((rt, xa)) = self.opened.take() else {
                return open_device(adapter);
            };
            let mut gpu = None;
            let opened = open_device_with(adapter, |_, desc| {
                let g = xa
                    .open(&rt, desc.required_features, &desc.required_limits)
                    .map_err(|e| RenderError::Device(e.to_string()))?;
                let pair = (g.device.clone(), g.queue.clone());
                gpu = Some(g);
                Ok(pair)
            });
            if let Some(g) = gpu {
                self.gpu = Some((rt, g));
            }
            opened
        }

        pub fn active(&self) -> bool {
            self.gpu.is_some()
        }

        /// Starts the session, its images matching the window's size and format.
        pub fn start(self, size: (u32, u32), format: wgpu::TextureFormat) -> Option<XrState> {
            let (runtime, gpu) = self.gpu?;
            match XrSession::new(&runtime, &gpu, size, format) {
                Ok(session) => Some(XrState {
                    runtime,
                    _gpu: gpu,
                    session,
                    frame: None,
                }),
                Err(e) => {
                    log::warn!("VR: {e}; staying on the desktop");
                    None
                }
            }
        }
    }

    pub struct XrState {
        runtime: XrRuntime,
        _gpu: XrGpu,
        session: XrSession,
        frame: Option<XrFrame>,
    }

    impl XrState {
        /// Waits for the headset's next frame: its eyes and controllers, or
        /// `None` when nothing is to be drawn for it.
        pub fn begin(&mut self) -> Option<XrFrame> {
            match self.session.poll(&self.runtime) {
                Ok(true) => {}
                Ok(false) => return None,
                Err(e) => {
                    log::warn!("VR: {e}");
                    return None;
                }
            }
            if !self.session.running() {
                return None;
            }
            match self.session.begin_frame() {
                Ok(frame) => {
                    self.frame = frame.clone();
                    frame
                }
                Err(e) => {
                    log::warn!("VR: {e}");
                    None
                }
            }
        }

        pub fn eye_target(&self, layer: u32) -> Option<XrEyeTarget> {
            let frame = self.frame.as_ref()?;
            Some(XrEyeTarget {
                texture: self.session.targets[frame.image].clone(),
                layer,
            })
        }

        /// Hands the frame's two eyes to the headset.
        pub fn end(&mut self) {
            if self.frame.take().is_some()
                && let Err(e) = self.session.end_frame()
            {
                log::warn!("VR: {e}");
            }
        }
    }
}

#[cfg(not(all(feature = "openxr", not(target_os = "macos"))))]
mod imp {
    use super::*;

    pub struct Pending;

    impl Pending {
        pub fn open() -> Self {
            if wanted(std::env::var("AURORA_VR").ok().as_deref()) {
                log::warn!("VR: AURORA_VR is set but this build has no OpenXR support (the `openxr` feature)");
            }
            Pending
        }

        pub fn adapter_override(&self) -> Option<(wgpu::Instance, wgpu::Adapter)> {
            None
        }

        pub fn open_device(&mut self, adapter: &wgpu::Adapter) -> Result<OpenedDevice, RenderError> {
            open_device(adapter)
        }

        pub fn active(&self) -> bool {
            false
        }

        pub fn start(self, _size: (u32, u32), _format: wgpu::TextureFormat) -> Option<XrState> {
            None
        }
    }

    pub enum XrState {}
}

pub use imp::{Pending, XrState};

#[cfg(test)]
mod tests {
    use super::wanted;

    #[test]
    fn vr_is_opt_in() {
        assert!(!wanted(None));
        assert!(!wanted(Some("")));
        assert!(!wanted(Some("0")));
        assert!(!wanted(Some(" off ")));
        assert!(wanted(Some("1")));
        assert!(wanted(Some("yes")));
    }
}
