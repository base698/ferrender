//! Menus, the toolbar, the browser tree, the timeline and the floating dialogs.

use egui::{Align, Align2, Color32, Context, FontId, Layout, RichText, Sense, Stroke, StrokeKind, Ui, Vec2, vec2};
use egui_phosphor::regular as icon;
use fr_core::doc::{HoleFit, HoleShape};
use fr_core::{Axis, CKind, FeatureKind, Id, Kind, Op, Plane, Unit, threads};
use serde_json::json;

use crate::ai::Who;
use crate::app::{ACCENT, Action, App, Dialog, Mode, Tool};
use crate::view;

const DIM: Color32 = Color32::from_rgb(110, 116, 128);
const BAD: Color32 = Color32::from_rgb(196, 40, 40);
const GOOD: Color32 = Color32::from_rgb(34, 150, 70);

fn cmd(key: &str) -> String {
    if cfg!(target_os = "macos") { format!("\u{2318}{key}") } else { format!("Ctrl+{key}") }
}

fn item(app: &mut App, ui: &mut Ui, label: &str, shortcut: &str, a: Action) {
    let mut b = egui::Button::new(label);
    if !shortcut.is_empty() {
        b = b.shortcut_text(shortcut);
    }
    if ui.add(b).clicked() {
        let ctx = ui.ctx().clone();
        app.run(&ctx, a);
        ui.close();
    }
}

pub fn menu_bar(app: &mut App, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.menu_button("File", |ui| {
            item(app, ui, "New", &cmd("N"), Action::New);
            item(app, ui, "Open\u{2026}", &cmd("O"), Action::Open);
            ui.separator();
            item(app, ui, "Save", &cmd("S"), Action::Save);
            item(app, ui, "Save As\u{2026}", &cmd("\u{21e7}S"), Action::SaveAs);
            item(app, ui, "Recover Unsaved\u{2026}", "", Action::Recover);
            ui.separator();
            item(app, ui, "Import STL\u{2026}", &cmd("I"), Action::Import);
            item(app, ui, "Export STL\u{2026}", &cmd("E"), Action::Export);
            item(app, ui, "Export STEP\u{2026}", "", Action::ExportStep);
        });
        ui.menu_button("Edit", |ui| {
            item(app, ui, "Undo", &cmd("Z"), Action::Undo);
            item(app, ui, "Redo", &cmd("\u{21e7}Z"), Action::Redo);
            ui.separator();
            item(app, ui, "Cut", &cmd("X"), Action::Cut);
            item(app, ui, "Copy", &cmd("C"), Action::Copy);
            item(app, ui, "Paste", &cmd("V"), Action::Paste);
            item(app, ui, "Delete", "Del", Action::Delete);
            item(app, ui, "Select All", &cmd("A"), Action::SelectAll);
            ui.separator();
            item(app, ui, "Parameters\u{2026}", "", Action::Parameters);
        });
        ui.menu_button("Sketch", |ui| {
            item(app, ui, "New Sketch", "", Action::NewSketch);
            item(app, ui, "Finish Sketch", "", Action::FinishSketch);
            ui.separator();
            for (label, key, tool) in [("Select", "S", Tool::Select), ("Line", "L", Tool::Line), ("Rectangle", "R", Tool::Rect), ("Circle", "C", Tool::Circle), ("Arc", "A", Tool::Arc), ("Point", "P", Tool::Point), ("Dimension", "D", Tool::Dimension)] {
                item(app, ui, label, key, Action::Tool(tool));
            }
            item(app, ui, "Toggle Construction", "X", Action::Construction);
            ui.separator();
            item(app, ui, "Polygon", "", Action::Tool(Tool::Polygon));
            item(app, ui, "Project Face", "", Action::Tool(Tool::Project));
            item(app, ui, "Trim", "T", Action::Tool(Tool::Trim));
            item(app, ui, "Offset", "O", Action::Offset);
            item(app, ui, "Mirror", "", Action::MirrorSketch);
            item(app, ui, "Fillet Corner", "", Action::Fillet);
            item(app, ui, "Chamfer Corner", "", Action::Chamfer);
        });
        ui.menu_button("Model", |ui| {
            item(app, ui, "Extrude", "E", Action::Extrude);
            item(app, ui, "Revolve", "", Action::Revolve);
            item(app, ui, "Text / Emboss", "", Action::Text);
            ui.separator();
            item(app, ui, "Move / Rotate / Scale", "", Action::Transform);
            item(app, ui, "Combine", "", Action::Combine);
            item(app, ui, "Pattern / Mirror", "", Action::Pattern);
            ui.separator();
            item(app, ui, "Fillet Edges", "", Action::Blend(false));
            item(app, ui, "Chamfer Edges", "", Action::Blend(true));
            item(app, ui, "Shell", "", Action::Shell);
            ui.separator();
            item(app, ui, "Hole", "", Action::Hole);
            item(app, ui, "Thread", "", Action::Thread);
        });
        ui.menu_button("View", |ui| {
            for (label, view) in [("Home", "iso"), ("Top", "top"), ("Front", "front"), ("Right", "right"), ("Back", "back"), ("Left", "left"), ("Bottom", "bottom")] {
                item(app, ui, label, "", Action::View(view));
            }
            item(app, ui, "Fit", "F", Action::Fit);
            item(app, ui, "Section Analysis\u{2026}", "", Action::Section);
            item(app, ui, "Measure", "I", Action::Measure);
            ui.separator();
            ui.checkbox(&mut app.opts.grid, "Grid");
            ui.checkbox(&mut app.opts.constraints, "Constraints");
            ui.checkbox(&mut app.opts.dimensions, "Dimensions");
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add(egui::Button::new(format!("{} Assistant", icon::SPARKLE)).selected(app.ai.open)).clicked() {
                let ctx = ui.ctx().clone();
                app.run(&ctx, Action::Assistant);
            }
            ui.label(RichText::new(format!("{}{}", app.doc_name(), if app.session.dirty { "*" } else { "" })).color(DIM));
        });
    });
}

/// A toolbar button: an icon over a caption.
fn big(ui: &mut Ui, glyph: &str, label: &str, selected: bool, tip: &str) -> egui::Response {
    let font = FontId::proportional(11.0);
    let w = ui.painter().layout_no_wrap(label.to_owned(), font.clone(), DIM).size().x.max(34.0) + 12.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 46.0), Sense::click());
    let fill = if selected { Color32::from_rgb(205, 226, 250) } else if resp.hovered() { Color32::from_rgb(226, 229, 235) } else { Color32::TRANSPARENT };
    ui.painter().rect(rect, 4.0, fill, if selected { Stroke::new(1.0, ACCENT) } else { Stroke::NONE }, StrokeKind::Inside);
    let ink = if ui.is_enabled() { Color32::from_rgb(40, 44, 54) } else { Color32::from_rgb(170, 174, 182) };
    ui.painter().text(rect.center_top() + vec2(0.0, 15.0), Align2::CENTER_CENTER, glyph, FontId::proportional(22.0), ink);
    ui.painter().text(rect.center_bottom() - vec2(0.0, 8.0), Align2::CENTER_CENTER, label, font, ink);
    resp.on_hover_text(tip)
}

fn group(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    ui.vertical(|ui| {
        ui.horizontal(|ui| add(ui));
        let (r, _) = ui.allocate_exact_size(vec2(ui.min_rect().width(), 11.0), Sense::hover());
        ui.painter().text(r.center(), Align2::CENTER_CENTER, title, FontId::proportional(9.5), DIM);
    });
    ui.separator();
}

fn constraint_button(app: &mut App, ui: &mut Ui, kind: CKind) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 4.0, Color32::from_rgb(226, 229, 235));
    }
    view::constraint_icon(ui.painter(), rect.shrink(3.0), kind, Color32::from_rgb(40, 44, 54));
    let name = kind.name();
    if resp.on_hover_text(format!("{}{}: select {}, then click.", name[..1].to_uppercase(), &name[1..], kind.needs())).clicked() {
        let ctx = ui.ctx().clone();
        app.run(&ctx, Action::Constrain(kind));
    }
}

pub fn toolbar(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    egui::ScrollArea::horizontal().show(ui, |ui| toolbar_buttons(app, ui, &ctx));
}

