//! The running VR session: per-eye swapchain, frame timing, head and eye
//! poses, and the controller actions (VRChat-style bindings).

use ash::vk;
use ash::vk::Handle;
use glam::{Mat4, Quat, Vec2, Vec3};
use openxr as xr;
use wgpu::hal::{self, api::Vulkan};

use crate::{XrError, XrGpu, XrRuntime};

const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;

fn rt(e: impl std::fmt::Display) -> XrError {
    XrError::Runtime(e.to_string())
}

/// One eye of a frame, in the stage space (metres, Y up, -Z forward).
#[derive(Debug, Clone, Copy)]
pub struct EyeView {
    pub position: Vec3,
    pub orientation: Quat,
    /// Field of view half-angles (radians): left, right, up, down.
    pub fov: [f32; 4],
}

impl EyeView {
    /// World-from-eye inverse (view matrix) in stage space.
    pub fn view(&self) -> Mat4 {
        Mat4::from_rotation_translation(self.orientation, self.position).inverse()
    }

    /// Off-axis, reversed-Z-free right-handed projection (depth 0..1).
    pub fn projection(&self, near: f32, far: f32) -> Mat4 {
        let [l, r, u, d] = self.fov.map(f32::tan);
        let (w, h) = (r - l, u - d);
        Mat4::from_cols_array(&[
            2.0 / w,
            0.0,
            0.0,
            0.0,
            0.0,
            2.0 / h,
            0.0,
            0.0,
            (r + l) / w,
            (u + d) / h,
            far / (near - far),
            -1.0,
            0.0,
            0.0,
            near * far / (near - far),
            0.0,
        ])
    }
}

/// Controller state of a frame, VRChat-style: left stick moves, right stick
/// turns, A/X jump (double tap flies), B/Y menu, trigger interacts, grip sits.
#[derive(Debug, Clone, Copy, Default)]
pub struct XrInput {
    pub move_stick: Vec2,
    pub turn_stick: Vec2,
    pub jump: bool,
    pub descend: bool,
    pub menu: bool,
    pub trigger: [bool; 2],
    pub grip: [bool; 2],
    /// Aim ray of each hand (origin, direction) in stage space, when tracked.
    pub aim: [Option<(Vec3, Vec3)>; 2],
}

/// A frame the renderer must draw: two eyes into `targets[image]` layers 0/1.
pub struct XrFrame {
    pub display_time: xr::Time,
    pub eyes: [EyeView; 2],
    pub image: usize,
    pub input: XrInput,
}

struct Actions {
    set: xr::ActionSet,
    move_stick: xr::Action<xr::Vector2f>,
    turn_stick: xr::Action<xr::Vector2f>,
    jump: xr::Action<bool>,
    descend: xr::Action<bool>,
    menu: xr::Action<bool>,
    trigger: xr::Action<bool>,
    grip: xr::Action<bool>,
    aim: xr::Action<xr::Posef>,
    hands: [xr::Path; 2],
    aim_spaces: [xr::Space; 2],
}

pub struct XrSession {
    session: xr::Session<xr::Vulkan>,
    frame_wait: xr::FrameWaiter,
    frame_stream: xr::FrameStream<xr::Vulkan>,
    stage: xr::Space,
    swapchain: xr::Swapchain<xr::Vulkan>,
    /// One 2-layer texture per swapchain image (layer 0 left eye, 1 right).
    pub targets: Vec<wgpu::Texture>,
    pub format: wgpu::TextureFormat,
    pub eye_size: (u32, u32),
    actions: Actions,
    running: bool,
    pending: Option<(xr::Time, [xr::View; 2])>,
    blend: xr::EnvironmentBlendMode,
}

fn pick_format(formats: &[u32]) -> Option<(u32, wgpu::TextureFormat)> {
    [
        (vk::Format::R8G8B8A8_SRGB, wgpu::TextureFormat::Rgba8UnormSrgb),
        (vk::Format::B8G8R8A8_SRGB, wgpu::TextureFormat::Bgra8UnormSrgb),
    ]
    .into_iter()
    .find(|(vkf, _)| formats.contains(&(vkf.as_raw() as u32)))
    .map(|(vkf, f)| (vkf.as_raw() as u32, f))
}

