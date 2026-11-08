//! VR controllers (AURORA_VR=1, `openxr` feature), VRChat-style: left stick
//! moves, right stick turns (snap or smooth), A jumps and a double tap flies,
//! while flying A rises and X descends, grip or moving stands up from a seat,
//! B/Y opens the menu (right stick chooses, trigger selects); otherwise
//! the trigger touches what the controller points at and the grip sits on it.
//! AURORA_VR_SIM=1 plays a scripted controller over a simulated headset.

use std::time::{Duration, Instant};

use glam::{Mat4, Vec2, Vec3};

use crate::agent::MoveInput;

const DEAD_ZONE: f32 = 0.25;
const RUN_AT: f32 = 0.9;
const SNAP_AT: f32 = 0.7;
const SNAP_RELEASE: f32 = 0.3;
const DOUBLE_TAP: Duration = Duration::from_millis(350);
/// From the agent's position (its centre) down to its feet, where the VR
/// stage's floor goes.
#[cfg_attr(not(feature = "openxr"), allow(dead_code))]
pub const AGENT_TO_FEET: f32 = 0.9;
/// Smooth turn at full deflection, radians per second.
const SMOOTH_RATE: f32 = 2.5;

/// The controllers of one headset frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct VrPad {
    pub move_stick: Vec2,
    pub turn_stick: Vec2,
    pub jump: bool,
    pub descend: bool,
    pub menu: bool,
    pub trigger: [bool; 2],
    pub grip: [bool; 2],
}

/// The entries of the VR menu, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItem {
    Fly,
    SitStand,
    AlwaysRun,
    TurnMode,
    Close,
}

pub const MENU: [MenuItem; 5] = [
    MenuItem::Fly,
    MenuItem::SitStand,
    MenuItem::AlwaysRun,
    MenuItem::TurnMode,
    MenuItem::Close,
];

/// What a frame of controller input asks beyond the movement keys.
#[derive(Debug, Default, PartialEq)]
pub struct VrActions {
    /// Avatar yaw change, radians (counter-clockwise).
    pub turn: f32,
    pub toggle_fly: bool,
    pub stand: bool,
    /// B/Y pressed: the menu opened or closed.
    pub menu: bool,
    /// The trigger chose this menu entry.
    pub pick: Option<MenuItem>,
    /// The trigger of this hand (0 left, 1 right) was pressed, menu closed.
    pub point: Option<usize>,
    /// The grip of this hand was pressed while standing.
    pub grab: Option<usize>,
}

#[derive(Debug)]
pub struct VrControls {
    pub pad: VrPad,
    last: VrPad,
    last_jump: Option<Instant>,
    turn_latched: bool,
    /// Snap turn step in degrees; 0 turns smoothly.
    pub snap_degrees: f32,
    /// The left stick is pushed fully: run.
    pub running: bool,
    pub menu_open: bool,
    pub menu_sel: usize,
    menu_latched: bool,
    /// Controller aim rays in the stage (origin, direction).
    pub aim: [Option<(Vec3, Vec3)>; 2],
    /// The stage in the world, for this frame.
    pub stage: Mat4,
    /// AURORA_VR_SIM=1: the scripted controller's start.
    pub sim: Option<Instant>,
    #[cfg(feature = "openxr")]
    frame: Option<aurora_render::aurora_xr::XrFrame>,
}

impl Default for VrControls {
    fn default() -> Self {
        VrControls {
            pad: VrPad::default(),
            last: VrPad::default(),
            last_jump: None,
            turn_latched: false,
            snap_degrees: 30.0,
            running: false,
            menu_open: false,
            menu_sel: 0,
            menu_latched: false,
            aim: [None; 2],
            stage: Mat4::IDENTITY,
            sim: sim_wanted().then(Instant::now),
            #[cfg(feature = "openxr")]
            frame: None,
        }
    }
}