fn toolbar_buttons(app: &mut App, ui: &mut Ui, ctx: &Context) {
    let ctx = ctx.clone();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if app.sketch().is_some() {
            group(ui, "CREATE", |ui| {
                for (glyph, label, tool, tip) in [
                    (icon::LINE_SEGMENT, "Line", Tool::Line, "Line (L)"),
                    (icon::RECTANGLE, "Rectangle", Tool::Rect, "Two-point rectangle (R)"),
                    (icon::CIRCLE, "Circle", Tool::Circle, "Centre and radius circle (C)"),
                    (icon::CIRCLE_NOTCH, "Arc", Tool::Arc, "Centre, start and end arc (A)"),
                    (icon::DOT_OUTLINE, "Point", Tool::Point, "Point (P)"),
                    (icon::POLYGON, "Polygon", Tool::Polygon, "Regular polygon; set the sides in the Sketch Palette"),
                    (icon::STACK_SIMPLE, "Project", Tool::Project, "Copy the outline of a body's face into the sketch"),
                ] {
                    if big(ui, glyph, label, app.tool == tool, tip).clicked() {
                        app.run(&ctx, Action::Tool(tool));
                    }
                }
            });
            group(ui, "MODIFY", |ui| {
                if big(ui, icon::ARROW_BEND_UP_RIGHT, "Fillet", false, "Round the selected corner").clicked() {
                    app.run(&ctx, Action::Fillet);
                }
                if big(ui, icon::ANGLE, "Chamfer", false, "Cut the selected corner straight").clicked() {
                    app.run(&ctx, Action::Chamfer);
                }
                if big(ui, icon::SCISSORS, "Trim", app.tool == Tool::Trim, "Cut geometry back to where it crosses other geometry (T)").clicked() {
                    app.run(&ctx, Action::Tool(Tool::Trim));
                }
                if big(ui, icon::ARROWS_OUT_LINE_HORIZONTAL, "Offset", false, "Parallel copy of the selected lines or circles (O)").clicked() {
                    app.run(&ctx, Action::Offset);
                }
                if big(ui, icon::SPLIT_HORIZONTAL, "Mirror", false, "Mirror the selection across the line selected last").clicked() {
                    app.run(&ctx, Action::MirrorSketch);
                }
                if big(ui, icon::LINE_SEGMENTS, "Construction", app.opts.construction, "Make the selection construction geometry, or draw new geometry as construction (X)").clicked() {
                    app.run(&ctx, Action::Construction);
                }
                if big(ui, icon::FUNCTION, "Parameters", app.show_params, "Named values you can use in any size box as $name").clicked() {
                    app.run(&ctx, Action::Parameters);
                }
            });
            group(ui, "CONSTRAINTS", |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
                    let kinds = [CKind::Coincident, CKind::Collinear, CKind::Concentric, CKind::Midpoint, CKind::Fix, CKind::Equal, CKind::Parallel, CKind::Perpendicular, CKind::Horizontal, CKind::Vertical, CKind::Tangent, CKind::Symmetric];
                    for row in kinds.chunks(6) {
                        ui.horizontal(|ui| {
                            for k in row {
                                constraint_button(app, ui, *k);
                            }
                        });
                    }
                });
            });
            group(ui, "INSPECT", |ui| {
                if big(ui, icon::RULER, "Dimension", app.tool == Tool::Dimension, "Dimension (D)").clicked() {
                    app.run(&ctx, Action::Tool(Tool::Dimension));
                }
            });
            group(ui, "SELECT", |ui| {
                if big(ui, icon::CURSOR, "Select", app.tool == Tool::Select, "Select and drag (S)").clicked() {
                    app.run(&ctx, Action::Tool(Tool::Select));
                }
            });
            let (rect, resp) = ui.allocate_exact_size(vec2(84.0, 58.0), Sense::click());
            ui.painter().text(rect.center_top() + vec2(0.0, 20.0), Align2::CENTER_CENTER, icon::CHECK_CIRCLE, FontId::proportional(32.0), GOOD);
            ui.painter().text(rect.center_bottom() - vec2(0.0, 9.0), Align2::CENTER_CENTER, "FINISH SKETCH", FontId::proportional(10.0), Color32::from_rgb(40, 44, 54));
            if resp.on_hover_text("Leave the sketch and return to the model").clicked() {
                app.run(&ctx, Action::FinishSketch);
            }
        } else {
            group(ui, "SKETCH", |ui| {
                if big(ui, icon::PENCIL_RULER, "New Sketch", app.dialog == Dialog::PickPlane, "Start a sketch on a plane or a flat face").clicked() {
                    app.run(&ctx, Action::NewSketch);
                }
            });
            group(ui, "CREATE", |ui| {
                let (ex, rev) = match &app.dialog {
                    Dialog::Feature(f) => (!f.revolve, f.revolve),
                    _ => (false, false),
                };
                if big(ui, icon::ARROW_FAT_LINES_UP, "Extrude", ex, "Pull a sketch profile into a solid (E)").clicked() {
                    app.run(&ctx, Action::Extrude);
                }
                if big(ui, icon::ARROWS_CLOCKWISE, "Revolve", rev, "Turn a sketch profile around an axis, like a lathe").clicked() {
                    app.run(&ctx, Action::Revolve);
                }
                if big(ui, icon::TEXT_T, "Text", matches!(app.dialog, Dialog::Text(_)), "Create solid text, or raise or engrave it on a flat face").clicked() {
                    app.run(&ctx, Action::Text);
                }
            });
            group(ui, "MODIFY", |ui| {
                if big(ui, icon::ARROWS_OUT_CARDINAL, "Move", matches!(app.dialog, Dialog::Transform(_)), "Move, rotate or scale the selected body").clicked() {
                    app.run(&ctx, Action::Transform);
                }
                if big(ui, icon::UNITE, "Combine", matches!(app.dialog, Dialog::Combine(_)), "Join, cut or intersect bodies").clicked() {
                    app.run(&ctx, Action::Combine);
                }
                if big(ui, icon::ARROW_BEND_UP_RIGHT, "Fillet", matches!(&app.dialog, Dialog::Blend(b) if !b.chamfer), "Round the edges of a body").clicked() {
                    app.run(&ctx, Action::Blend(false));
                }
                if big(ui, icon::ANGLE, "Chamfer", matches!(&app.dialog, Dialog::Blend(b) if b.chamfer), "Bevel the edges of a body").clicked() {
                    app.run(&ctx, Action::Blend(true));
                }
                if big(ui, icon::CUBE_FOCUS, "Shell", matches!(app.dialog, Dialog::Shell(_)), "Hollow a body to a wall thickness, open at the faces you choose").clicked() {
                    app.run(&ctx, Action::Shell);
                }
                if big(ui, icon::CIRCLE_DASHED, "Hole", matches!(app.dialog, Dialog::Hole(_)), "Drill holes sized for a screw: clearance, tapped, counterbored or countersunk").clicked() {
                    app.run(&ctx, Action::Hole);
                }
                if big(ui, icon::SPIRAL, "Thread", matches!(app.dialog, Dialog::Thread(_)), "Put a screw thread on a rod or in a hole").clicked() {
                    app.run(&ctx, Action::Thread);
                }
                if big(ui, icon::CIRCLES_THREE, "Pattern", matches!(app.dialog, Dialog::Pattern(_)), "Repeat a feature in a circle or a row, or mirror it").clicked() {
                    app.run(&ctx, Action::Pattern);
                }
                if big(ui, icon::FUNCTION, "Parameters", app.show_params, "Named values you can use in any size box as $name").clicked() {
                    app.run(&ctx, Action::Parameters);
                }
            });
            group(ui, "INSPECT", |ui| {
                if big(ui, icon::RULER, "Measure", matches!(app.dialog, Dialog::Measure(_)), "The distance and angle between two points, edges or faces (I)").clicked() {
                    app.run(&ctx, Action::Measure);
                }
                if big(ui, icon::CUBE_TRANSPARENT, "Section", app.section.on, "Cut the view open to look inside").clicked() {
                    app.run(&ctx, Action::Section);
                }
            });
            group(ui, "MESH", |ui| {
                if big(ui, icon::DOWNLOAD_SIMPLE, "Import STL", false, "Bring a mesh in as a body").clicked() {
                    app.run(&ctx, Action::Import);
                }
                if big(ui, icon::EXPORT, "Export STL", false, "Write the visible bodies for 3D printing").clicked() {
                    app.run(&ctx, Action::Export);
                }
            });
        }
    });
}

