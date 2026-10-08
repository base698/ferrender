//! Select-tool reference-image placement. Pointer movement only changes a cheap
//! preview; `finish` returns at most one undoable document change.

use egui::{CursorIcon, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, vec2};
use fr_core::{Id, Plane, reference::ReferenceImage, render::Camera};
use glam::DVec2;

use crate::reference::{Change, project};

const HANDLE_RADIUS: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    Move,
    /// Bottom-left, bottom-right, top-right, top-left, as in `ReferenceImage::corners`.
    Corner(usize),
}

impl Handle {
    pub fn cursor(self) -> CursorIcon {
        match self {
            Self::Move => CursorIcon::Grab,
            // A direction-independent cursor remains accurate for rotated images.
            Self::Corner(_) => CursorIcon::Move,
        }
    }
}

struct Drag {
    original: ReferenceImage,
    preview: ReferenceImage,
    start: DVec2,
    handle: Handle,
    error: Option<String>,
}

#[derive(Default)]
pub struct State {
    sketch: Option<Id>,
    drag: Option<Drag>,
}

impl State {
    pub fn selected(&self, sketch: Id) -> bool { self.sketch == Some(sketch) }
    pub fn select(&mut self, sketch: Id) { self.sketch = Some(sketch); self.drag = None; }
    pub fn is_dragging(&self) -> bool { self.drag.is_some() }
    pub fn clear(&mut self) { *self = Self::default(); }
    pub fn cancel_drag(&mut self) { self.drag = None; }

    pub fn preview(&self, sketch: Id) -> Option<&ReferenceImage> {
        self.selected(sketch).then_some(self.drag.as_ref()).flatten().map(|d| &d.preview)
    }

    pub fn hint(&self, sketch: Id) -> Option<&'static str> {
        if !self.selected(sketch) { return None; }
        Some(match self.drag.as_ref().map(|d| d.handle) {
            Some(Handle::Move) => "Move the reference image. Release to apply; Escape cancels.",
            Some(Handle::Corner(_)) => "Scale the reference image around its opposite corner. Release to apply; Escape cancels.",
            None => "Reference image selected. Drag inside to move, or drag a corner to scale. Escape clears selection.",
        })
    }

    /// Selected handles take priority. Set `allow_body` to false when a sketch
    /// point, edge, or dimension is under the pointer, so tracing remains usable.
    pub fn hit(&self, sketch: Id, image: &ReferenceImage, plane: Plane, camera: Camera, rect: Rect, pos: Pos2, allow_body: bool) -> Option<Handle> {
        if !image.visible || !pos.is_finite() { return None; }
        let corners = screen_corners(image, plane, camera, rect)?;
        if self.selected(sketch)
            && let Some((index, _)) = corners.iter().enumerate()
                .map(|(i, p)| (i, p.distance(pos)))
                .filter(|(_, distance)| *distance <= HANDLE_RADIUS)
                .min_by(|a, b| a.1.total_cmp(&b.1)) {
            return Some(Handle::Corner(index));
        }
        (allow_body && inside_quad(corners, pos)).then_some(Handle::Move)
    }

    pub fn begin(&mut self, sketch: Id, image: &ReferenceImage, handle: Handle, at: DVec2) -> Result<(), String> {
        if !image.visible { return Err("Show the reference image before moving it.".into()); }
        if !at.is_finite() { return Err("The pointer cannot be projected onto the sketch plane.".into()); }
        if matches!(handle, Handle::Corner(index) if index >= 4) {
            return Err("Choose a reference-image corner to scale it.".into());
        }
        image.validate()?;
        self.sketch = Some(sketch);
        self.drag = Some(Drag { original: image.clone(), preview: image.clone(), start: at, handle, error: None });
        Ok(())
    }

    /// Always calculate from the original placement, avoiding rounding drift.
    /// Pixel data and its decode cache are shared by all these preview clones.
    pub fn update(&mut self, at: DVec2) -> Result<(), String> {
        let Some(drag) = &mut self.drag else { return Ok(()); };
        match placement(&drag.original, drag.handle, drag.start, at) {
            Ok(preview) => { drag.preview = preview; drag.error = None; Ok(()) }
            Err(error) => { drag.error = Some(error.clone()); Err(error) }
        }
    }

    /// An invalid final pointer position rejects the entire gesture rather than
    /// silently committing the most recent valid preview. Selection is retained.
    pub fn finish(&mut self) -> Result<Option<Change>, String> {
        let Some(drag) = self.drag.take() else { return Ok(None); };
        if let Some(error) = drag.error { return Err(error); }
        if drag.preview == drag.original { return Ok(None); }
        let Some(sketch) = self.sketch else { return Ok(None); };
        Ok(Some(Change { sketch, image: Some(drag.preview) }))
    }

    pub fn paint(&self, painter: &Painter, sketch: Id, image: &ReferenceImage, plane: Plane, camera: Camera, rect: Rect) {
        if !self.selected(sketch) || !image.visible { return; }
        let Some(corners) = screen_corners(image, plane, camera, rect) else { return; };
        let colors = crate::theme::Palette::from_ctx(painter.ctx());
        let outline = [corners[0], corners[1], corners[2], corners[3], corners[0]];
        painter.add(Shape::line(outline.to_vec(), Stroke::new(4.0, colors.panel)));
        painter.add(Shape::line(outline.to_vec(), Stroke::new(1.5, colors.accent)));
        for corner in corners {
            let handle = Rect::from_center_size(corner, vec2(9.0, 9.0));
            painter.rect_filled(handle, 1.0, colors.panel);
            painter.rect_stroke(handle, 1.0, Stroke::new(2.0, colors.accent), StrokeKind::Middle);
        }
    }
}

