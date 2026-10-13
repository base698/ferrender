//! The Loads panel: a deck load over the design's tagged members, shown on the model.
//! Every number comes from `fr_core::structure`; this file asks and draws.

use std::collections::BTreeMap;

use egui::{Color32, Context, Painter, RichText, Shape, Stroke, vec2};
use fr_core::Id;
use fr_core::structure::Case;
use glam::DVec3;
use serde_json::{Value as J, json};

use crate::app::{App, Dialog};
use crate::theme::Palette;

const PSF: f64 = 47.880_26;
const PSI: f64 = 0.006_894_757;
const INCH: f64 = 25.4;
const PCF: f64 = 16.018_46;

/// The lumber a tag is made from, as typed.
#[derive(Clone, Debug, PartialEq)]
pub struct Lumber {
    pub species: String,
    pub grade: String,
    pub e: String,
    pub fb: String,
    pub fv: String,
    /// The repetitive-member factor, 1.15, for joists that share a deck.
    pub repetitive: bool,
    pub density: String,
    pub bearing: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoadsDlg {
    /// Pounds, inches and psi rather than kPa, mm and MPa.
    pub us: bool,
    pub live: String,
    pub dead: String,
    pub own_weight: bool,
    /// How many times over the sag is drawn.
    pub scale: f64,
    /// Compare with every member at this width and depth.
    pub trial: bool,
    pub trial_width: String,
    pub trial_depth: String,
    /// Draw the trial rather than the design as it is.
    pub show_trial: bool,
    /// Deck widths typed in place of the ones taken from spacing.
    pub widths: BTreeMap<Id, String>,
    pub lumber: Lumber,
}

fn number(text: &str, what: &str) -> Result<f64, String> {
    text.trim().parse::<f64>().ok().filter(|v| v.is_finite()).ok_or(format!("{what} should be a number"))
}

fn tidy(value: f64) -> String {
    fr_core::units::trim_num(value, 3)
}

/// A sag, to the hundredth of an inch or the tenth of a millimetre.
fn sag_text(mm: f64, us: bool) -> String {
    if us { format!("{:.2} in", mm / INCH) } else { format!("{:.1} mm", mm) }
}

impl LoadsDlg {
    /// The panel as it opens: the case kept with the design, or a light storage load.
    pub fn open(app: &App) -> LoadsDlg {
        let us = matches!(app.doc().units, fr_core::Unit::In);
        let (pressure, length, stress, density) = if us { (PSF, INCH, PSI, PCF) } else { (1000.0, 1.0, 1.0, 1.0) };
        let first = app.doc().structural_members.values().next();
        let lumber = Lumber {
            species: first.map_or(String::new(), |t| t.species.clone()),
            grade: first.map_or(String::new(), |t| t.grade.clone()),
            e: first.map_or(String::new(), |t| tidy(t.design_values.e_mpa / stress)),
            fb: first.map_or(String::new(), |t| tidy(t.design_values.fb_mpa / stress)),
            fv: first.map_or(String::new(), |t| tidy(t.design_values.fv_mpa / stress)),
            repetitive: first.is_some_and(|t| t.design_values.factors.bending.contains_key("Cr")),
            density: first.and_then(|t| t.density_kg_m3).map_or(String::new(), |d| tidy(d / density)),
            bearing: first.and_then(|t| t.end_bearing_mm).map_or(String::new(), |b| tidy(b[0] / length)),
        };
        let kept = app.doc().structural_case.clone();
        let trial = kept.as_ref().and_then(|c| c.trial_section_mm);
        LoadsDlg {
            us,
            live: kept.as_ref().map_or(if us { "30".into() } else { "1.5".into() }, |c| tidy(c.live_pa / pressure)),
            dead: kept.as_ref().map_or(if us { "5".into() } else { "0.25".into() }, |c| tidy(c.dead_pa / pressure)),
            own_weight: kept.as_ref().is_none_or(|c| c.include_self_weight),
            scale: kept.as_ref().map_or(10.0, |c| c.deformation_scale),
            trial: trial.is_some(),
            trial_width: trial.map_or(String::new(), |t| tidy(t[0] / length)),
            trial_depth: trial.map_or(String::new(), |t| tidy(t[1] / length)),
            show_trial: false,
            widths: kept.map_or(BTreeMap::new(), |c| c.tributary_mm.iter().map(|(id, w)| (*id, tidy(w / length))).collect()),
            lumber,
        }
    }