pub fn status(app: &mut App, ui: &mut Ui) {
    ui.horizontal(|ui| {
        match app.sketch() {
            Some((id, _)) => {
                let name = app.doc().feature(id).map_or(String::new(), |f| f.name.clone());
                ui.label(RichText::new(format!("Editing {name}")).strong());
                if !app.report.ok {
                    ui.colored_label(BAD, "Constraints conflict");
                } else if app.report.dof == 0 {
                    ui.colored_label(GOOD, "Fully constrained");
                } else {
                    ui.label(RichText::new(format!("{} degree{} of freedom", app.report.dof, if app.report.dof == 1 { "" } else { "s" })).color(ACCENT));
                }
            }
            None => {
                let n = app.session.built.bodies.len();
                ui.label(format!("{n} bod{}", if n == 1 { "y" } else { "ies" }));
                if let Some(id) = app.sel_body
                    && let Some(b) = app.session.built.body(id)
                {
                    let u = app.doc().units;
                    let size = b.mesh.bbox().map_or(glam::DVec3::ZERO, |(lo, hi)| hi - lo) / u.mm();
                    let n = |v: f64| fr_core::units::trim_num(v, 3);
                    ui.label(RichText::new(format!("{} ({}): {} \u{d7} {} \u{d7} {} {}, {} {}\u{b3}, {} triangles", b.name, if b.is_exact() { "exact" } else { "mesh" }, n(size.x), n(size.y), n(size.z), u.name(), n(b.mesh.volume() / u.mm().powi(3)), u.name(), b.mesh.tris.len())).color(DIM));
                }
                if let Some(f) = &app.sel_face
                    && let Some(b) = app.session.built.body(f.body)
                {
                    let u = app.doc().units;
                    ui.label(RichText::new(format!("{} face of {}, {} {}\u{b2}", if f.plane.is_some() { "Flat" } else { "Curved" }, b.name, fr_core::units::trim_num(f.area / u.mm().powi(2), 3), u.name())).color(DIM));
                }
                let errors = app.session.built.errors.len();
                if errors > 0 {
                    ui.colored_label(BAD, format!("{errors} feature{} failed; hover it in the timeline", if errors == 1 { "" } else { "s" }));
                }
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let mut unit = app.doc().units;
            egui::ComboBox::from_id_salt("units").width(52.0).selected_text(unit.name()).show_ui(ui, |ui| {
                for u in Unit::ALL {
                    ui.selectable_value(&mut unit, u, u.name());
                }
            });
            if unit != app.doc().units {
                let _ = app.execute(&json!({"op": "set_units", "units": unit.name()}));
            }
            ui.label(RichText::new("Units").color(DIM));
            if let Some(b) = &app.bridge {
                ui.label(RichText::new(format!("MCP channel {}", b.port)).color(DIM)).on_hover_text("Private local connection for AI clients running as your user. Connect with `ferrender mcp`.");
                ui.separator();
            }
        });
    });
}

fn feature_icon(kind: &FeatureKind) -> &'static str {
    match kind {
        FeatureKind::Sketch(_) => icon::PENCIL_RULER,
        FeatureKind::Extrude(_) => icon::ARROW_FAT_LINES_UP,
        FeatureKind::Revolve(_) => icon::ARROWS_CLOCKWISE,
        FeatureKind::Import(_) => icon::DOWNLOAD_SIMPLE,
        FeatureKind::Transform(_) => icon::ARROWS_OUT_CARDINAL,
        FeatureKind::Combine(_) => icon::UNITE,
        FeatureKind::Pattern(_) => icon::CIRCLES_THREE,
        FeatureKind::Blend(b) if b.chamfer => icon::ANGLE,
        FeatureKind::Blend(_) => icon::ARROW_BEND_UP_RIGHT,
        FeatureKind::Shell(_) => icon::CUBE_TRANSPARENT,
        FeatureKind::Hole(_) => icon::CIRCLE_DASHED,
        FeatureKind::Thread(_) => icon::SPIRAL,
        FeatureKind::Text(_) => icon::TEXT_T,
    }
}

/// Edit, suppress and delete, shared by the timeline and the browser.
fn feature_menu(app: &mut App, ui: &mut Ui, id: Id, suppressed: bool) {
    if ui.button("Edit").clicked() {
        app.edit_feature(id);
        ui.close();
    }
    if ui.button("Rename").clicked() {
        let name = app.doc().feature(id).map_or(String::new(), |f| f.name.clone());
        app.rename = Some((id, name));
        ui.close();
    }
    if ui.button(if suppressed { "Unsuppress" } else { "Suppress" }).clicked() {
        let _ = app.execute(&json!({"op": "edit_feature", "feature": id, "suppressed": !suppressed}));
        ui.close();
    }
    if ui.button("Delete").clicked() {
        app.delete_feature(id);
        ui.close();
    }
}

/// The roll-back marker: drag it along the timeline to see the model as it was.
fn marker(app: &mut App, ui: &mut Ui) {
    let (rect, resp) = ui.allocate_exact_size(vec2(12.0, 24.0), Sense::drag());
    let hot = resp.hovered() || resp.dragged();
    let color = if hot { ACCENT } else { Color32::from_rgb(70, 74, 84) };
    ui.painter().line_segment([rect.center_top(), rect.center_bottom()], Stroke::new(if hot { 3.0 } else { 2.0 }, color));
    ui.painter().add(egui::Shape::convex_polygon(vec![rect.center_top() + vec2(-6.0, 0.0), rect.center_top() + vec2(6.0, 0.0), rect.center_top() + vec2(0.0, 8.0)], color, Stroke::NONE));
    if hot {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    if resp.dragged()
        && let Some(p) = resp.interact_pointer_pos()
    {
        // The marker sits after every chip whose middle is left of the pointer.
        let count = app.chips.iter().filter(|c| c.center().x < p.x).count();
        app.roll_to(count);
    }
    resp.on_hover_text("Drag to roll the model back in time. New features go in where the marker is.");
}

pub fn timeline(app: &mut App, ui: &mut Ui) {
    egui::ScrollArea::horizontal().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("TIMELINE").size(10.0).color(DIM));
            let features: Vec<(Id, String, &'static str, bool)> = app.doc().features.iter().map(|f| (f.id, f.name.clone(), feature_icon(&f.kind), f.suppressed)).collect();
            if features.is_empty() {
                ui.label(RichText::new("Features appear here in the order they were made. Start with New Sketch.").color(DIM));
            }
            let (active, total) = (app.doc().active(), features.len());
            let mut chips = Vec::new();
            for (i, (id, name, glyph, suppressed)) in features.into_iter().enumerate() {
                if i == active {
                    marker(app, ui);
                }
                let error = app.session.built.errors.get(&id).cloned();
                let off = suppressed || i >= active;
                let color = if error.is_some() { BAD } else if off { Color32::from_rgb(170, 174, 182) } else { Color32::from_rgb(40, 44, 54) };
                let chosen = app.mode == Mode::Sketch(id) || app.sel_feature == Some(id);
                let resp = ui.add(egui::Button::new(RichText::new(format!("{glyph} {name}")).color(color)).selected(chosen));
                chips.push(resp.rect);
                let resp = match &error {
                    Some(e) => resp.on_hover_text(e),
                    None if i >= active => resp.on_hover_text("Not built: the timeline is rolled back to before this"),
                    None => resp.on_hover_text("Double-click to edit"),
                };
                if resp.double_clicked() && i < active {
                    app.edit_feature(id);
                } else if resp.clicked() {
                    app.sel_feature = Some(id);
                    app.sel_body = app.session.built.body(id).map(|b| b.id);
                }
                resp.context_menu(|ui| {
                    if ui.button("Roll Back to Here").clicked() {
                        app.roll_to(i + 1);
                        ui.close();
                    }
                    feature_menu(app, ui, id, suppressed);
                });
            }
            if total > 0 && active == total {
                marker(app, ui);
            } else if total > 0 && ui.small_button("Roll to End").clicked() {
                app.roll_to(total);
            }
            app.chips = chips;
        });
    });
}

