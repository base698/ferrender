//! The sketch reference-image editor and its viewport overlay.
//! The editor returns one change for the app to apply as one undo step.

use egui::{Color32, Context, Painter, Pos2, Rect, Shape, Stroke, TextureHandle, TextureOptions, pos2, vec2};
use fr_core::reference::ReferenceImage;
use fr_core::render::Camera;
use fr_core::{Document, Id, Kind, Plane};
use glam::DVec2;

pub struct Change {
    pub sketch: Id,
    pub image: Option<ReferenceImage>,
}

#[derive(Default)]
pub struct Editor {
    sketch: Option<Id>,
    open: bool,
    image: Option<ReferenceImage>,
    x: String,
    y: String,
    width: String,
    rotation: String,
    distance: String,
    calibration: Option<Vec<DVec2>>,
    error: Option<String>,
}

impl Editor {
    pub fn open(sketch: Id, image: Option<&ReferenceImage>) -> Self {
        let mut editor = Self { sketch: Some(sketch), open: true, image: image.cloned(), distance: "30 mm".into(), ..Self::default() };
        editor.fill_fields();
        editor
    }

    fn fill_fields(&mut self) {
        let (origin, width, rotation) = self.image.as_ref().map(|i| (i.origin, i.width, i.rotation)).unwrap_or((DVec2::ZERO, 100.0, 0.0));
        // Explicit units preserve exact values and make changing document units harmless.
        self.x = format!("{} mm", origin.x);
        self.y = format!("{} mm", origin.y);
        self.width = format!("{width} mm");
        self.rotation = format!("{rotation} deg");
    }

    pub fn sketch_id(&self) -> Option<Id> { self.sketch }
    pub fn cancel(&mut self) { *self = Self::default(); }
    pub fn is_calibrating(&self) -> bool { self.calibration.as_ref().is_some_and(|p| p.len() < 2) }
    pub fn is_open(&self) -> bool { self.open }
    /// Show the draft while picking calibration points, before its single undoable commit.
    pub fn calibration_image(&self) -> Option<&ReferenceImage> {
        self.calibration.as_ref().and(self.image.as_ref())
    }