impl VrControls {
    /// Adds this frame's controllers to the movement keys `i`.
    pub fn apply(&mut self, i: &mut MoveInput, flying: bool, seated: bool, now: Instant, dt: f32) -> VrActions {
        let pad = self.pad;
        let mut out = VrActions::default();
        let m = pad.move_stick;
        i.forward |= m.y > DEAD_ZONE;
        i.back |= m.y < -DEAD_ZONE;
        i.strafe_right |= m.x > DEAD_ZONE;
        i.strafe_left |= m.x < -DEAD_ZONE;
        self.running = m.length() > RUN_AT;
        let moving = m.length() > DEAD_ZONE;

        let jump_pressed = pad.jump && !self.last.jump;
        if jump_pressed {
            match self.last_jump {
                Some(at) if now.duration_since(at) <= DOUBLE_TAP => {
                    out.toggle_fly = true;
                    self.last_jump = None;
                }
                _ => self.last_jump = Some(now),
            }
        }
        i.up |= pad.jump && !out.toggle_fly;
        if flying {
            i.down |= pad.descend;
        }

        let x = if self.menu_open { 0.0 } else { pad.turn_stick.x };
        if self.menu_open {
            let y = pad.turn_stick.y;
            if y.abs() > SNAP_AT && !self.menu_latched {
                let n = MENU.len();
                self.menu_sel = (self.menu_sel + if y < 0.0 { 1 } else { n - 1 }) % n;
                self.menu_latched = true;
            } else if y.abs() < SNAP_RELEASE {
                self.menu_latched = false;
            }
        }
        if self.snap_degrees > 0.0 {
            if x.abs() > SNAP_AT && !self.turn_latched {
                out.turn = -x.signum() * self.snap_degrees.to_radians();
                self.turn_latched = true;
            } else if x.abs() < SNAP_RELEASE {
                self.turn_latched = false;
            }
        } else if x.abs() > DEAD_ZONE {
            out.turn = -x * SMOOTH_RATE * dt;
        }

        let grip = (0..2).find(|&h| pad.grip[h] && !self.last.grip[h]);
        out.stand = seated && (moving || jump_pressed || grip.is_some());
        if !seated {
            out.grab = grip;
        }
        out.menu = pad.menu && !self.last.menu;
        if out.menu {
            self.menu_open = !self.menu_open;
            self.menu_sel = 0;
        }
        if let Some(h) = (0..2).find(|&h| pad.trigger[h] && !self.last.trigger[h]) {
            if self.menu_open {
                let item = MENU[self.menu_sel.min(MENU.len() - 1)];
                self.menu_open &= item != MenuItem::Close;
                out.pick = Some(item);
            } else {
                out.point = Some(h);
            }
        }
        self.last = pad;
        out
    }
}

#[cfg(feature = "openxr")]
impl From<&aurora_render::aurora_xr::XrInput> for VrPad {
    fn from(x: &aurora_render::aurora_xr::XrInput) -> Self {
        VrPad {
            move_stick: x.move_stick,
            turn_stick: x.turn_stick,
            jump: x.jump,
            descend: x.descend,
            menu: x.menu,
            trigger: x.trigger,
            grip: x.grip,
        }
    }
}

/// AURORA_VR_SIM=1 (with the `openxr` feature): the scripted controller.
fn sim_wanted() -> bool {
    cfg!(feature = "openxr") && std::env::var("AURORA_VR_SIM").is_ok_and(|v| !matches!(v.trim(), "" | "0" | "off" | "false" | "no"))
}

impl VrControls {
    /// A controller's aim ray in the world (origin, direction).
    pub fn aim_world(&self, hand: usize) -> Option<(Vec3, Vec3)> {
        let (o, d) = self.aim[hand]?;
        Some((self.stage.transform_point3(o), self.stage.transform_vector3(d).try_normalize()?))
    }

    /// The headset's frame is under way: its eyes are to be drawn.
    #[cfg_attr(not(feature = "openxr"), allow(dead_code))]
    pub fn has_frame(&self) -> bool {
        #[cfg(feature = "openxr")]
        return self.frame.is_some();
        #[cfg(not(feature = "openxr"))]
        false
    }

    /// The head in the world: its place and where it looks.
    pub fn head(&self) -> Option<(Vec3, Vec3)> {
        #[cfg(feature = "openxr")]
        {
            let [l, r] = &self.frame.as_ref()?.eyes;
            let pos = self.stage.transform_point3((l.position + r.position) * 0.5);
            let fwd = self.stage.transform_vector3(l.orientation * Vec3::NEG_Z);
            Some((pos, fwd.try_normalize()?))
        }
        #[cfg(not(feature = "openxr"))]
        None
    }
}

#[cfg(feature = "openxr")]
impl VrControls {
    /// Waits for the headset's next frame, its stage at the avatar's place
    /// (`feet`, `yaw`), and reads its controllers.
    pub fn begin(&mut self, renderer: &mut aurora_render::Renderer, feet: Vec3, yaw: f32) {
        self.frame = renderer.xr_begin_frame();
        self.stage = aurora_render::aurora_xr::world_from_stage(feet, yaw);
        match &self.frame {
            Some(f) => {
                self.pad = VrPad::from(&f.input);
                self.aim = f.input.aim;
            }
            // a paused headset leaves no stick held
            None => self.pad = VrPad::default(),
        }
        if let Some(start) = self.sim {
            self.pad = sim_pad(start.elapsed().as_secs_f32());
        }
    }