fn eye(ui: &mut Ui, visible: bool) -> bool {
    ui.add(egui::Button::new(RichText::new(if visible { icon::EYE } else { icon::EYE_SLASH }).color(if visible { Color32::from_rgb(40, 44, 54) } else { DIM })).frame(false)).on_hover_text("Show or hide").clicked()
}

pub fn browser(app: &mut App, ui: &mut Ui) {
    ui.label(RichText::new("BROWSER").size(10.0).color(DIM));
    ui.add_space(2.0);
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.label(RichText::new(format!("{} {}", icon::CUBE, app.doc_name())).strong());
        egui::CollapsingHeader::new(format!("{} Document Settings", icon::GEAR)).default_open(true).show(ui, |ui| {
            ui.label(format!("Units: {}", app.doc().units.name()));
            let n = app.doc().params.len();
            if ui.link(format!("Parameters ({n})")).clicked() {
                app.show_params = true;
            }
        });
        egui::CollapsingHeader::new(format!("{} Origin", icon::FOLDER)).default_open(false).show(ui, |ui| {
            for (label, plane) in [("XY plane (top)", Plane::XY), ("XZ plane (front)", Plane::XZ), ("YZ plane (right)", Plane::YZ)] {
                if ui.link(label).on_hover_text("Start a sketch on this plane").clicked() {
                    app.finish_sketch();
                    app.create_sketch(plane);
                }
            }
        });
        egui::CollapsingHeader::new(format!("{} Sketches", icon::FOLDER)).default_open(true).show(ui, |ui| {
            let list: Vec<(Id, String, bool)> = app.doc().sketches().map(|(f, s)| (f.id, f.name.clone(), s.visible)).collect();
            if list.is_empty() {
                ui.label(RichText::new("None yet").color(DIM));
            }
            for (id, name, visible) in list {
                ui.horizontal(|ui| {
                    if eye(ui, visible) {
                        let _ = app.execute(&json!({"op": "set_visible", "id": id, "visible": !visible}));
                    }
                    let resp = ui.selectable_label(app.mode == Mode::Sketch(id), name).on_hover_text("Double-click to edit");
                    if resp.double_clicked() {
                        app.edit_sketch(id);
                    } else if resp.clicked() {
                        app.sel_feature = Some(id);
                    }
                    resp.context_menu(|ui| feature_menu(app, ui, id, false));
                });
            }
        });
        egui::CollapsingHeader::new(format!("{} Bodies", icon::FOLDER)).default_open(true).show(ui, |ui| {
            let list: Vec<(Id, String, bool)> = app.session.built.bodies.iter().map(|b| (b.id, b.name.clone(), !app.doc().hidden_bodies.contains(&b.id))).collect();
            if list.is_empty() {
                ui.label(RichText::new("None yet").color(DIM));
            }
            for (id, name, visible) in list {
                ui.horizontal(|ui| {
                    if eye(ui, visible) {
                        let _ = app.execute(&json!({"op": "set_visible", "id": id, "visible": !visible}));
                    }
                    if ui.selectable_label(app.sel_body == Some(id), name).clicked() {
                        app.sel_body = Some(id);
                    }
                });
            }
        });
    });
}

/// A size box with its evaluated result or error next to it.
fn value_row(app: &App, ui: &mut Ui, label: &str, text: &mut String, kind: Kind) {
    ui.label(label);
    ui.add(egui::TextEdit::singleline(text).desired_width(110.0).hint_text(match kind {
        Kind::Length => "10 mm, $d, d = 5",
        Kind::Angle => "90 deg",
        Kind::Scalar => "1",
    }));
    let rhs = text.split_once('=').map_or(text.as_str(), |p| p.1);
    match app.doc().value(rhs, kind) {
        Ok(v) => ui.label(RichText::new(format!("= {}{}", app.doc().show(&fr_core::Value { expr: String::new(), v: v.v }, kind), if kind == Kind::Length { format!(" {}", app.doc().units.name()) } else { String::new() })).color(DIM)),
        Err(e) => ui.label(RichText::new(icon::WARNING).color(BAD)).on_hover_text(e),
    };
    ui.end_row();
}

fn op_row(ui: &mut Ui, op: &mut Op, allowed: &[Op]) {
    ui.label("Operation");
    egui::ComboBox::from_id_salt("op").width(110.0).selected_text(op.label()).show_ui(ui, |ui| {
        for o in allowed {
            ui.selectable_value(op, *o, o.label());
        }
    });
    ui.end_row();
}

/// OK and Cancel, with the preview's error if there is one.
fn confirm(app: &mut App, ui: &mut Ui, ok: &str) {
    let error = app.preview.as_ref().and_then(|p| p.2.clone());
    if let Some(e) = &error {
        ui.colored_label(BAD, e);
    }
    ui.horizontal(|ui| {
        if ui.add_enabled(error.is_none(), egui::Button::new(ok)).clicked() {
            app.apply_dialog();
        }
        if ui.button("Cancel").clicked() {
            app.dialog = Dialog::None;
        }
    });
}

/// A catalog thread chooser, grouped by family. `fits` narrows it to the sizes worth offering.
fn thread_row(ui: &mut Ui, thread: &mut String, fits: impl Fn(&threads::ThreadSpec) -> bool) {
    ui.label("Thread");
    egui::ComboBox::from_id_salt("thread").width(110.0).selected_text(if thread.is_empty() { "choose" } else { thread.as_str() }).show_ui(ui, |ui| {
        for family in [threads::Family::Metric, threads::Family::Unified] {
            ui.label(RichText::new(family.label()).color(DIM).small());
            for t in threads::CATALOG.iter().filter(|t| t.family == family && fits(t)) {
                ui.selectable_value(thread, t.name.to_owned(), t.name);
            }
        }
    });
    ui.end_row();
}

fn dialog_window(app: &App, title: &str) -> egui::Window<'static> {
    egui::Window::new(title.to_owned()).collapsible(false).resizable(false).pivot(Align2::RIGHT_TOP).default_pos(app.vp.right_top() + vec2(-14.0, 14.0))
}