impl XrSession {
    pub fn new(rt_: &XrRuntime, gpu: &XrGpu) -> Result<Self, XrError> {
        let inst = &rt_.instance;
        // SAFETY: the handles come from XrRuntime::create_gpu on this system.
        let (session, frame_wait, frame_stream) = unsafe {
            inst.create_session::<xr::Vulkan>(
                rt_.system,
                &xr::vulkan::SessionCreateInfo {
                    instance: gpu.vk_instance.as_raw() as _,
                    physical_device: gpu.vk_physical_device.as_raw() as _,
                    device: gpu.vk_device.as_raw() as _,
                    queue_family_index: gpu.queue_family,
                    queue_index: 0,
                },
            )
        }
        .map_err(rt)?;
        let stage = session
            .create_reference_space(xr::ReferenceSpaceType::STAGE, xr::Posef::IDENTITY)
            .or_else(|_| session.create_reference_space(xr::ReferenceSpaceType::LOCAL, xr::Posef::IDENTITY))
            .map_err(rt)?;
        let (vk_format, format) = pick_format(&session.enumerate_swapchain_formats().map_err(rt)?).ok_or_else(|| rt("no sRGB swapchain format"))?;
        let eye_size = rt_.info.eye_size;
        let swapchain = session
            .create_swapchain(&xr::SwapchainCreateInfo {
                create_flags: xr::SwapchainCreateFlags::EMPTY,
                usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT | xr::SwapchainUsageFlags::TRANSFER_DST,
                format: vk_format,
                sample_count: 1,
                width: eye_size.0,
                height: eye_size.1,
                face_count: 1,
                array_size: 2,
                mip_count: 1,
            })
            .map_err(rt)?;
        let size = wgpu::Extent3d { width: eye_size.0, height: eye_size.1, depth_or_array_layers: 2 };
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST;
        let mut targets = Vec::new();
        for image in swapchain.enumerate_images().map_err(rt)? {
            // SAFETY: the image belongs to the runtime's swapchain (external
            // memory) and lives as long as the swapchain, which outlives the
            // textures (dropped first in Drop order below).
            let tex = unsafe {
                let hal_dev = gpu.device.as_hal::<Vulkan>().ok_or_else(|| rt("not a Vulkan device"))?;
                let raw = hal_dev.texture_from_raw(
                    vk::Image::from_raw(image),
                    &hal::TextureDescriptor {
                        label: Some("xr eye"),
                        size,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUses::COLOR_TARGET | wgpu::TextureUses::COPY_DST,
                        memory_flags: hal::MemoryFlags::empty(),
                        view_formats: vec![],
                    },
                    None,
                    hal::vulkan::TextureMemory::External,
                );
                gpu.device.create_texture_from_hal::<Vulkan>(
                    raw,
                    &wgpu::TextureDescriptor {
                        label: Some("xr eye"),
                        size,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage,
                        view_formats: &[],
                    },
                    wgpu::TextureUses::UNINITIALIZED,
                )
            };
            targets.push(tex);
        }
        let actions = Actions::new(inst, &session)?;
        let blend = inst
            .enumerate_environment_blend_modes(rt_.system, VIEW_TYPE)
            .map_err(rt)?
            .first()
            .copied()
            .unwrap_or(xr::EnvironmentBlendMode::OPAQUE);
        Ok(Self { session, frame_wait, frame_stream, stage, swapchain, targets, format, eye_size, actions, running: false, pending: None, blend })
    }

    /// Handles runtime events; returns false once the session must close.
    pub fn poll(&mut self, rt_: &XrRuntime) -> Result<bool, XrError> {
        let mut storage = xr::EventDataBuffer::new();
        while let Some(event) = rt_.instance.poll_event(&mut storage).map_err(rt)? {
            match event {
                xr::Event::SessionStateChanged(e) => match e.state() {
                    xr::SessionState::READY => {
                        self.session.begin(VIEW_TYPE).map_err(rt)?;
                        self.running = true;
                    }
                    xr::SessionState::STOPPING => {
                        self.session.end().map_err(rt)?;
                        self.running = false;
                    }
                    xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => return Ok(false),
                    _ => {}
                },
                xr::Event::InstanceLossPending(_) => return Ok(false),
                _ => {}
            }
        }
        Ok(true)
    }

    pub fn running(&self) -> bool {
        self.running
    }

    /// Waits for the runtime's frame slot. `None`: nothing to draw this
    /// time (the frame is already submitted empty).
    pub fn begin_frame(&mut self) -> Result<Option<XrFrame>, XrError> {
        if !self.running {
            return Ok(None);
        }
        let state = self.frame_wait.wait().map_err(rt)?;
        self.frame_stream.begin().map_err(rt)?;
        if !state.should_render {
            self.frame_stream.end(state.predicted_display_time, self.blend, &[]).map_err(rt)?;
            return Ok(None);
        }
        let t = state.predicted_display_time;
        let input = self.actions.read(&self.session, &self.stage, t)?;
        let (_, views) = self.session.locate_views(VIEW_TYPE, t, &self.stage).map_err(rt)?;
        let views: [xr::View; 2] = [views[0], views[1]];
        let image = self.swapchain.acquire_image().map_err(rt)? as usize;
        self.swapchain.wait_image(xr::Duration::INFINITE).map_err(rt)?;
        self.pending = Some((t, views));
        Ok(Some(XrFrame { display_time: t, eyes: views.map(eye), image, input }))
    }