    fn length(&self) -> f64 { if self.us { INCH } else { 1.0 } }
    fn pressure_name(&self) -> &'static str { if self.us { "psf" } else { "kPa" } }
    fn length_name(&self) -> &'static str { if self.us { "in" } else { "mm" } }
    fn stress_name(&self) -> &'static str { if self.us { "psi" } else { "MPa" } }

    /// The load case typed into the panel, with or without its trial section.
    pub fn case(&self, trial: bool) -> Result<Case, String> {
        let pressure = if self.us { PSF } else { 1000.0 };
        let mut tributary_mm = BTreeMap::new();
        for (id, text) in self.widths.iter().filter(|(_, t)| !t.trim().is_empty()) {
            tributary_mm.insert(*id, number(text, "A deck width")? * self.length());
        }
        let trial_section_mm = if trial {
            Some([number(&self.trial_width, "The trial width")? * self.length(), number(&self.trial_depth, "The trial depth")? * self.length()])
        } else {
            None
        };
        let case = Case {
            live_pa: number(&self.live, "The storage load")? * pressure,
            dead_pa: number(&self.dead, "The deck's weight")? * pressure,
            include_self_weight: self.own_weight,
            tributary_mm,
            trial_section_mm,
            deformation_scale: self.scale,
        };
        case.validate()?;
        Ok(case)
    }