fn dialogs(app: &mut App, ctx: &Context) {
    match app.dialog.clone() {
        Dialog::None => {}
        Dialog::PickPlane => {
            dialog_window(app, "New Sketch").show(ctx, |ui| {
                ui.label("Sketch on a plane:");
                ui.horizontal(|ui| {
                    for (label, plane) in [("XY (top)", Plane::XY), ("XZ (front)", Plane::XZ), ("YZ (right)", Plane::YZ)] {
                        if ui.button(label).clicked() {
                            app.create_sketch(plane);
                        }
                    }
                });
                ui.label(RichText::new("or click a flat face of a body.").color(DIM));
                ui.horizontal(|ui| {
                    ui.label("Offset");
                    ui.add(egui::TextEdit::singleline(&mut app.plane_offset).desired_width(90.0).hint_text("0 mm")).on_hover_text("Distance from the plane or face, for a sketch above or below it");
                });
                if ui.button("Cancel").clicked() {
                    app.dialog = Dialog::None;
                }
            });
        }
        Dialog::Feature(mut f) => {
            let title = match (f.revolve, f.editing.is_some()) {
                (false, false) => "Extrude",
                (false, true) => "Edit Extrude",
                (true, false) => "Revolve",
                (true, true) => "Edit Revolve",
            };
            dialog_window(app, title).show(ctx, |ui| {
                egui::Grid::new("feature").num_columns(3).show(ui, |ui| {
                    ui.label("Profiles");
                    ui.label(match (f.profiles.len(), &f.face) {
                        (_, Some(_)) => RichText::new("1 face"),
                        (0, None) => RichText::new("click in the viewport").color(ACCENT),
                        (n, None) => RichText::new(format!("{n} selected")),
                    });
                    ui.end_row();
                    if f.revolve {
                        ui.label("Axis");
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut f.axis, Axis::X, "X");
                            ui.selectable_value(&mut f.axis, Axis::Y, "Y");
                            if ui.selectable_label(f.pick_axis || matches!(f.axis, Axis::Line(_)), "Line").on_hover_text("Click a line in the sketch").clicked() {
                                f.pick_axis = true;
                            }
                        });
                        ui.end_row();
                        value_row(app, ui, "Angle", &mut f.text, Kind::Angle);
                    } else {
                        if !f.through_all {
                            value_row(app, ui, "Distance", &mut f.text, Kind::Length);
                        }
                        ui.label("Extent");
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut f.through_all, "Through all").on_hover_text("Go past every body on the side the distance points to");
                            if ui.selectable_label(f.pick_to, "To face").on_hover_text("Click a face to set the distance that reaches it").clicked() {
                                f.pick_to = !f.pick_to;
                            }
                        });
                        ui.end_row();
                        ui.label("Taper");
                        ui.add(egui::TextEdit::singleline(&mut f.taper).desired_width(110.0).hint_text("0 deg")).on_hover_text("Degrees the walls lean outward; negative leans them in");
                        ui.end_row();
                        if f.face.is_none() {
                            ui.label("Direction");
                            ui.checkbox(&mut f.symmetric, "Symmetric");
                            ui.end_row();
                        }
                    }
                    op_row(ui, &mut f.op, &Op::ALL);
                });
                if f.face.is_some() && f.op == Op::Join {
                    let inward = app.doc().value(&f.text, Kind::Length).is_ok_and(|v| v.v < 0.0);
                    ui.label(RichText::new(if inward { "A negative distance pushes the face in and cuts." } else { "Pulls the face out. A negative distance pushes it in and cuts." }).color(DIM));
                }
                app.dialog = Dialog::Feature(f);
                confirm(app, ui, "OK");
            });
        }
        Dialog::Transform(mut t) => {
            dialog_window(app, "Move / Rotate / Scale").show(ctx, |ui| {
                let name = app.session.built.body(t.body).map_or("body".to_owned(), |b| b.name.clone());
                ui.label(RichText::new(name).strong());
                egui::Grid::new("transform").num_columns(3).show(ui, |ui| {
                    for (i, axis) in ["X", "Y", "Z"].iter().enumerate() {
                        value_row(app, ui, &format!("Move {axis}"), &mut t.translate[i], Kind::Length);
                    }
                    for (i, axis) in ["X", "Y", "Z"].iter().enumerate() {
                        value_row(app, ui, &format!("Rotate {axis}"), &mut t.rotate[i], Kind::Angle);
                    }
                    value_row(app, ui, "Scale", &mut t.scale, Kind::Scalar);
                });
                ui.label(RichText::new("Scales about the origin, then rotates, then moves.").color(DIM));
                app.dialog = Dialog::Transform(t);
                confirm(app, ui, "OK");
            });
        }
        Dialog::Combine(mut c) => {
            dialog_window(app, "Combine").show(ctx, |ui| {
                let name = |id: Id| app.session.built.body(id).map_or("?".to_owned(), |b| b.name.clone());
                egui::Grid::new("combine").num_columns(2).show(ui, |ui| {
                    ui.label("Target");
                    ui.label(c.target.map_or(RichText::new("click a body").color(ACCENT), |t| RichText::new(name(t))));
                    ui.end_row();
                    ui.label("Tools");
                    ui.label(if c.tools.is_empty() { RichText::new(if c.target.is_some() { "click bodies" } else { "\u{2014}" }).color(ACCENT) } else { RichText::new(c.tools.iter().map(|t| name(*t)).collect::<Vec<_>>().join(", ")) });
                    ui.end_row();
                    op_row(ui, &mut c.op, &[Op::Join, Op::Cut, Op::Intersect]);
                    ui.label("");
                    ui.checkbox(&mut c.keep_tools, "Keep tools");
                    ui.end_row();
                });
                if ui.small_button("Clear selection").clicked() {
                    c.target = None;
                    c.tools.clear();
                }
                app.dialog = Dialog::Combine(c);
                confirm(app, ui, "OK");
            });
        }
        Dialog::Pattern(mut p) => {
            dialog_window(app, "Pattern").show(ctx, |ui| {
                let sources: Vec<(Id, String)> = app.doc().features.iter().filter(|f| matches!(f.kind, FeatureKind::Extrude(_) | FeatureKind::Revolve(_) | FeatureKind::Import(_)) || matches!(&f.kind, FeatureKind::Text(t) if t.op == Op::New)).map(|f| (f.id, f.name.clone())).collect();
                let shown = sources.iter().find(|s| Some(s.0) == p.source).map_or("choose".to_owned(), |s| s.1.clone());
                egui::Grid::new("pattern").num_columns(3).show(ui, |ui| {
                    ui.label("Repeat");
                    egui::ComboBox::from_id_salt("source").width(110.0).selected_text(shown).show_ui(ui, |ui| {
                        for (id, name) in &sources {
                            ui.selectable_value(&mut p.source, Some(*id), name);
                        }
                    });
                    ui.end_row();
                    ui.label("Type");
                    ui.horizontal(|ui| {
                        for (i, label, text) in [(0, "Circular", "360 deg"), (1, "Linear", "10 mm"), (2, "Mirror", "")] {
                            if ui.selectable_label(p.kind == i, label).clicked() && p.kind != i {
                                p.kind = i;
                                p.text = text.into();
                                p.axis = [2, 0, 0][i];
                            }
                        }
                    });
                    ui.end_row();
                    ui.label(if p.kind == 2 { "Across" } else { "Axis" });
                    ui.horizontal(|ui| {
                        for (i, label) in [if p.kind == 2 { "YZ plane" } else { "X" }, if p.kind == 2 { "XZ plane" } else { "Y" }, if p.kind == 2 { "XY plane" } else { "Z" }].iter().enumerate() {
                            ui.selectable_value(&mut p.axis, i, *label);
                        }
                    });
                    ui.end_row();
                    if p.kind != 2 {
                        ui.label("Count");
                        ui.add(egui::DragValue::new(&mut p.count).range(2..=360)).on_hover_text("Including the original");
                        ui.end_row();
                        value_row(app, ui, if p.kind == 0 { "Angle" } else { "Spacing" }, &mut p.text, if p.kind == 0 { Kind::Angle } else { Kind::Length });
                    }
                });
                ui.label(RichText::new("Axes and planes are the model's, through the origin.").color(DIM));
                app.dialog = Dialog::Pattern(p);
                confirm(app, ui, "OK");
            });
        }
        Dialog::Blend(mut b) => {
            dialog_window(app, if b.chamfer { "Chamfer" } else { "Fillet" }).show(ctx, |ui| {
                egui::Grid::new("blend").num_columns(3).show(ui, |ui| {
                    ui.label("Edges");
                    ui.label(match b.edges.len() {
                        0 => RichText::new("click edges in the viewport").color(ACCENT),
                        n => RichText::new(format!("{n} selected")),
                    });
                    ui.end_row();
                    value_row(app, ui, if b.chamfer { "Distance" } else { "Radius" }, &mut b.text, Kind::Length);
                });
                if ui.small_button("Clear edges").clicked() {
                    b.edges.clear();
                }
                app.dialog = Dialog::Blend(b);
                confirm(app, ui, "OK");
            });
        }
        Dialog::Shell(mut sh) => {
            dialog_window(app, "Shell").show(ctx, |ui| {
                egui::Grid::new("shell").num_columns(3).show(ui, |ui| {
                    ui.label("Open faces");
                    ui.label(match sh.faces.len() {
                        0 => RichText::new("click faces in the viewport").color(ACCENT),
                        n => RichText::new(format!("{n} selected")),
                    });
                    ui.end_row();
                    value_row(app, ui, "Wall", &mut sh.text, Kind::Length);
                });
                ui.label(RichText::new("The wall is measured inward from the outside.").color(DIM));
                app.dialog = Dialog::Shell(sh);
                confirm(app, ui, "OK");
            });
        }
        Dialog::Hole(mut h) => {
            dialog_window(app, "Hole").show(ctx, |ui| {
                let plain = h.fit == HoleFit::Plain;
                egui::Grid::new("hole").num_columns(3).show(ui, |ui| {
                    ui.label("Position");
                    ui.label(match h.at.len() {
                        0 => RichText::new("click a flat face").color(ACCENT),
                        1 => RichText::new("1 hole"),
                        n => RichText::new(format!("{n} holes")),
                    });
                    if !h.at.is_empty() && ui.small_button("Clear").clicked() {
                        h.at.clear();
                    }
                    ui.end_row();
                    ui.label("Type");
                    ui.horizontal(|ui| {
                        for s in HoleShape::ALL {
                            ui.selectable_value(&mut h.shape, s, s.label());
                        }
                    });
                    ui.end_row();
                    ui.label("For");
                    egui::ComboBox::from_id_salt("fit").width(150.0).selected_text(h.fit.label()).show_ui(ui, |ui| {
                        for f in HoleFit::ALL {
                            ui.selectable_value(&mut h.fit, f, f.label());
                        }
                    });
                    ui.end_row();
                    if plain {
                        value_row(app, ui, "Diameter", &mut h.diameter, Kind::Length);
                    } else {
                        thread_row(ui, &mut h.thread, |_| true);
                    }
                    ui.label("Depth");
                    ui.checkbox(&mut h.through, "Through all");
                    ui.end_row();
                    if !h.through {
                        value_row(app, ui, "", &mut h.depth, Kind::Length);
                        ui.label("");
                        ui.checkbox(&mut h.pointed, "Drill point").on_hover_text("A 118 degree cone at the bottom, as a twist drill leaves");
                        ui.end_row();
                    }
                    if h.fit == HoleFit::Tapped {
                        ui.label("Thread");
                        let model = ui.checkbox(&mut h.modeled, "Model it").on_hover_text("Adds the real thread, for printing. Off, the hole is left at the tap drill size, to be tapped or for a screw to cut its own thread.");
                        // A printed thread needs room to turn, so one is offered along with it.
                        if model.changed() && h.modeled && h.extra.trim().is_empty() {
                            h.extra = if app.doc().units == Unit::In { "0.008 in".to_owned() } else { format!("{} mm", crate::app::PRINT_ALLOWANCE) };
                        }
                        ui.add_enabled(h.modeled, egui::Checkbox::new(&mut h.left, "Left hand"));
                        ui.end_row();
                    }
                    if h.shape != HoleShape::Simple {
                        let what = if h.shape == HoleShape::Counterbore { "Counterbore" } else { "Countersink" };
                        if !plain {
                            ui.label(what);
                            ui.checkbox(&mut h.custom_head, "Custom size").on_hover_text("Otherwise it fits a socket cap screw (counterbore) or a flat head screw (countersink) of the thread size");
                            ui.end_row();
                        }
                        if plain || h.custom_head {
                            value_row(app, ui, if plain { what } else { "" }, &mut h.head_diameter, Kind::Length);
                            if h.shape == HoleShape::Counterbore {
                                value_row(app, ui, "its depth", &mut h.head_depth, Kind::Length);
                            } else {
                                value_row(app, ui, "its angle", &mut h.head_angle, Kind::Angle);
                            }
                        }
                    }
                    ui.label("Allowance").on_hover_text("Added to every diameter, for a printer that makes holes come out small");
                    ui.add(egui::TextEdit::singleline(&mut h.extra).desired_width(110.0).hint_text("none, or 0.2 mm"));
                    ui.end_row();
                });
                // What those choices come to.
                if let Ok(sizes) = h.hole(&mut app.doc().clone()).and_then(|hole| hole.sizes()) {
                    let u = app.doc().units;
                    let len = |mm: f64| format!("{} {}", fr_core::units::fmt_len(mm, u), u.name());
                    let head = match sizes.head {
                        Some(fr_core::exact::DrillHead::Counterbore { diameter, depth }) => format!(", counterbore {} by {} deep", len(diameter), len(depth)),
                        Some(fr_core::exact::DrillHead::Countersink { diameter, angle }) => format!(", countersink {} at {angle}\u{b0}", len(diameter)),
                        None => String::new(),
                    };
                    let bore = match sizes.thread {
                        Some((major, pitch)) => format!("Threaded {} with a {} pitch", len(major), len(pitch)),
                        None => format!("Drilled {}", len(sizes.diameter)),
                    };
                    ui.label(RichText::new(format!("{bore}{head}")).color(DIM));
                }
                app.dialog = Dialog::Hole(h);
                confirm(app, ui, "OK");
            });
        }
        Dialog::Text(mut t) => {
            dialog_window(app, if t.editing.is_some() { "Edit Text / Emboss" } else { "Text / Emboss" }).show(ctx, |ui| {
                ui.set_max_width(380.0);
                if t.editing.is_some() {
                    ui.label(RichText::new("Later features are hidden while editing this text.").small().color(DIM));
                }
                ui.label("Text");
                ui.add(egui::TextEdit::singleline(&mut t.text).desired_width(340.0).char_limit(128).hint_text("Enter text"));
                ui.label(RichText::new("Sans Bold · up to 128 characters").small().color(DIM));
                ui.separator();
                egui::Grid::new("text_placement").num_columns(2).show(ui, |ui| {
                    ui.label("Place on");
                    ui.horizontal(|ui| {
                        for (label, plane) in [("XY", Plane::XY), ("XZ", Plane::XZ), ("YZ", Plane::YZ)] {
                            if ui.selectable_label(t.body.is_none() && t.plane == plane, label).clicked() {
                                (t.plane, t.body, t.face, t.frame, t.op) = (plane, None, None, None, Op::New);
                            }
                        }
                    });
                    ui.end_row();
                    ui.label("Face");
                    ui.label(match t.body {
                        Some(id) => RichText::new(app.doc().feature(id).map_or("Selected flat face", |f| f.name.as_str())),
                        None => RichText::new("click a flat face in the viewport").color(ACCENT),
                    });
                    ui.end_row();
                    ui.label("Operation");
                    ui.horizontal(|ui| {
                        for (label, op) in [("New body", Op::New), ("Raised", Op::Join), ("Engraved", Op::Cut)] {
                            ui.selectable_value(&mut t.op, op, label);
                        }
                    });
                    ui.end_row();
                    ui.label("Alignment");
                    ui.horizontal(|ui| {
                        for (label, align) in [("Left", fr_core::text::Align::Left), ("Center", fr_core::text::Align::Center), ("Right", fr_core::text::Align::Right)] {
                            ui.selectable_value(&mut t.align, align, label);
                        }
                    });
                    ui.end_row();
                });
                ui.separator();
                egui::Grid::new("text_sizes").num_columns(3).show(ui, |ui| {
                    value_row(app, ui, "Letter height", &mut t.height, Kind::Length);
                    value_row(app, ui, "Depth", &mut t.depth, Kind::Length);
                    value_row(app, ui, "Extra spacing", &mut t.spacing, Kind::Length);
                    value_row(app, ui, "X offset", &mut t.x, Kind::Length);
                    value_row(app, ui, "Y offset", &mut t.y, Kind::Length);
                    value_row(app, ui, "Angle", &mut t.angle, Kind::Angle);
                });
                ui.label(RichText::new("Offsets and alignment use the baseline at the origin or clicked point. Flat solid faces only; curved wrapping is not available.").small().color(DIM));
                app.dialog = Dialog::Text(t);
                confirm(app, ui, "OK");
            });
        }
        Dialog::Thread(mut t) => {
            dialog_window(app, "Thread").show(ctx, |ui| {
                egui::Grid::new("thread").num_columns(3).show(ui, |ui| {
                    ui.label("On");
                    ui.label(match t.found {
                        None => RichText::new("click a rod or a hole").color(ACCENT),
                        Some((across, internal)) => RichText::new(format!("{} {} {}", fr_core::units::fmt_len(across, app.doc().units), app.doc().units.name(), if internal { "hole" } else { "rod" })),
                    });
                    ui.end_row();
                    if let Some((across, internal)) = t.found {
                        // A hole can be remade for any thread; a rod can only be turned down.
                        thread_row(ui, &mut t.thread, |s| internal || across >= s.major * 0.9);
                        ui.label("Length");
                        ui.checkbox(&mut t.full, "Whole cylinder");
                        ui.end_row();
                        if !t.full {
                            value_row(app, ui, "from the end", &mut t.offset, Kind::Length);
                            value_row(app, ui, "for", &mut t.length, Kind::Length);
                        }
                        ui.label("");
                        ui.checkbox(&mut t.left, "Left hand");
                        ui.end_row();
                        ui.label("Allowance").on_hover_text(if internal { "Makes the hole's thread this much wider across, so a screw turns in it." } else { "Makes the rod's thread this much thinner across, so it turns in a nut or hole." });
                        ui.add(egui::TextEdit::singleline(&mut t.extra).desired_width(110.0).hint_text("none"));
                        ui.end_row();
                    }
                });
                let remade = t.found.zip(threads::find(&t.thread).ok()).is_some_and(|((across, internal), s)| if internal { across > s.major + threads::BED || across < s.minor() * 0.9 } else { across > s.major * 1.02 });
                ui.label(RichText::new(match (remade, t.found) {
                    (true, Some((_, true))) => "The hole will be remade to this thread's size.",
                    (true, _) => "The rod will be turned down to this thread's size.",
                    _ => "A hole of any size is remade to suit the thread;\na rod must be at least the thread's size across.",
                })
                .color(DIM));
                if t.found.is_some() {
                    ui.label(RichText::new(if t.extra.trim().is_empty() { "No allowance: the exact size. Printed, it will not turn\nin another thread made the same way." } else { "The allowance is room to turn when printed.\nClear it for the exact size." }).color(DIM));
                }
                app.dialog = Dialog::Thread(t);
                confirm(app, ui, "OK");
            });
        }
        Dialog::Measure(mut m) => {
            dialog_window(app, "Measure").show(ctx, |ui| {
                let u = app.doc().units;
                let len = |mm: f64| format!("{} {}", fr_core::units::fmt_len(mm, u), u.name());
                for (i, p) in m.picks.iter().enumerate() {
                    ui.label(format!("{}  {}", i + 1, p.label));
                }
                if m.picks.len() < 2 {
                    ui.label(RichText::new(if m.picks.is_empty() { "click a point, an edge or a face" } else { "click a second one" }).color(ACCENT));
                }
                if let Some(r) = m.result {
                    ui.separator();
                    egui::Grid::new("measure").num_columns(2).show(ui, |ui| {
                        // Square across is what a drawing would dimension; the closest points can be further when the two do not overlap.
                        if let Some(apart) = r.apart {
                            ui.label("Apart");
                            ui.label(RichText::new(len(apart)).strong());
                            ui.end_row();
                        }
                        if r.apart.is_none_or(|a| (a - r.distance).abs() > 1e-6) {
                            ui.label(if r.apart.is_some() { "Closest points" } else { "Distance" });
                            ui.label(if r.apart.is_some() { RichText::new(len(r.distance)) } else { RichText::new(len(r.distance)).strong() });
                            ui.end_row();
                        }
                        if let Some(angle) = r.angle {
                            ui.label("Angle");
                            ui.label(format!("{}\u{b0}", fr_core::units::trim_num(angle, 3)));
                            ui.end_row();
                        }
                        let d = r.to - r.from;
                        for (axis, v) in [("X", d.x), ("Y", d.y), ("Z", d.z)] {
                            ui.label(RichText::new(format!("\u{394}{axis}")).color(DIM));
                            ui.label(RichText::new(len(v.abs())).color(DIM));
                            ui.end_row();
                        }
                    });
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(!m.picks.is_empty(), egui::Button::new("Clear")).clicked() {
                        m = Default::default();
                    }
                    if ui.button("Close").clicked() {
                        app.dialog = Dialog::None;
                    } else {
                        app.dialog = Dialog::Measure(m.clone());
                    }
                });
            });
        }
        Dialog::Export(mut unit) => {
            dialog_window(app, "Export STL").show(ctx, |ui| {
                ui.label("STL files carry numbers without units. Write them in:");
                ui.horizontal(|ui| {
                    for u in Unit::ALL {
                        ui.selectable_value(&mut unit, u, u.name());
                    }
                });
                ui.label(RichText::new(if unit == Unit::Mm { "Slicers read STL as millimetres, so this prints at true size." } else { "Most slicers assume millimetres; tell yours the file is in these units." }).color(DIM));
                app.dialog = Dialog::Export(unit);
                ui.horizontal(|ui| {
                    if ui.button("Export\u{2026}").clicked() {
                        app.dialog = Dialog::None;
                        app.export_stl(unit);
                    }
                    if ui.button("Cancel").clicked() {
                        app.dialog = Dialog::None;
                    }
                });
            });
        }
        Dialog::Import(path, mut unit) => {
            dialog_window(app, "Import STL").show(ctx, |ui| {
                ui.label(path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned()));
                ui.label("The file's numbers are in:");
                ui.horizontal(|ui| {
                    for u in Unit::ALL {
                        ui.selectable_value(&mut unit, u, u.name());
                    }
                });
                app.dialog = Dialog::Import(path.clone(), unit);
                ui.horizontal(|ui| {
                    if ui.button("Import").clicked() {
                        app.dialog = Dialog::None;
                        app.import_stl(&path, unit);
                    }
                    if ui.button("Cancel").clicked() {
                        app.dialog = Dialog::None;
                    }
                });
            });
        }
    }
}