    /// Hands the drawn eyes to the compositor. Call after the queue
    /// submission that draws into `targets[frame.image]`.
    pub fn end_frame(&mut self) -> Result<(), XrError> {
        let Some((t, views)) = self.pending.take() else { return Ok(()) };
        self.swapchain.release_image().map_err(rt)?;
        let rect = xr::Rect2Di {
            offset: xr::Offset2Di { x: 0, y: 0 },
            extent: xr::Extent2Di { width: self.eye_size.0 as i32, height: self.eye_size.1 as i32 },
        };
        let proj = |i: usize| {
            xr::CompositionLayerProjectionView::new()
                .pose(views[i].pose)
                .fov(views[i].fov)
                .sub_image(xr::SwapchainSubImage::new().swapchain(&self.swapchain).image_array_index(i as u32).image_rect(rect))
        };
        let layer_views = [proj(0), proj(1)];
        let layer = xr::CompositionLayerProjection::new().space(&self.stage).views(&layer_views);
        self.frame_stream.end(t, self.blend, &[&layer]).map_err(rt)
    }
}

fn eye(v: xr::View) -> EyeView {
    let p = v.pose;
    EyeView {
        position: Vec3::new(p.position.x, p.position.y, p.position.z),
        orientation: Quat::from_xyzw(p.orientation.x, p.orientation.y, p.orientation.z, p.orientation.w),
        fov: [v.fov.angle_left, v.fov.angle_right, v.fov.angle_up, v.fov.angle_down],
    }
}

impl Actions {
    fn new(inst: &xr::Instance, session: &xr::Session<xr::Vulkan>) -> Result<Self, XrError> {
        let path = |s: &str| inst.string_to_path(s).map_err(rt);
        let hands = [path("/user/hand/left")?, path("/user/hand/right")?];
        let set = inst.create_action_set("aurora", "Aurora", 0).map_err(rt)?;
        let move_stick = set.create_action::<xr::Vector2f>("move", "Move", &[]).map_err(rt)?;
        let turn_stick = set.create_action::<xr::Vector2f>("turn", "Turn", &[]).map_err(rt)?;
        let jump = set.create_action::<bool>("jump", "Jump / fly up", &[]).map_err(rt)?;
        let descend = set.create_action::<bool>("descend", "Fly down", &[]).map_err(rt)?;
        let menu = set.create_action::<bool>("menu", "Menu", &[]).map_err(rt)?;
        let trigger = set.create_action::<bool>("interact", "Interact", &hands).map_err(rt)?;
        let grip = set.create_action::<bool>("sit", "Sit / grab", &hands).map_err(rt)?;
        let aim = set.create_action::<xr::Posef>("aim", "Aim", &hands).map_err(rt)?;
        // Touch-style controllers (Quest, Index, Pico, WMR all map to these).
        let touch = [
            ("/interaction_profiles/oculus/touch_controller", "a/click", "x/click", "b/click", "y/click", "trigger/value", "squeeze/value"),
            ("/interaction_profiles/valve/index_controller", "a/click", "a/click", "b/click", "b/click", "trigger/click", "squeeze/value"),
            ("/interaction_profiles/bytedance/pico4_controller", "a/click", "x/click", "b/click", "y/click", "trigger/value", "squeeze/value"),
        ];
        for (profile, a, x, b, y, trig, squeeze) in touch {
            let Ok(profile) = path(profile) else { continue };
            let mut b_ = Vec::new();
            b_.push(xr::Binding::new(&move_stick, path("/user/hand/left/input/thumbstick")?));
            b_.push(xr::Binding::new(&turn_stick, path("/user/hand/right/input/thumbstick")?));
            b_.push(xr::Binding::new(&jump, path(&format!("/user/hand/right/input/{a}"))?));
            b_.push(xr::Binding::new(&descend, path(&format!("/user/hand/left/input/{x}"))?));
            b_.push(xr::Binding::new(&menu, path(&format!("/user/hand/right/input/{b}"))?));
            b_.push(xr::Binding::new(&menu, path(&format!("/user/hand/left/input/{y}"))?));
            for h in ["left", "right"] {
                b_.push(xr::Binding::new(&trigger, path(&format!("/user/hand/{h}/input/{trig}"))?));
                b_.push(xr::Binding::new(&grip, path(&format!("/user/hand/{h}/input/{squeeze}"))?));
                b_.push(xr::Binding::new(&aim, path(&format!("/user/hand/{h}/input/aim/pose"))?));
            }
            // Unknown profiles on older runtimes are skipped, not fatal.
            if let Err(e) = inst.suggest_interaction_profile_bindings(profile, &b_) {
                log::debug!("VR bindings not accepted for a profile: {e}");
            }
        }
        // HTC Vive wands: trackpad moves/turns, menu button opens the menu.
        if let Ok(vive) = path("/interaction_profiles/htc/vive_controller") {
            let mut b_ = vec![
                xr::Binding::new(&move_stick, path("/user/hand/left/input/trackpad")?),
                xr::Binding::new(&turn_stick, path("/user/hand/right/input/trackpad")?),
                xr::Binding::new(&jump, path("/user/hand/right/input/trackpad/click")?),
                xr::Binding::new(&descend, path("/user/hand/left/input/trackpad/click")?),
                xr::Binding::new(&menu, path("/user/hand/left/input/menu/click")?),
                xr::Binding::new(&menu, path("/user/hand/right/input/menu/click")?),
            ];
            for h in ["left", "right"] {
                b_.push(xr::Binding::new(&trigger, path(&format!("/user/hand/{h}/input/trigger/click"))?));
                b_.push(xr::Binding::new(&grip, path(&format!("/user/hand/{h}/input/squeeze/click"))?));
                b_.push(xr::Binding::new(&aim, path(&format!("/user/hand/{h}/input/aim/pose"))?));
            }
            if let Err(e) = inst.suggest_interaction_profile_bindings(vive, &b_) {
                log::debug!("VR bindings not accepted for Vive wands: {e}");
            }
        }
        session.attach_action_sets(&[&set]).map_err(rt)?;
        let aim_spaces = [
            aim.create_space(session, hands[0], xr::Posef::IDENTITY).map_err(rt)?,
            aim.create_space(session, hands[1], xr::Posef::IDENTITY).map_err(rt)?,
        ];
        Ok(Self { set, move_stick, turn_stick, jump, descend, menu, trigger, grip, aim, hands, aim_spaces })
    }

