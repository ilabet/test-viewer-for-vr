//! VR headsets through OpenXR (AURORA_VR=1, built with the `openxr`
//! feature): the device is created through the runtime, each eye is drawn
//! offscreen and copied into the headset's image. Without a runtime or a
//! headset the viewer stays on the desktop.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::RenderError;
use crate::main_thread::{OpenedDevice, open_device};

static OFF: AtomicBool = AtomicBool::new(false);

/// No headset for the rest of this run (its GPU could not start): the next
/// renderer is a desktop one.
pub fn turn_off() {
    OFF.store(true, Ordering::Relaxed);
}

/// AURORA_VR=1 asks for a real headset (not the simulated one), and it is
/// not turned off.
pub fn headset_asked() -> bool {
    !OFF.load(Ordering::Relaxed)
        && wanted(std::env::var("AURORA_VR").ok().as_deref())
        && !wanted(std::env::var("AURORA_VR_SIM").ok().as_deref())
}

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
        /// AURORA_VR_SIM=1: a simulated headset on the desktop GPU.
        sim: bool,
    }

    impl Pending {
        pub fn open() -> Self {
            if wanted(std::env::var("AURORA_VR_SIM").ok().as_deref()) {
                log::info!("VR: simulated headset (AURORA_VR_SIM)");
                return Pending {
                    opened: None,
                    gpu: None,
                    sim: true,
                };
            }
            let opened = if headset_asked() {
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
            Pending {
                opened,
                gpu: None,
                sim: false,
            }
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
            self.gpu.is_some() || self.sim
        }

        /// Starts the session, its images matching the window's size and format.
        pub fn start(self, size: (u32, u32), format: wgpu::TextureFormat, device: &wgpu::Device) -> Option<XrState> {
            if self.sim {
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("simulated headset"),
                    size: wgpu::Extent3d {
                        width: size.0.max(1),
                        height: size.1.max(1),
                        depth_or_array_layers: 2,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                });
                return Some(XrState {
                    real: None,
                    sim: Some((texture, std::time::Instant::now())),
                    frame: None,
                });
            }
            let (runtime, gpu) = self.gpu?;
            match XrSession::new(&runtime, &gpu, size, format) {
                Ok(session) => Some(XrState {
                    real: Some((runtime, gpu, session)),
                    sim: None,
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
        real: Option<(XrRuntime, XrGpu, XrSession)>,
        /// The simulated headset's two eye images and its start.
        sim: Option<(wgpu::Texture, std::time::Instant)>,
        frame: Option<XrFrame>,
    }

    /// A head 1.6 m above the floor looking slowly around, and a right
    /// controller held forward, pointing a little down.
    fn sim_frame(t: f32) -> XrFrame {
        use aurora_xr::{EyeView, XrInput};
        use glam::{Quat, Vec3};
        let head = Vec3::new(0.0, 1.6, 0.0);
        let look = Quat::from_rotation_y((t * 0.5).sin() * 0.3) * Quat::from_rotation_x((t * 0.37).sin() * 0.08);
        let eye = |dx: f32| EyeView {
            position: head + look * Vec3::new(dx, 0.0, 0.0),
            orientation: look,
            fov: [-0.8, 0.8, 0.7, -0.7],
        };
        let hand = head + look * Vec3::new(0.2, -0.35, -0.25);
        XrFrame {
            display_time: aurora_xr::xr::Time::from_nanos(0),
            eyes: [eye(-0.032), eye(0.032)],
            image: 0,
            input: XrInput {
                aim: [None, Some((hand, look * Vec3::new(0.0, -0.12, -1.0).normalize()))],
                ..Default::default()
            },
        }
    }

    impl XrState {
        /// Waits for the headset's next frame: its eyes and controllers, or
        /// `None` when nothing is to be drawn for it.
        pub fn begin(&mut self) -> Option<XrFrame> {
            if let Some((_, start)) = &self.sim {
                self.frame = Some(sim_frame(start.elapsed().as_secs_f32()));
                return self.frame.clone();
            }
            let (runtime, _, session) = self.real.as_mut()?;
            match session.poll(runtime) {
                Ok(true) => {}
                Ok(false) => return None,
                Err(e) => {
                    log::warn!("VR: {e}");
                    return None;
                }
            }
            if !session.running() {
                return None;
            }
            match session.begin_frame() {
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
            let texture = match (&self.sim, &self.real) {
                (Some((t, _)), _) => t.clone(),
                (None, Some((_, _, session))) => session.targets[frame.image].clone(),
                (None, None) => return None,
            };
            Some(XrEyeTarget { texture, layer })
        }

        /// Hands the frame's two eyes to the headset.
        pub fn end(&mut self) {
            if self.frame.take().is_some()
                && let Some((_, _, session)) = self.real.as_mut()
                && let Err(e) = session.end_frame()
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

        pub fn start(self, _size: (u32, u32), _format: wgpu::TextureFormat, _device: &wgpu::Device) -> Option<XrState> {
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