fn sketch_palette(app: &mut App, ctx: &Context) {
    if app.sketch().is_none() {
        return;
    }
    dialog_window(app, "Sketch Palette").show(ctx, |ui| {
        ui.checkbox(&mut app.opts.construction, "Construction").on_hover_text("Draw new geometry as construction lines (X)");
        ui.horizontal(|ui| {
            ui.label("Polygon sides");
            ui.add(egui::DragValue::new(&mut app.opts.sides).range(3..=64));
        });
        ui.checkbox(&mut app.opts.grid, "Sketch Grid");
        ui.checkbox(&mut app.opts.snap_grid, "Snap to Grid");
        ui.checkbox(&mut app.opts.constraints, "Show Constraints");
        ui.checkbox(&mut app.opts.dimensions, "Show Dimensions");
        ui.horizontal(|ui| {
            if ui.button("Look At").on_hover_text("Face the sketch plane").clicked()
                && let Mode::Sketch(id) = app.mode
            {
                app.edit_sketch(id);
            }
            if ui.button("Finish Sketch").clicked() {
                app.finish_sketch();
            }
        });
    });
}

fn section(app: &mut App, ctx: &Context) {
    if !app.show_section {
        return;
    }
    let mut open = true;
    let unit = app.doc().units;
    egui::Window::new("Section Analysis").open(&mut open).resizable(false).pivot(Align2::LEFT_BOTTOM).default_pos(app.vp.left_bottom() + vec2(14.0, -40.0)).show(ctx, |ui| {
        let s = &mut app.section;
        ui.checkbox(&mut s.on, "Cut the view open");
        ui.horizontal(|ui| {
            ui.label("Plane");
            for (i, label) in ["YZ", "XZ", "XY"].iter().enumerate() {
                ui.selectable_value(&mut s.axis, i, *label);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Position");
            let mut shown = s.offset / unit.mm();
            if ui.add(egui::DragValue::new(&mut shown).speed(0.5 / unit.mm()).max_decimals(3).suffix(format!(" {}", unit.name()))).changed() {
                s.offset = shown * unit.mm();
            }
        });
        ui.checkbox(&mut s.flip, "Show the other side");
        ui.label(RichText::new("Cut faces are hatched. This only changes the view, not the model.").color(DIM));
    });
    app.show_section = open;
}

fn parameters(app: &mut App, ctx: &Context) {
    if !app.show_params {
        return;
    }
    let mut open = true;
    egui::Window::new("Parameters").open(&mut open).resizable(false).default_pos(app.vp.left_top() + vec2(14.0, 14.0)).show(ctx, |ui| {
        ui.label(RichText::new("Use a parameter in any size box as $name. Changing one rebuilds everything that uses it.").color(DIM));
        let rows: Vec<(String, String, String)> = app.doc().params.iter().map(|p| (p.name.clone(), p.expr.clone(), app.doc().show_param(&p.name))).collect();
        egui::Grid::new("params").num_columns(4).striped(true).show(ui, |ui| {
            ui.label(RichText::new("Name").strong());
            ui.label(RichText::new("Expression").strong());
            ui.label(RichText::new("Value").strong());
            ui.end_row();
            for (name, expr, value) in rows {
                ui.label(format!("${name}"));
                // The box edits a copy; it is applied when the box loses focus.
                let editing = app.param_edit.as_ref().is_some_and(|e| e.0 == name);
                let mut text = if editing { app.param_edit.as_ref().unwrap().1.clone() } else { expr.clone() };
                let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(150.0));
                if r.changed() || (r.has_focus() && !editing) {
                    app.param_edit = Some((name.clone(), text.clone()));
                }
                if r.lost_focus() && editing {
                    app.param_edit = None;
                    if text.trim() != expr
                        && let Err(e) = app.execute(&json!({"op": "set_parameter", "name": name, "expr": text}))
                    {
                        app.toast(e);
                    }
                }
                ui.label(RichText::new(value).color(DIM));
                if ui.small_button(icon::TRASH).on_hover_text("Delete").clicked()
                    && let Err(e) = app.execute(&json!({"op": "delete_parameter", "name": name}))
                {
                    app.toast(e);
                }
                ui.end_row();
            }
            ui.add(egui::TextEdit::singleline(&mut app.param_new.0).desired_width(70.0).hint_text("name"));
            let r = ui.add(egui::TextEdit::singleline(&mut app.param_new.1).desired_width(150.0).hint_text("10 mm"));
            ui.label("");
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("Add").clicked() || enter) && !app.param_new.0.trim().is_empty() {
                let (name, expr) = (app.param_new.0.trim().to_owned(), app.param_new.1.clone());
                match app.execute(&json!({"op": "set_parameter", "name": name, "expr": expr})) {
                    Ok(_) => app.param_new = Default::default(),
                    Err(e) => app.toast(e),
                }
            }
            ui.end_row();
        });
    });
    app.show_params = open;
}