    fn read(&self, session: &xr::Session<xr::Vulkan>, stage: &xr::Space, t: xr::Time) -> Result<XrInput, XrError> {
        session.sync_actions(&[(&self.set).into()]).map_err(rt)?;
        let v2 = |a: &xr::Action<xr::Vector2f>| a.state(session, xr::Path::NULL).map(|s| Vec2::new(s.current_state.x, s.current_state.y)).unwrap_or_default();
        let b = |a: &xr::Action<bool>, p: xr::Path| a.state(session, p).map(|s| s.current_state).unwrap_or(false);
        let mut input = XrInput {
            move_stick: v2(&self.move_stick),
            turn_stick: v2(&self.turn_stick),
            jump: b(&self.jump, xr::Path::NULL),
            descend: b(&self.descend, xr::Path::NULL),
            menu: b(&self.menu, xr::Path::NULL),
            ..Default::default()
        };
        for i in 0..2 {
            input.trigger[i] = b(&self.trigger, self.hands[i]);
            input.grip[i] = b(&self.grip, self.hands[i]);
            if let Ok(loc) = self.aim_spaces[i].locate(stage, t)
                && loc.location_flags.contains(xr::SpaceLocationFlags::POSITION_VALID | xr::SpaceLocationFlags::ORIENTATION_VALID)
            {
                let p = loc.pose;
                let q = Quat::from_xyzw(p.orientation.x, p.orientation.y, p.orientation.z, p.orientation.w);
                input.aim[i] = Some((Vec3::new(p.position.x, p.position.y, p.position.z), q * Vec3::NEG_Z));
            }
        }
        let _ = &self.aim;
        Ok(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symmetric_projection_matches_perspective() {
        let half = 45f32.to_radians();
        let e = EyeView { position: Vec3::ZERO, orientation: Quat::IDENTITY, fov: [-half, half, half, -half] };
        let a = e.projection(0.1, 100.0);
        // right-handed perspective, depth 0..1, 90 degrees, square
        let (n, f) = (0.1f32, 100.0f32);
        let b = Mat4::from_cols_array(&[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, f / (n - f), -1.0, 0.0, 0.0, n * f / (n - f), 0.0]);
        assert!(a.abs_diff_eq(b, 1e-4), "{a:?} vs {b:?}");
    }

    #[test]
    fn view_is_inverse_of_pose() {
        let e = EyeView { position: Vec3::new(1.0, 1.6, 0.0), orientation: Quat::from_rotation_y(0.5), fov: [-1.0, 1.0, 1.0, -1.0] };
        let p = e.view() * Vec3::new(1.0, 1.6, 0.0).extend(1.0);
        assert!(p.truncate().length() < 1e-5);
    }
}