    /// Draws both eyes of the frame begun, then hands it to the headset.
    pub fn draw_eyes(
        &mut self,
        renderer: &mut aurora_render::Renderer,
        fp: &aurora_render::FrameParams,
        lists: &aurora_render::DrawLists,
        ui: [Option<aurora_render::EguiFrame>; 2],
    ) {
        let Some(frame) = self.frame.take() else { return };
        for ((layer, eye), ui) in frame.eyes.iter().enumerate().zip(ui) {
            let mut p = fp.clone();
            p.view = eye.view_in(self.stage);
            p.proj = eye.projection(fp.near);
            p.camera_pos = eye.position_in(self.stage);
            p.fov_y = eye.fov[2] - eye.fov[3];
            renderer.render_xr_eye(p, lists, layer as u32, ui);
        }
        renderer.xr_end_frame();
    }
}

/// The scripted controller of AURORA_VR_SIM, `t` seconds in (a 30 s loop):
/// walk, snap turns, double tap to fly, rise, descend, land, the menu
/// (choose and select twice), touch what is ahead, sit on it, stand up.
#[cfg_attr(not(feature = "openxr"), allow(dead_code))]
pub fn sim_pad(t: f32) -> VrPad {
    let t = t % 30.0;
    let at = |a: f32, b: f32| (a..b).contains(&t);
    let tap = |a: f32| at(a, a + 0.12);
    let mut p = VrPad::default();
    if at(2.0, 5.0) {
        p.move_stick.y = 0.6;
    }
    if at(5.5, 5.8) || at(6.5, 6.8) {
        p.turn_stick.x = 1.0;
    }
    if at(7.5, 7.8) {
        p.turn_stick.x = -1.0;
    }
    p.jump = tap(9.0) || tap(9.25) || at(10.0, 12.0) || tap(14.0) || tap(14.25);
    p.descend = at(12.5, 13.5);
    p.menu = tap(16.0) || tap(22.0);
    if at(17.0, 17.2) || at(18.0, 18.2) || at(20.0, 20.2) {
        p.turn_stick.y = -1.0;
    }
    p.trigger[1] = tap(19.0) || tap(21.0) || tap(23.5);
    p.grip[1] = tap(25.0) || tap(27.0);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(c: &mut VrControls, pad: VrPad, flying: bool, seated: bool, at: Instant) -> (MoveInput, VrActions) {
        c.pad = pad;
        let mut i = MoveInput::default();
        let a = c.apply(&mut i, flying, seated, at, 1.0 / 90.0);
        (i, a)
    }

    #[test]
    fn left_stick_walks_and_runs() {
        let mut c = VrControls::default();
        let now = Instant::now();
        let (i, _) = step(
            &mut c,
            VrPad {
                move_stick: Vec2::new(0.0, 0.5),
                ..Default::default()
            },
            false,
            false,
            now,
        );
        assert!(i.forward && !i.back && !c.running);
        let (i, _) = step(
            &mut c,
            VrPad {
                move_stick: Vec2::new(-1.0, 0.0),
                ..Default::default()
            },
            false,
            false,
            now,
        );
        assert!(i.strafe_left && c.running);
        let (i, _) = step(
            &mut c,
            VrPad {
                move_stick: Vec2::new(0.1, -0.1),
                ..Default::default()
            },
            false,
            false,
            now,
        );
        assert!(!i.forward && !i.back && !i.strafe_left && !i.strafe_right);
    }

    #[test]
    fn snap_turn_once_per_push() {
        let mut c = VrControls::default();
        let now = Instant::now();
        let right = VrPad {
            turn_stick: Vec2::new(1.0, 0.0),
            ..Default::default()
        };
        let (_, a) = step(&mut c, right, false, false, now);
        assert!((a.turn + 30f32.to_radians()).abs() < 1e-6);
        let (_, a) = step(&mut c, right, false, false, now);
        assert_eq!(a.turn, 0.0);
        step(&mut c, VrPad::default(), false, false, now);
        let (_, a) = step(
            &mut c,
            VrPad {
                turn_stick: Vec2::new(-1.0, 0.0),
                ..Default::default()
            },
            false,
            false,
            now,
        );
        assert!(a.turn > 0.0);
    }

    #[test]
    fn smooth_turn_scales_with_time() {
        let mut c = VrControls {
            snap_degrees: 0.0,
            ..Default::default()
        };
        let (_, a) = step(
            &mut c,
            VrPad {
                turn_stick: Vec2::new(1.0, 0.0),
                ..Default::default()
            },
            false,
            false,
            Instant::now(),
        );
        assert!((a.turn + SMOOTH_RATE / 90.0).abs() < 1e-6);
    }

    #[test]
    fn double_tap_a_toggles_flying() {
        let mut c = VrControls::default();
        let t = Instant::now();
        let a_down = VrPad {
            jump: true,
            ..Default::default()
        };
        let (i, a) = step(&mut c, a_down, false, false, t);
        assert!(i.up && !a.toggle_fly);
        step(&mut c, VrPad::default(), false, false, t + Duration::from_millis(100));
        let (_, a) = step(&mut c, a_down, false, false, t + Duration::from_millis(200));
        assert!(a.toggle_fly);
        // a slow second tap is just another jump
        step(&mut c, VrPad::default(), false, false, t + Duration::from_millis(300));
        step(&mut c, a_down, false, false, t + Duration::from_millis(400));
        step(&mut c, VrPad::default(), false, false, t + Duration::from_millis(500));
        let (_, a) = step(&mut c, a_down, false, false, t + Duration::from_millis(1200));
        assert!(!a.toggle_fly);
    }

    #[test]
    fn flying_a_rises_x_descends() {
        let mut c = VrControls::default();
        let now = Instant::now();
        let (i, _) = step(
            &mut c,
            VrPad {
                descend: true,
                ..Default::default()
            },
            true,
            false,
            now,
        );
        assert!(i.down && !i.up);
        let (i, _) = step(
            &mut c,
            VrPad {
                descend: true,
                ..Default::default()
            },
            false,
            false,
            now,
        );
        assert!(!i.down);
    }

    #[test]
    fn seated_grip_or_move_stands() {
        let mut c = VrControls::default();
        let now = Instant::now();
        let (_, a) = step(
            &mut c,
            VrPad {
                grip: [false, true],
                ..Default::default()
            },
            false,
            true,
            now,
        );
        assert!(a.stand);
        let (_, a) = step(
            &mut c,
            VrPad {
                grip: [false, true],
                ..Default::default()
            },
            false,
            true,
            now,
        );
        assert!(!a.stand);
        let (_, a) = step(
            &mut c,
            VrPad {
                move_stick: Vec2::new(0.0, 1.0),
                ..Default::default()
            },
            false,
            true,
            now,
        );
        assert!(a.stand);
        let (_, a) = step(
            &mut c,
            VrPad {
                move_stick: Vec2::new(0.0, 1.0),
                ..Default::default()
            },
            false,
            false,
            now,
        );
        assert!(!a.stand);
    }

    #[test]
    fn menu_chooses_with_the_stick_and_selects_with_the_trigger() {
        let mut c = VrControls::default();
        let now = Instant::now();
        let press = |menu, y: f32, trigger| VrPad {
            menu,
            turn_stick: Vec2::new(0.0, y),
            trigger: [false, trigger],
            ..Default::default()
        };
        step(&mut c, press(true, 0.0, false), false, false, now);
        assert!(c.menu_open);
        step(&mut c, press(false, -1.0, false), false, false, now);
        step(&mut c, press(false, -1.0, false), false, false, now);
        step(&mut c, press(false, 0.0, false), false, false, now);
        step(&mut c, press(false, -1.0, false), false, false, now);
        let (_, a) = step(&mut c, press(false, 0.0, true), false, false, now);
        assert_eq!(a.pick, Some(MenuItem::AlwaysRun));
        assert_eq!(a.turn, 0.0);
        step(&mut c, press(true, 0.0, false), false, false, now);
        assert!(!c.menu_open);
        let (_, a) = step(&mut c, press(false, 0.0, true), false, false, now);
        assert_eq!((a.pick, a.point), (None, Some(1)));
    }

    #[test]
    fn grip_sits_while_standing() {
        let mut c = VrControls::default();
        let grip = VrPad {
            grip: [true, false],
            ..Default::default()
        };
        let (_, a) = step(&mut c, grip, false, false, Instant::now());
        assert_eq!((a.grab, a.stand), (Some(0), false));
    }

    #[test]
    fn menu_opens_on_press_only() {
        let mut c = VrControls::default();
        let now = Instant::now();
        let (_, a) = step(
            &mut c,
            VrPad {
                menu: true,
                ..Default::default()
            },
            false,
            false,
            now,
        );
        assert!(a.menu);
        let (_, a) = step(
            &mut c,
            VrPad {
                menu: true,
                ..Default::default()
            },
            false,
            false,
            now,
        );
        assert!(!a.menu);
    }
}