fn assistant(app: &mut App, ctx: &Context) {
    if !app.ai.open {
        return;
    }
    let mut open = true;
    let mut ai = std::mem::take(&mut app.ai);
    egui::Window::new(format!("{} Assistant", icon::SPARKLE)).open(&mut open).default_size([340.0, 420.0]).pivot(Align2::RIGHT_BOTTOM).default_pos(app.vp.right_bottom() + vec2(-14.0, -14.0)).show(ctx, |ui| {
        if ui.checkbox(&mut app.config.bridge.enabled, "Allow local MCP clients").on_hover_text("Allows programs running as your user to control this document. Changes take effect after restarting Ferrender.").changed() {
            if let Err(e) = app.config.save() {
                app.toast(e);
            } else {
                app.toast("Local MCP setting saved. Restart Ferrender to apply it.");
            }
        }
        ui.separator();
        if app.config.api_key().is_none() {
            ui.label("The assistant uses the Claude API. Paste an Anthropic API key, or set ANTHROPIC_API_KEY before starting Ferrender.");
            ui.add(egui::TextEdit::singleline(&mut ai.key_input).password(true).hint_text("sk-ant-\u{2026}").desired_width(f32::INFINITY));
            if ui.button("Save Key").clicked() && !ai.key_input.trim().is_empty() {
                app.config.ai.api_key = ai.key_input.trim().to_owned();
                ai.key_input.clear();
                if let Err(e) = app.config.save() {
                    app.toast(e);
                }
            }
            ui.label(RichText::new(format!("Saved to {}", crate::config::Config::path().display())).color(DIM).small());
            return;
        }
        egui::ScrollArea::vertical().max_height(ui.available_height() - 74.0).auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
            if ai.log.is_empty() {
                ui.label(RichText::new("Describe a part, or a change to this one. For example: \u{201c}a 60 by 40 mm plate, 5 mm thick, with a 6 mm hole near each corner\u{201d}.").color(DIM));
            }
            for (who, text) in &ai.log {
                match who {
                    Who::User => ui.label(RichText::new(text).strong()),
                    Who::Assistant => ui.label(text),
                    Who::Action => ui.label(RichText::new(format!("{} {text}", icon::GEAR)).color(DIM).small()),
                    Who::Error => ui.colored_label(BAD, text),
                };
                ui.add_space(3.0);
            }
            if ai.busy() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Working\u{2026}").color(DIM));
                });
            }
        });
        ui.separator();
        let r = ui.add(egui::TextEdit::multiline(&mut ai.input).desired_rows(2).desired_width(f32::INFINITY).hint_text("Ask for a part or a change"));
        let enter = r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
        ui.horizontal(|ui| {
            let ready = !ai.busy() && !ai.input.trim().is_empty();
            if (ui.add_enabled(ready, egui::Button::new("Send")).on_hover_text(cmd("Enter")).clicked() || enter) && ready {
                let prompt = std::mem::take(&mut ai.input).trim().to_owned();
                ai.send(app, prompt);
            }
            if ui.add_enabled(!ai.busy(), egui::Button::new("New Chat")).clicked() {
                ai.clear();
            }
            ui.label(RichText::new(app.config.model()).color(DIM).small());
        });
    });
    ai.open = open;
    app.ai = ai;
}