fn placement(image: &ReferenceImage, handle: Handle, start: DVec2, at: DVec2) -> Result<ReferenceImage, String> {
    if !start.is_finite() || !at.is_finite() {
        return Err("The pointer cannot be projected onto the sketch plane.".into());
    }
    if at == start { return Ok(image.clone()); }
    let mut candidate = image.clone();
    match handle {
        Handle::Move => candidate.origin += at - start,
        Handle::Corner(index) => {
            let corners = image.corners();
            let corner = *corners.get(index).ok_or("Choose a reference-image corner to scale it.")?;
            let anchor = corners[(index + 2) % 4];
            let diagonal = corner - anchor;
            // Retain the small offset between the pointer press and the handle
            // centre, so grabbing the edge of a handle does not jump the image.
            let scale = (at + corner - start - anchor).dot(diagonal) / diagonal.length_squared();
            if !scale.is_finite() || scale <= 0.0 {
                return Err("Keep the image corner on its original side of the opposite corner to give it a positive size.".into());
            }
            candidate.width *= scale;
            candidate.origin = anchor + (image.origin - anchor) * scale;
        }
    }
    // validate() reuses the image's existing decode cache; pointer movement never
    // reloads an image or alters the document/model.
    candidate.validate()?;
    Ok(candidate)
}

fn screen_corners(image: &ReferenceImage, plane: Plane, camera: Camera, rect: Rect) -> Option<[Pos2; 4]> {
    if !camera.scale.is_finite() || camera.scale <= 0.0 { return None; }
    let corners = image.corners().map(|p| project(plane, camera, rect, p));
    if corners.iter().any(|p| !p.is_finite()) { return None; }
    let a = corners[1] - corners[0];
    let b = corners[3] - corners[0];
    // An edge-on sketch does not define a usable image picking region.
    ((a.x * b.y - a.y * b.x).abs() > 1e-4).then_some(corners)
}