    pub fn calibration_hint(&self) -> Option<&'static str> {
        match self.calibration.as_ref().map(Vec::len) {
            Some(0) => Some("Click the first reference-image point. Escape cancels calibration."),
            Some(1) => Some("Click the second reference-image point. Escape cancels calibration."),
            _ => None,
        }
    }

    pub fn calibration_click(&mut self, point: DVec2) {
        let Some(points) = &mut self.calibration else { return };
        if points.len() >= 2 || !point.is_finite() { return; }
        if points.first().is_some_and(|p| p.distance(point) < 1e-9) {
            self.error = Some("Choose two different points.".into());
            return;
        }
        points.push(point);
        self.error = None;
        if points.len() == 2 { self.open = true; }
    }

    fn placement(&self, doc: &Document) -> Result<ReferenceImage, String> {
        let mut image = self.image.clone().ok_or("Choose a PNG or JPEG image first.")?;
        image.origin = DVec2::new(doc.eval(&self.x, Kind::Length)?, doc.eval(&self.y, Kind::Length)?);
        image.width = doc.eval(&self.width, Kind::Length)?;
        image.rotation = doc.eval(&self.rotation, Kind::Angle)?;
        image.validate()?;
        Ok(image)
    }

    fn result_image(&self, doc: &Document) -> Result<ReferenceImage, String> {
        let mut image = self.placement(doc)?;
        if let Some(points) = &self.calibration {
            if points.len() != 2 { return Err("Choose two image points before applying calibration.".into()); }
            image.calibrate(points[0], points[1], doc.eval(&self.distance, Kind::Length)?)?;
        }
        Ok(image)
    }

    pub fn show(&mut self, ctx: &Context, doc: &Document) -> Option<Change> {
        if !self.open { return None; }
        let mut open = true;
        let mut apply = false;
        let mut remove = false;
        let mut calibrate = false;
        let mut cancel = false;
        let measured = self.calibration.as_ref().is_some_and(|p| p.len() == 2);
        egui::Window::new("Reference Image").id(egui::Id::new("reference-image-editor"))
            .open(&mut open).collapsible(false).resizable(false).default_width(345.0).show(ctx, |ui| {
                ui.label("A tracing guide embedded in this sketch. It is saved in the design and does not become part of the solid.");
                ui.add_enabled_ui(!measured, |ui| {
                    if ui.button(if self.image.is_some() { "Replace Image…" } else { "Choose Image…" }).clicked()
                        && let Some(path) = rfd::FileDialog::new().add_filter("PNG or JPEG", &["png", "jpg", "jpeg"]).pick_file() {
                        let width = doc.eval(&self.width, Kind::Length).unwrap_or(100.0);
                        match ReferenceImage::from_file(&path, width) {
                            Ok(mut image) => {
                                if let Some(old) = &self.image {
                                    image.origin = old.origin;
                                    image.rotation = old.rotation;
                                    image.opacity = old.opacity;
                                    image.visible = old.visible;
                                }
                                self.image = Some(image);
                                self.fill_fields();
                                self.error = None;
                            }
                            Err(error) => self.error = Some(error),
                        }
                    }
                    ui.small("PNG / JPEG · up to 8 MiB and 4 megapixels");
                    if let Some(image) = &mut self.image {
                        ui.label(format!("{} · {} × {} pixels", image.name, image.pixel_width, image.pixel_height));
                        egui::Grid::new("reference-image-placement").num_columns(2).show(ui, |ui| {
                            for (label, text) in [("Origin X", &mut self.x), ("Origin Y", &mut self.y), ("Width", &mut self.width), ("Rotation", &mut self.rotation)] {
                                ui.label(label);
                                ui.add(egui::TextEdit::singleline(text).desired_width(190.0).hint_text("Value or expression"));
                                ui.end_row();
                            }
                        });
                        ui.small("Origin is the lower-left corner in sketch coordinates. Width keeps the image proportions.");
                        ui.add(egui::Slider::new(&mut image.opacity, 0.0..=1.0).text("Opacity"));
                        ui.checkbox(&mut image.visible, "Visible");
                    }
                });
                ui.separator();
                if measured {
                    ui.label("Enter the real distance between the two selected points. The first point will stay in place.");
                    ui.horizontal(|ui| {
                        ui.label("Known distance");
                        ui.add(egui::TextEdit::singleline(&mut self.distance).desired_width(170.0));
                    });
                } else if self.image.is_some() {
                    calibrate = ui.button("Calibrate from Two Points…").clicked();
                }
                if let Some(error) = &self.error { ui.colored_label(crate::theme::Palette::from_ctx(ui.ctx()).error, error); }
                ui.horizontal(|ui| {
                    apply = ui.add_enabled(self.image.is_some(), egui::Button::new(if measured { "Calibrate and Apply" } else { "Apply" })).clicked();
                    cancel = ui.button("Cancel").clicked();
                    if self.image.is_some() { remove = ui.button("Remove Image").clicked(); }
                });
            });
        if cancel || !open { self.cancel(); return None; }
        if remove {
            let change = Change { sketch: self.sketch?, image: None };
            self.cancel();
            return Some(change);
        }
        if calibrate {
            match self.placement(doc) {
                Ok(mut image) => {
                    image.visible = true;
                    self.image = Some(image);
                    self.fill_fields();
                    self.calibration = Some(Vec::with_capacity(2));
                    self.open = false;
                    self.error = None;
                }
                Err(error) => self.error = Some(error),
            }
        }
        if apply {
            match self.result_image(doc) {
                Ok(image) => {
                    let change = Change { sketch: self.sketch?, image: Some(image) };
                    self.cancel();
                    return Some(change);
                }
                Err(error) => self.error = Some(error),
            }
        }
        None
    }

    pub fn paint_calibration(&self, painter: &Painter, plane: Plane, camera: Camera, rect: Rect, cursor: Option<DVec2>) {
        let Some(points) = &self.calibration else { return };
        let color = crate::theme::choose(painter.ctx(), Color32::from_rgb(210, 55, 30), Color32::from_rgb(255, 157, 115));
        let halo = crate::theme::Palette::from_ctx(painter.ctx()).panel;
        let screen = |p| project(plane, camera, rect, p);
        for (index, point) in points.iter().enumerate() {
            let p = screen(*point);
            // Two-tone targets remain visible on both the viewport and arbitrary image pixels.
            painter.circle_stroke(p, 6.0, Stroke::new(4.0, halo));
            painter.circle_stroke(p, 6.0, Stroke::new(2.0, color));
            let text = painter.layout_no_wrap(format!("{}", index + 1), egui::FontId::proportional(14.0), color);
            let label = Rect::from_min_size(p + vec2(9.0, -11.0 - text.size().y), text.size() + vec2(4.0, 4.0));
            painter.rect_filled(label, 2.0, halo);
            painter.galley(label.min + vec2(2.0, 2.0), text, color);
        }
        if let Some(first) = points.first()
            && let Some(second) = points.get(1).copied().or(cursor) {
            painter.line_segment([screen(*first), screen(second)], Stroke::new(3.5, halo));
            painter.line_segment([screen(*first), screen(second)], Stroke::new(1.5, color));
        }
    }
}