fn rename(app: &mut App, ctx: &Context) {
    let Some((id, mut name)) = app.rename.clone() else { return };
    let mut done = false;
    egui::Window::new("Rename").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
        let r = ui.text_edit_singleline(&mut name);
        r.request_focus();
        ui.horizontal(|ui| {
            if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let _ = app.execute(&json!({"op": "edit_feature", "feature": id, "name": name.trim()}));
                done = true;
            }
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                done = true;
            }
        });
    });
    app.rename = (!done).then_some((id, name));
}

/// The view buttons in the viewport's top-left corner.
fn view_buttons(app: &mut App, ctx: &Context) {
    egui::Area::new("views".into()).fixed_pos(app.vp.left_top() + vec2(10.0, 10.0)).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (label, view) in [(icon::HOUSE, "iso"), ("Top", "top"), ("Front", "front"), ("Right", "right")] {
                if ui.small_button(label).on_hover_text(if view == "iso" { "Home view" } else { "Look from this side" }).clicked() {
                    app.run(ctx, Action::View(view));
                }
            }
            if ui.small_button(icon::ARROWS_OUT).on_hover_text("Fit everything in view (F)").clicked() {
                app.run(ctx, Action::Fit);
            }
        });
    });
}

/// Unsaved designs left behind by a crash, offered when the app starts.
fn recover(app: &mut App, ctx: &Context) {
    if app.recover.is_empty() {
        return;
    }
    let mut later = false;
    egui::Window::new("Recover unsaved work").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
        ui.label("Ferrender did not close normally. These designs had changes that were not saved:");
        ui.add_space(4.0);
        egui::Grid::new("recover").num_columns(4).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
            for f in app.recover.clone() {
                ui.label(RichText::new(f.name()).strong()).on_hover_text(f.path.as_ref().map_or("Never saved".to_owned(), |p| p.display().to_string()));
                ui.label(RichText::new(f.age()).color(DIM));
                if ui.button("Recover").on_hover_text("Open it here. It stays unsaved until you save it.").clicked() {
                    app.recover(&f);
                }
                if ui.button("Delete").on_hover_text("Throw this copy away").clicked() {
                    f.delete();
                    app.recover.retain(|g| *g != f);
                }
                ui.end_row();
            }
        });
        ui.add_space(4.0);
        if ui.button("Later").on_hover_text("Keep them for now; File \u{203a} Recover Unsaved brings this back").clicked() {
            later = true;
        }
    });
    if later {
        app.recover.clear();
    }
}

/// Unlike notifications, a failed open/import must survive a slow native picker.
fn file_error(app: &mut App, ctx: &Context) {
    let Some(error) = app.file_error.clone() else { return };
    egui::Modal::new("file_error".into()).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.heading(&error.title);
        ui.label(RichText::new(error.path.display().to_string()).color(DIM));
        ui.separator();
        egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| { ui.label(&error.message); });
        ui.add_space(6.0);
        ui.label(error.guidance);
        ui.horizontal(|ui| {
            if ui.button("Dismiss").clicked() { app.file_error = None; }
            if ui.button("Copy details").clicked() {
                ctx.copy_text(format!("{}\n{}\n{}", error.title, error.path.display(), error.message));
            }
        });
    });
}

pub fn windows(app: &mut App, ctx: &Context) {
    view_buttons(app, ctx);
    dialogs(app, ctx);
    sketch_palette(app, ctx);
    section(app, ctx);
    parameters(app, ctx);
    assistant(app, ctx);
    rename(app, ctx);
    recover(app, ctx);
    file_error(app, ctx);
}