    /// The tag the lumber boxes describe.
    fn tag(&self) -> Result<J, String> {
        let (stress, density) = if self.us { (PSI, PCF) } else { (1.0, 1.0) };
        let l = &self.lumber;
        let word = |text: &str| if text.trim().is_empty() { "not recorded".to_owned() } else { text.trim().to_owned() };
        let mut tag = json!({
            "species": word(&l.species), "grade": word(&l.grade), "nominal_size": "as modelled",
            "design_values": {
                "e_mpa": number(&l.e, "E")? * stress, "fb_mpa": number(&l.fb, "Fb")? * stress, "fv_mpa": number(&l.fv, "Fv")? * stress,
                "source": "typed into the Loads panel; check against the grade stamp and the published table",
                "factors": {"bending": if l.repetitive { json!({"Cr": 1.15}) } else { json!({}) }},
            },
        });
        if !l.density.trim().is_empty() { tag["density_kg_m3"] = json!(number(&l.density, "The density")? * density); }
        if !l.bearing.trim().is_empty() {
            let bearing = number(&l.bearing, "The bearing length")? * self.length();
            tag["end_bearing_mm"] = json!([bearing, bearing]);
        }
        Ok(tag)
    }
}

/// The estimate for the panel as it stands, and for its trial section when it has one.
/// Worked out again only when the design or the panel changes.
pub type Estimates = (Result<J, String>, Option<Result<J, String>>);

pub fn estimates(app: &mut App, d: &LoadsDlg) -> Estimates {
    let rev = app.session.rev;
    if let Some((at, panel, made)) = &app.loads && *at == rev && panel == d {
        return made.clone();
    }
    let run = |trial: bool| d.case(trial).and_then(|case| fr_core::structure::estimate(&app.session, &case));
    let made = (run(false), d.trial.then(|| run(true)));
    app.loads = Some((rev, d.clone(), made.clone()));
    made
}

fn rgb(member: &J) -> Color32 {
    let c = |i: usize| member["color_rgb"][i].as_u64().unwrap_or(128) as u8;
    Color32::from_rgb(c(0), c(1), c(2))
}

fn point(v: &J) -> DVec3 {
    DVec3::new(v[0].as_f64().unwrap_or(0.0), v[1].as_f64().unwrap_or(0.0), v[2].as_f64().unwrap_or(0.0))
}

/// The distance between a member's two supports.
fn span(member: &J) -> f64 {
    let g = &member["geometry"];
    match g["support_centers_from_start_mm"].as_array() {
        Some(c) => c[1].as_f64().unwrap_or(0.0) - c[0].as_f64().unwrap_or(0.0),
        None => g["length_mm"].as_f64().unwrap_or(0.0),
    }
}

fn find(result: &J, body: Id) -> Option<&J> {
    result["members"].as_array()?.iter().find(|m| m["body"].as_u64() == Some(body as u64))
}

pub fn dialog(app: &mut App, ctx: &Context, mut d: LoadsDlg) {
    let colors = Palette::from_ctx(ctx);
    let (now, trial) = estimates(app, &d);
    let mut done = false;
    crate::panels::dialog_window(app, "Loads").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label("Units");
            ui.selectable_value(&mut d.us, true, "psf, in, psi");
            ui.selectable_value(&mut d.us, false, "kPa, mm, MPa");
        });
        egui::Grid::new("loads").num_columns(3).show(ui, |ui| {
            ui.label("Storage load").on_hover_text("What is stored or stood on the deck, spread evenly");
            ui.add(egui::TextEdit::singleline(&mut d.live).desired_width(70.0));
            ui.label(d.pressure_name());
            ui.end_row();
            ui.label("Deck weight").on_hover_text("The sheathing and anything fixed to it. The members' own weight is counted separately.");
            ui.add(egui::TextEdit::singleline(&mut d.dead).desired_width(70.0));
            ui.label(d.pressure_name());
            ui.end_row();
        });
        ui.checkbox(&mut d.own_weight, "Count the members' own weight");
        ui.horizontal(|ui| {
            ui.checkbox(&mut d.trial, "Compare with every member at");
            ui.add_enabled(d.trial, egui::TextEdit::singleline(&mut d.trial_width).desired_width(44.0).hint_text("wide"));
            ui.label("by");
            ui.add_enabled(d.trial, egui::TextEdit::singleline(&mut d.trial_depth).desired_width(44.0).hint_text("deep"));
            ui.label(d.length_name());
        });
        ui.horizontal(|ui| {
            ui.label("Draw the sag");
            ui.add(egui::Slider::new(&mut d.scale, 1.0..=50.0).fixed_decimals(0).suffix("\u{d7}"));
            if d.trial {
                ui.selectable_value(&mut d.show_trial, false, "as built");
                ui.selectable_value(&mut d.show_trial, true, "trial");
            }
        });
        ui.separator();
        match &now {
            Err(e) => { ui.colored_label(colors.error, e); }
            Ok(result) => {
                let other = trial.as_ref().and_then(|t| t.as_ref().ok());
                if let Some(Err(e)) = &trial { ui.colored_label(colors.error, format!("Trial: {e}")); }
                let length = d.length();
                egui::Grid::new("members").num_columns(8).striped(true).show(ui, |ui| {
                    for head in ["Member", "Section", "Carries", "Bending", "Shear", "Sag", "Span / sag"] { ui.label(RichText::new(head).color(colors.muted)); }
                    if other.is_some() {
                        ui.label(RichText::new("Trial bending").color(colors.muted));
                        ui.label(RichText::new("Trial sag").color(colors.muted));
                    }
                    ui.end_row();
                    for m in result["members"].as_array().into_iter().flatten() {
                        let Some(body) = m["body"].as_u64().map(|b| b as Id) else { continue };
                        let name = app.session.built.body(body).map_or(format!("Body {body}"), |b| b.name.clone());
                        if ui.selectable_label(app.sel_body == Some(body), name).clicked() { app.sel_body = Some(body); }
                        ui.label(format!("{} \u{d7} {}", tidy(m["section"]["width_mm"].as_f64().unwrap_or(0.0) / length), tidy(m["section"]["depth_mm"].as_f64().unwrap_or(0.0) / length)));
                        let auto = result["tributary_mm"][body.to_string()].as_f64().map_or("none".to_owned(), |w| tidy(w / length));
                        let typed = d.widths.entry(body).or_default();
                        ui.add(egui::TextEdit::singleline(typed).desired_width(48.0).hint_text(auto)).on_hover_text("The width of deck this member carries. Empty takes it from the spacing of the members beside it.");
                        let ratio = |ui: &mut egui::Ui, m: &J, key: &str| {
                            let v = m[key].as_f64().unwrap_or(0.0);
                            ui.label(RichText::new(format!("{v:.2}")).color(if v > 1.0 { colors.error } else { colors.ink }).strong());
                        };
                        let sag = |m: &J| m["sampled_max_displacement_mm"].as_f64().unwrap_or(0.0);
                        ratio(ui, m, "bending_utilization");
                        ratio(ui, m, "shear_utilization");
                        ui.label(sag_text(sag(m), d.us));
                        ui.label(if sag(m) > 1e-9 { format!("L / {:.0}", span(m) / sag(m)) } else { "\u{2014}".into() });
                        if let Some(t) = other.and_then(|t| find(t, body)) {
                            ratio(ui, t, "bending_utilization");
                            ui.label(sag_text(sag(t), d.us));
                        }
                        ui.end_row();
                    }
                });
                let worst = |r: &J| r["members"].as_array().into_iter().flatten().map(|m| m["bending_shear_utilization"].as_f64().unwrap_or(0.0)).fold(0.0, f64::max);
                let verdict = |ui: &mut egui::Ui, label: &str, v: f64| {
                    let text = if v > 1.0 { format!("{label}: the worst member is at {v:.2} of its bending or shear limit, over by {:.0}%.", (v - 1.0) * 100.0) } else { format!("{label}: every member is within its bending and shear limits; the worst is at {v:.2}.") };
                    ui.label(RichText::new(text).color(if v > 1.0 { colors.error } else { colors.good }));
                };
                verdict(ui, "As built", worst(result));
                if let Some(t) = other { verdict(ui, "Trial", worst(t)); }
                for skipped in result["skipped"].as_array().into_iter().flatten() {
                    let body = skipped["body"].as_u64().unwrap_or(0) as Id;
                    let name = app.session.built.body(body).map_or(format!("Body {body}"), |b| b.name.clone());
                    ui.colored_label(colors.muted, format!("{name} is left out: {}", skipped["error"].as_str().unwrap_or_default()));
                }
            }
        }
        ui.label(RichText::new("A ratio above 1 is past the allowable stress you entered. Each member is a simple span on its own: no sharing through the deck, no buckling, bearing or connection checks. An estimate, not a design check.").color(colors.muted).small());
        ui.collapsing("Lumber", |ui| {
            let (stress, length) = (d.stress_name(), d.length_name());
            let density = if d.us { "lb/ft\u{b3}" } else { "kg/m\u{b3}" };
            egui::Grid::new("lumber").num_columns(3).show(ui, |ui| {
                let l = &mut d.lumber;
                for (label, text, unit, tip) in [
                    ("Species", &mut l.species, "", "As on the grade stamp"),
                    ("Grade", &mut l.grade, "", "As on the grade stamp"),
                    ("E", &mut l.e, stress, "Modulus of elasticity"),
                    ("Fb", &mut l.fb, stress, "Allowable bending stress, before the repetitive-member factor"),
                    ("Fv", &mut l.fv, stress, "Allowable shear stress"),
                    ("Density", &mut l.density, density, "Needed to count the members' own weight"),
                    ("Bearing", &mut l.bearing, length, "How far each end rests on its support. Supports are put at the middle of it."),
                ] {
                    ui.label(label).on_hover_text(tip);
                    ui.add(egui::TextEdit::singleline(text).desired_width(150.0));
                    ui.label(unit);
                    ui.end_row();
                }
            });
            ui.checkbox(&mut d.lumber.repetitive, "Joists sharing a deck (bending \u{d7} 1.15)");
            ui.horizontal(|ui| {
                let tagged: Vec<Id> = app.doc().structural_members.keys().copied().collect();
                let chosen = app.sel_body;
                let apply = |app: &mut App, d: &LoadsDlg, bodies: &[Id]| match d.tag() {
                    Err(e) => app.toast(e),
                    Ok(tag) => for body in bodies {
                        if let Err(e) = app.execute(&json!({"op": "set_structural_member", "body": body, "member": tag})) { app.toast(e); break; }
                    },
                };
                if ui.add_enabled(chosen.is_some(), egui::Button::new("Tag selected body")).on_hover_text("Makes the selected body a member of this lumber. It must be a plain rectangular extrusion along its length.").clicked() {
                    apply(app, &d, &chosen.into_iter().collect::<Vec<_>>());
                }
                if ui.add_enabled(!tagged.is_empty(), egui::Button::new("Use for all members")).clicked() { apply(app, &d, &tagged); }
                if ui.add_enabled(chosen.is_some_and(|b| tagged.contains(&b)), egui::Button::new("Untag selected")).clicked()
                    && let Some(body) = chosen
                    && let Err(e) = app.execute(&json!({"op": "remove_structural_member", "body": body}))
                {
                    app.toast(e);
                }
            });
        });
        ui.horizontal(|ui| {
            if ui.button("Done").on_hover_text("Closes the panel and keeps this load with the design").clicked() { done = true; }
        });
    });
    if done {
        // The case goes with the design, so the panel opens on it next time.
        if let Ok(case) = d.case(d.trial) {
            let _ = app.execute(&json!({"op": "estimate_structure", "case": case, "remember": true}));
        }
        app.dialog = Dialog::None;
        app.loads = None;
    } else {
        app.dialog = Dialog::Loads(d);
    }
}