fn project(plane: Plane, camera: Camera, rect: Rect, point: DVec2) -> Pos2 {
    let p = camera.project(plane.to_world(point)).0;
    rect.center() + vec2(p.x as f32, p.y as f32)
}

#[derive(Default)]
pub struct TextureCache {
    current: Option<(ReferenceImage, TextureHandle)>,
}

impl TextureCache {
    pub fn clear(&mut self) { self.current = None; }

    /// Paint before sketch/grid geometry. Display only the active sketch's reference image.
    pub fn paint(&mut self, painter: &Painter, image: &ReferenceImage, plane: Plane, camera: Camera, rect: Rect) -> Result<(), String> {
        if !image.visible { return Ok(()); }
        if self.current.as_ref().is_none_or(|(old, _)| !old.same_pixels(image)) {
            let pixels = image.pixels()?;
            let texture = painter.ctx().load_texture("sketch-reference-image", egui::ColorImage::from_rgba_unmultiplied([pixels.width as usize, pixels.height as usize], &pixels.rgba), TextureOptions::LINEAR);
            self.current = Some((image.clone(), texture));
        }
        let texture = &self.current.as_ref().unwrap().1;
        let tint = Color32::from_white_alpha((image.opacity.clamp(0.0, 1.0) * 255.0).round() as u8);
        let mut mesh = egui::Mesh::with_texture(texture.id());
        // Geometry starts at bottom-left, while an image's row zero is at its top.
        let uv = [pos2(0.0, 1.0), pos2(1.0, 1.0), pos2(1.0, 0.0), pos2(0.0, 0.0)];
        for (point, uv) in image.corners().into_iter().zip(uv) {
            mesh.vertices.push(egui::epaint::Vertex { pos: project(plane, camera, rect, point), uv, color: tint });
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(Shape::mesh(mesh));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> ReferenceImage {
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(2, 1, image::Rgba([255, 0, 0, 255])).write_to(&mut png, image::ImageFormat::Png).unwrap();
        ReferenceImage::from_bytes("pixel.png", png.get_ref(), 30.0).unwrap()
    }

    #[test]
    fn editor_calibrates_pending_change_and_cancel_has_no_document_side_effects() {
        let image = image();
        let mut editor = Editor::open(42, Some(&image));
        editor.calibration = Some(Vec::new());
        editor.open = false;
        editor.calibration_click(DVec2::new(5.0, 5.0));
        editor.calibration_click(DVec2::new(5.0, 5.0));
        assert!(editor.is_calibrating());
        editor.calibration_click(DVec2::new(10.0, 5.0));
        assert!(!editor.is_calibrating());
        assert!(editor.is_open());
        editor.distance = "10 mm".into();
        let result = editor.result_image(&Document::default()).unwrap();
        assert_eq!(result.width, 60.0);
        assert_eq!(result.origin, DVec2::new(-5.0, -5.0));
        assert_eq!(image.width, 30.0);
        editor.cancel();
        assert_eq!(editor.sketch_id(), None);
    }

    #[test]
    fn projected_overlay_has_correct_top_left_uv_and_alpha() {
        let mut image = image();
        image.origin = DVec2::new(2.0, 3.0);
        image.opacity = 0.25;
        let context = Context::default();
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(300.0, 300.0));
        let mut cache = TextureCache::default();
        let plane = Plane::XY;
        let (yaw, pitch) = Camera::facing(&plane);
        let camera = Camera { yaw, pitch, scale: 2.0, ..Camera::default() };
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            let painter = ui.ctx().layer_painter(egui::LayerId::background());
            cache.paint(&painter, &image, plane, camera, rect).unwrap();
        });
        let mesh = output.shapes.iter().find_map(|s| if let Shape::Mesh(m) = &s.shape { Some(m) } else { None }).unwrap();
        assert_eq!(mesh.vertices[3].uv, pos2(0.0, 0.0));
        assert_eq!(mesh.vertices[0].uv, pos2(0.0, 1.0));
        assert!(mesh.vertices[3].pos.y < mesh.vertices[0].pos.y);
        assert_eq!(mesh.vertices[0].color.a(), 64);
        assert_eq!(mesh.indices, [0, 1, 2, 0, 2, 3]);
        output.textures_delta.clear();
        cache.clear();
    }
}
