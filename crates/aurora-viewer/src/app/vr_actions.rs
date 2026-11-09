//! The VR controllers' actions: the B/Y menu, and touching or sitting on
//! what a controller points at.
use super::*;
use crate::ui::widgets::Floater;
use crate::vr::{MENU, MenuItem};

impl App {
    pub(super) fn vr_menu_pick(&mut self, item: MenuItem) {
        match item {
            MenuItem::Fly => self.toggle_fly(),
            MenuItem::SitStand => self.sit_stand_shortcut(),
            MenuItem::AlwaysRun => self.set_always_run(!self.always_run),
            MenuItem::TurnMode => self.vr.snap_degrees = if self.vr.snap_degrees > 0.0 { 0.0 } else { 30.0 },
            MenuItem::Close => {}
        }
    }

    /// The trigger touches (or the grip, `sit`, sits on) the object the
    /// controller of `hand` points at, as a left click would.
    pub(super) fn vr_point(&mut self, hand: usize, sit: bool) {
        let Some(ray) = self.vr.aim_world(hand) else { return };
        let (point, idx) = self
            .scene
            .selection_pick(&self.world, Some(ray), None, false, false, self.settings.draw_distance);
        let (Some(point), Some(idx)) = (point, idx) else { return };
        if sit {
            // any object is a seat, clickable or not
            if let Some(object) = self.world.objects.get(idx)
                && let Some((pos, rot, _)) = Scene::object_transform(&self.world, idx, Instant::now(), 0)
            {
                let (handle, target) = (object.key.region, object.full_id);
                self.send(NetCommand::RequestSit {
                    handle,
                    target,
                    offset: rot.inverse() * (point - pos),
                });
            }
        } else if let Some(target) = crate::interaction::target(&self.world, idx, &self.interactions.props) {
            self.activate_object_action(target, idx, point, Some(ray));
        }
    }

    /// The VR menu (B/Y), and the simulated controller's readout.
    pub(super) fn vr_menu_ui(&mut self, ctx: &egui::Context) {
        self.vr.panels.clear();
        let mut panels = Vec::new();
        if self.vr.sim.is_some() {
            let p = self.vr.pad;
            let b = |on: bool| if on { "●" } else { "○" };
            let mut open = true;
            Floater::new(
                "vr-sim",
                aurora_i18n::tr!("vr-sim-title"),
                egui::pos2(12.0, 560.0),
                egui::vec2(300.0, 70.0),
            )
            .show(ctx, &self.palette.clone(), &mut open, |ui| {
                panels.push(ui.max_rect());
                ui.monospace(format!(
                    "L {:+.1} {:+.1}   R {:+.1} {:+.1}",
                    p.move_stick.x, p.move_stick.y, p.turn_stick.x, p.turn_stick.y
                ));
                ui.monospace(format!(
                    "A {}  X {}  B {}  T {}  G {}",
                    b(p.jump),
                    b(p.descend),
                    b(p.menu),
                    b(p.trigger[1]),
                    b(p.grip[1])
                ));
            });
        }
        if !self.vr.menu_open {
            self.vr.panels = panels;
            return;
        }
        let mut picked = None;
        let mut open = true;
        let palette = self.palette.clone();
        Floater::new(
            "vr-menu",
            aurora_i18n::tr!("vr-menu-title"),
            egui::pos2(480.0, 240.0),
            egui::vec2(320.0, 260.0),
        )
        .show(ctx, &palette, &mut open, |ui| {
            panels.push(ui.max_rect());
            for (i, &item) in MENU.iter().enumerate() {
                let label = match item {
                    MenuItem::Fly if self.world.agent.flying => aurora_i18n::tr!("vr-menu-land").to_string(),
                    MenuItem::Fly => aurora_i18n::tr!("vr-menu-fly").to_string(),
                    MenuItem::SitStand if self.world.agent.is_sitting() => aurora_i18n::tr!("vr-menu-stand").to_string(),
                    MenuItem::SitStand => aurora_i18n::tr!("vr-menu-sit").to_string(),
                    MenuItem::AlwaysRun => {
                        format!(
                            "{} {}",
                            if self.always_run { "☑" } else { "☐" },
                            aurora_i18n::tr!("vr-menu-always-run")
                        )
                    }
                    MenuItem::TurnMode if self.vr.snap_degrees > 0.0 => aurora_i18n::tr!("vr-menu-snap-turn").to_string(),
                    MenuItem::TurnMode => aurora_i18n::tr!("vr-menu-smooth-turn").to_string(),
                    MenuItem::Close => aurora_i18n::tr!("vr-menu-close").to_string(),
                };
                if ui.selectable_label(i == self.vr.menu_sel, label).clicked() {
                    picked = Some((i, item));
                }
            }
            ui.small(aurora_i18n::tr!("vr-menu-hint"));
        });
        self.vr.panels = panels;
        if !open {
            self.vr.menu_open = false;
        }
        if let Some((i, item)) = picked {
            self.vr.menu_sel = i;
            self.vr.menu_open &= item != MenuItem::Close;
            self.vr_menu_pick(item);
        }
    }

    /// What is drawn over an eye of the headset: the shapes of the VR
    /// panels (the desktop's other windows stay on the window) and a beam
    /// along each controller's aim.
    #[cfg(feature = "openxr")]
    pub(super) fn vr_eye_shapes(
        &self,
        shapes: &[egui::epaint::ClippedShape],
        screen: egui::Rect,
        eye: usize,
    ) -> Vec<egui::epaint::ClippedShape> {
        // a panel's title bar and shadow lie around its body
        let panels: Vec<egui::Rect> = self
            .vr
            .panels
            .iter()
            .map(|r| egui::Rect::from_min_max(r.min - egui::vec2(24.0, 56.0), r.max + egui::vec2(24.0, 24.0)))
            .collect();
        let mut out: Vec<_> = shapes
            .iter()
            .filter(|s| {
                let r = s.shape.visual_bounding_rect().intersect(s.clip_rect);
                panels.iter().any(|p| p.contains_rect(r))
            })
            .cloned()
            .collect();
        let Some(vp) = self.vr.eye_view_proj(eye) else { return out };
        let size = glam::Vec2::new(screen.width(), screen.height());
        for hand in 0..2 {
            let Some(aim) = self.vr.aim_world(hand) else { continue };
            let Some([a, b]) = crate::vr::beam_on_screen(vp, aim, crate::vr::BEAM_LENGTH, size) else {
                continue;
            };
            let (a, b) = (screen.min + egui::vec2(a.x, a.y), screen.min + egui::vec2(b.x, b.y));
            let colour = self.palette.teal;
            for shape in [
                egui::Shape::line_segment([a, b], egui::Stroke::new(3.0, colour)),
                egui::Shape::circle_filled(b, 4.0, colour),
            ] {
                out.push(egui::epaint::ClippedShape {
                    clip_rect: screen,
                    shape,
                    transform_after_tessellation: egui::emath::TSTransform::IDENTITY,
                });
            }
        }
        out
    }
}
