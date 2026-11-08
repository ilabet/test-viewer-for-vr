//! VR controllers (AURORA_VR=1, `openxr` feature), VRChat-style: left stick
//! moves, right stick turns (snap or smooth), A jumps and a double tap flies,
//! while flying A rises and X descends, grip or moving stands up from a seat,
//! B/Y opens the menu.

use std::time::{Duration, Instant};

use glam::Vec2;

use crate::agent::MoveInput;

const DEAD_ZONE: f32 = 0.25;
const RUN_AT: f32 = 0.9;
const SNAP_AT: f32 = 0.7;
const SNAP_RELEASE: f32 = 0.3;
const DOUBLE_TAP: Duration = Duration::from_millis(350);
/// From the agent's position (its centre) down to its feet, where the VR
/// stage's floor goes.
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

/// What a frame of controller input asks beyond the movement keys.
#[derive(Debug, Default, PartialEq)]
pub struct VrActions {
    /// Avatar yaw change, radians (counter-clockwise).
    pub turn: f32,
    pub toggle_fly: bool,
    pub stand: bool,
    pub menu: bool,
}

#[derive(Debug)]
pub struct VrControls {
    pub pad: VrPad,
    last: VrPad,
    last_jump: Option<Instant>,
    turn_latched: bool,
    /// Snap turn step in degrees; 0 turns smoothly.
    pub snap_degrees: f32,
}

impl Default for VrControls {
    fn default() -> Self {
        VrControls {
            pad: VrPad::default(),
            last: VrPad::default(),
            last_jump: None,
            turn_latched: false,
            snap_degrees: 30.0,
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
        i.run |= m.length() > RUN_AT;
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

        let x = pad.turn_stick.x;
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

        let grip_pressed = (0..2).any(|h| pad.grip[h] && !self.last.grip[h]);
        out.stand = seated && (moving || jump_pressed || grip_pressed);
        out.menu = pad.menu && !self.last.menu;
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

/// Draws the headset's two eyes from the avatar's place (`feet`, `yaw`) and
/// returns its controllers, or `None` when the headset wants no frame.
#[cfg(feature = "openxr")]
pub fn draw_eyes(
    renderer: &mut aurora_render::Renderer,
    fp: &aurora_render::FrameParams,
    lists: &aurora_render::DrawLists,
    feet: glam::Vec3,
    yaw: f32,
) -> Option<VrPad> {
    let frame = renderer.xr_begin_frame()?;
    let stage = aurora_render::aurora_xr::world_from_stage(feet, yaw);
    for (layer, eye) in frame.eyes.iter().enumerate() {
        let mut p = fp.clone();
        p.view = eye.view_in(stage);
        p.proj = eye.projection(fp.near);
        p.camera_pos = eye.position_in(stage);
        p.fov_y = eye.fov[2] - eye.fov[3];
        renderer.render_xr_eye(p, lists, layer as u32);
    }
    renderer.xr_end_frame();
    Some(VrPad::from(&frame.input))
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
        assert!(i.forward && !i.back && !i.run);
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
        assert!(i.strafe_left && i.run);
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