fn inside_quad(corners: [Pos2; 4], p: Pos2) -> bool {
    let mut positive = false;
    let mut negative = false;
    for index in 0..4 {
        let edge = corners[(index + 1) % 4] - corners[index];
        let offset = p - corners[index];
        let cross = edge.x * offset.y - edge.y * offset.x;
        positive |= cross > 1e-4;
        negative |= cross < -1e-4;
    }
    !(positive && negative)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, pos2};

    fn image() -> ReferenceImage {
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(4, 2, image::Rgba([255, 0, 0, 255])).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let mut image = ReferenceImage::from_bytes("pixel.png", png.get_ref(), 30.0).unwrap();
        image.origin = DVec2::new(-8.0, 3.0);
        image.rotation = 31.0;
        image
    }

    fn close(a: DVec2, b: DVec2) { assert!(a.distance(b) < 1e-9, "{a:?} != {b:?}"); }

    #[test]
    fn move_is_preview_only_shares_pixels_and_emits_one_change() {
        let image = image();
        let mut state = State::default();
        let original_origin = image.origin;
        state.begin(42, &image, Handle::Move, DVec2::new(4.0, 5.0)).unwrap();
        for n in 0..30 { state.update(DVec2::new(4.0 + n as f64, 7.0)).unwrap(); }
        assert_eq!(image.origin, original_origin);
        let preview = state.preview(42).unwrap();
        close(preview.origin, original_origin + DVec2::new(29.0, 2.0));
        assert!(preview.same_pixels(&image));
        assert!(std::sync::Arc::ptr_eq(&preview.pixels().unwrap(), &image.pixels().unwrap()));
        assert!(state.preview(43).is_none());
        let change = state.finish().unwrap().unwrap();
        assert_eq!(change.sketch, 42);
        assert_eq!(change.image.unwrap().width, image.width);
        assert!(state.finish().unwrap().is_none());
        assert!(state.selected(42));
        assert!(!state.is_dragging());
    }

    #[test]
    fn every_rotated_corner_scales_uniformly_around_its_opposite_corner() {
        for rotation in [0.0, 31.0, 90.0, -145.0] {
            let mut image = image();
            image.rotation = rotation;
            let original = image.corners();
            for index in 0..4 {
                let anchor = original[(index + 2) % 4];
                let offset = DVec2::new(0.2, -0.3);
                let start = original[index] + offset;
                let at = anchor + (original[index] - anchor) * 1.75 + offset;
                let mut state = State::default();
                state.begin(42, &image, Handle::Corner(index), start).unwrap();
                state.update(start).unwrap();
                close(state.preview(42).unwrap().origin, image.origin);
                state.update(at).unwrap();
                let changed = state.finish().unwrap().unwrap().image.unwrap();
                assert!((changed.width - image.width * 1.75).abs() < 1e-9);
                assert_eq!(changed.rotation, rotation);
                assert_eq!(changed.height() / changed.width, image.height() / image.width);
                close(changed.corners()[(index + 2) % 4], anchor);
                for corner in 0..4 { close(changed.corners()[corner], anchor + (original[corner] - anchor) * 1.75); }
            }
        }
    }

    #[test]
    fn invalid_drag_cannot_commit_and_can_be_corrected_before_release() {
        let image = image();
        let mut state = State::default();
        for at in [DVec2::splat(f64::NAN), DVec2::splat(f64::INFINITY), DVec2::splat(2e9)] {
            state.begin(42, &image, Handle::Move, DVec2::ZERO).unwrap();
            assert!(state.update(at).is_err());
            assert!(state.finish().is_err());
        }
        let corners = image.corners();
        state.begin(42, &image, Handle::Corner(0), corners[0]).unwrap();
        assert!(state.update(corners[2]).is_err());
        assert!(state.update(corners[2] * 2.0 - corners[0]).is_err());
        state.update(corners[0] + (corners[0] - corners[2])).unwrap();
        let changed = state.finish().unwrap().unwrap().image.unwrap();
        assert!((changed.width - image.width * 2.0).abs() < 1e-9);
    }

    #[test]
    fn cancellation_and_click_without_moving_have_no_change() {
        let image = image();
        let mut state = State::default();
        state.begin(42, &image, Handle::Move, DVec2::ZERO).unwrap();
        assert!(state.finish().unwrap().is_none());
        state.begin(42, &image, Handle::Move, DVec2::ZERO).unwrap();
        state.update(DVec2::new(1.0, 2.0)).unwrap();
        state.cancel_drag();
        assert!(state.finish().unwrap().is_none());
        assert!(state.preview(42).is_none());
        assert!(state.selected(42));
        state.clear();
        assert!(!state.selected(42));
    }

    #[test]
    fn hit_testing_respects_geometry_hidden_images_rotation_and_sketch_plane() {
        let mut image = image();
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(600.0, 400.0));
        for plane in [Plane::XY, Plane::XZ, Plane::YZ] {
            let (yaw, pitch) = Camera::facing(&plane);
            let camera = Camera { yaw, pitch, scale: 4.0, ..Camera::default() };
            let corners = screen_corners(&image, plane, camera, rect).unwrap();
            let center = corners[0].lerp(corners[2], 0.5);
            let mut state = State::default();
            assert_eq!(state.hit(42, &image, plane, camera, rect, center, true), Some(Handle::Move));
            assert_eq!(state.hit(42, &image, plane, camera, rect, center, false), None);
            assert_eq!(state.hit(42, &image, plane, camera, rect, pos2(-10000.0, -10000.0), true), None);
            state.begin(42, &image, Handle::Move, image.origin).unwrap();
            state.finish().unwrap();
            assert_eq!(state.hit(42, &image, plane, camera, rect, corners[2], false), Some(Handle::Corner(2)));
            assert_eq!(state.hit(43, &image, plane, camera, rect, corners[2], false), None);
            image.visible = false;
            assert_eq!(state.hit(42, &image, plane, camera, rect, center, true), None);
            assert_eq!(state.hit(42, &image, plane, camera, rect, corners[2], true), None);
            assert!(state.begin(42, &image, Handle::Move, image.origin).is_err());
            image.visible = true;
        }
        let camera = Camera { yaw: 0.0, pitch: 0.0, scale: 4.0, ..Camera::default() };
        assert!(screen_corners(&image, Plane::XY, camera, rect).is_none());
    }

    #[test]
    fn selected_overlay_draws_four_handles_without_texture_uploads() {
        let image = image();
        let mut state = State::default();
        state.begin(42, &image, Handle::Move, DVec2::ZERO).unwrap();
        let context = egui::Context::default();
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(600.0, 400.0));
        let (yaw, pitch) = Camera::facing(&Plane::XY);
        let camera = Camera { yaw, pitch, scale: 4.0, ..Camera::default() };
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            state.paint(ui.painter(), 42, &image, Plane::XY, camera, rect);
        });
        assert_eq!(output.shapes.iter().filter(|shape| matches!(shape.shape, Shape::Rect(_))).count(), 8);
        assert!(!output.shapes.iter().any(|shape| matches!(shape.shape, Shape::Mesh(_))));
        // This shape-only test intentionally does not have a texture renderer.
        output.textures_delta.clear();
    }
}