/// Draws each member bent as the estimate has it, coloured by how near its limit it is,
/// with the load pressing on it.
pub fn overlay(app: &mut App, painter: &Painter, d: &LoadsDlg) {
    let colors = Palette::from_ctx(painter.ctx());
    let (now, trial) = estimates(app, d);
    let shown = match (d.show_trial && d.trial, trial) {
        (true, Some(Ok(t))) => t,
        _ => match now { Ok(r) => r, Err(_) => return },
    };
    for m in shown["members"].as_array().into_iter().flatten() {
        let depth = m["section"]["depth_mm"].as_f64().unwrap_or(0.0);
        // The side of the section that stands in the frame: the one that is not level.
        let axes = [point(&m["geometry"]["section_axes"][0]), point(&m["geometry"]["section_axes"][1])];
        let up = if axes[0].z.abs() > axes[1].z.abs() { axes[0] } else { axes[1] } * 0.5 * depth;
        let up = if up.z < 0.0 { -up } else { up };
        let samples: Vec<(DVec3, DVec3)> = m["samples"].as_array().into_iter().flatten().map(|s| (point(&s["position_mm"]), point(&s["deformed_mm"]))).collect();
        if samples.len() < 2 { continue; }
        let color = rgb(m);
        let fill = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 200);
        for pair in samples.windows(2) {
            let quad = [pair[0].1 + up, pair[1].1 + up, pair[1].1 - up, pair[0].1 - up].map(|p| crate::view::to_screen(app, p));
            painter.add(Shape::convex_polygon(quad.to_vec(), fill, Stroke::NONE));
        }
        let edge = |side: f64| samples.iter().map(|s| crate::view::to_screen(app, s.1 + up * side)).collect::<Vec<_>>();
        painter.add(Shape::line(edge(1.0), Stroke::new(1.5, colors.ink)));
        painter.add(Shape::line(edge(-1.0), Stroke::new(1.5, colors.ink)));
        // The load, where there is one: arrows onto the member as it lies unloaded.
        if m["uniform_load_n_per_mm"][1].as_f64().unwrap_or(0.0) < -1e-9 {
            let arrow = Stroke::new(1.5, colors.accent);
            for i in (0..samples.len()).step_by((samples.len() / 6).max(1)) {
                let tip = crate::view::to_screen(app, samples[i].0 + up);
                painter.line_segment([tip - vec2(0.0, 20.0), tip], arrow);
                painter.line_segment([tip - vec2(3.5, 6.0), tip], arrow);
                painter.line_segment([tip - vec2(-3.5, 6.0), tip], arrow);
            }
        }
        let middle = &samples[samples.len() / 2];
        let sag = m["sampled_max_displacement_mm"].as_f64().unwrap_or(0.0);
        let text = format!("{:.2}  \u{b7}  {}", m["bending_shear_utilization"].as_f64().unwrap_or(0.0), sag_text(sag, d.us));
        let at = crate::view::to_screen(app, middle.1 - up) + vec2(0.0, 12.0);
        let galley = painter.layout_no_wrap(text, egui::FontId::proportional(12.0), colors.ink);
        let rect = egui::Rect::from_center_size(at, galley.size() + vec2(8.0, 4.0));
        painter.rect_filled(rect, 3.0, colors.panel);
        painter.galley(rect.min + vec2(4.0, 2.0), galley, colors.ink);
    }
}
