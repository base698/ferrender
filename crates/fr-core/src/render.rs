//! The camera, and a small software renderer used for screenshots when
//! there is no window (the MCP server, the assistant and tests). The app
//! draws its viewport on the GPU instead.

use glam::{DVec2, DVec3};

use crate::doc::{Body, Session};
use crate::sketch::{Id, Plane};

/// An orthographic camera orbiting `target`. Z is up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub target: DVec3,
    /// Radians around Z; the eye sits at this bearing from the target.
    pub yaw: f64,
    /// Radians above the XY plane.
    pub pitch: f64,
    /// Pixels per millimetre.
    pub scale: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Camera::iso()
    }
}

impl Camera {
    pub fn iso() -> Camera {
        Camera { target: DVec3::ZERO, yaw: -std::f64::consts::FRAC_PI_4, pitch: 0.6155, scale: 4.0 }
    }

    /// Looks straight at a plane, with its x axis to the right.
    pub fn facing(plane: &Plane) -> (f64, f64) {
        let n = plane.normal();
        let yaw = if n.x.abs() < 1e-9 && n.y.abs() < 1e-9 { -std::f64::consts::FRAC_PI_2 } else { n.y.atan2(n.x) };
        (yaw, n.z.clamp(-1.0, 1.0).asin())
    }

    /// `top`, `front`, `right`, `back`, `left`, `bottom` or `iso`.
    pub fn named(view: &str) -> Option<(f64, f64)> {
        use std::f64::consts::{FRAC_PI_2 as Q, PI};
        Some(match view {
            "top" => (-Q, Q),
            "bottom" => (-Q, -Q),
            "front" => (-Q, 0.0),
            "back" => (Q, 0.0),
            "right" => (0.0, 0.0),
            "left" => (PI, 0.0),
            "iso" | "home" => (Camera::iso().yaw, Camera::iso().pitch),
            _ => return None,
        })
    }

    /// Unit vectors toward the eye, screen right and screen up.
    pub fn basis(&self) -> (DVec3, DVec3, DVec3) {
        let eye = DVec3::new(self.pitch.cos() * self.yaw.cos(), self.pitch.cos() * self.yaw.sin(), self.pitch.sin());
        let right = DVec3::new(-self.yaw.sin(), self.yaw.cos(), 0.0);
        (eye, right, eye.cross(right))
    }

    /// Screen position relative to the viewport centre (y down), and depth toward the eye.
    pub fn project(&self, p: DVec3) -> (DVec2, f64) {
        let (eye, right, up) = self.basis();
        let d = p - self.target;
        (DVec2::new(d.dot(right), -d.dot(up)) * self.scale, d.dot(eye))
    }

    /// The ray under a screen position relative to the viewport centre.
    pub fn ray(&self, s: DVec2) -> (DVec3, DVec3) {
        let (eye, right, up) = self.basis();
        (self.target + (right * s.x - up * s.y) / self.scale + eye * 1e5, -eye)
    }

    pub fn pan(&mut self, screen_delta: DVec2) {
        let (_, right, up) = self.basis();
        self.target -= (right * screen_delta.x - up * screen_delta.y) / self.scale;
    }

    pub fn orbit(&mut self, screen_delta: DVec2) {
        self.yaw -= screen_delta.x * 0.008;
        self.pitch = (self.pitch + screen_delta.y * 0.008).clamp(-std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);
    }

    /// Zooms by `factor`, keeping the point under `at` (relative to the centre) still.
    pub fn zoom(&mut self, factor: f64, at: DVec2) {
        let new = (self.scale * factor).clamp(1e-3, 1e5);
        let (_, right, up) = self.basis();
        self.target += (right * at.x - up * at.y) * (1.0 / self.scale - 1.0 / new);
        self.scale = new;
    }

    /// Centres and scales to show the box in a `w` by `h` viewport.
    pub fn fit(&mut self, lo: DVec3, hi: DVec3, w: f64, h: f64) {
        self.target = (lo + hi) / 2.0;
        let (mut sx, mut sy) = (1e-6f64, 1e-6f64);
        for i in 0..8 {
            let c = DVec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
            let (_, right, up) = self.basis();
            sx = sx.max((c - self.target).dot(right).abs());
            sy = sy.max((c - self.target).dot(up).abs());
        }
        self.scale = (w / 2.0 / sx).min(h / 2.0 / sy) * 0.8;
        if !self.scale.is_finite() || self.scale > 1e4 {
            self.scale = 4.0;
        }
    }
}

/// Bounds of everything visible: bodies and sketch geometry.
pub fn scene_bounds(s: &Session) -> Option<(DVec3, DVec3)> {
    let mut pts: Vec<DVec3> = Vec::new();
    for b in s.visible_bodies() {
        if let Some((lo, hi)) = b.mesh.bbox() {
            pts.extend([lo, hi]);
        }
    }
    for (f, sk) in s.doc.sketches().filter(|(f,sk)| sk.visible && s.built.component_visible(f.owner)) {
        let Some(plane)=s.built.sketch_plane(&s.doc,f.id) else {continue};
        if let Some((lo, hi)) = sk.bbox() {
            for c in [lo, hi, DVec2::new(lo.x, hi.y), DVec2::new(hi.x, lo.y)] {
                pts.push(plane.to_world(c));
            }
        }
    }
    for (id,p) in &s.built.planes {
        if matches!(s.doc.feature(*id).map(|f|&f.kind),Some(crate::FeatureKind::Plane(c)) if c.visible)
            && s.built.component_visible(p.component) {pts.extend(p.corners);}
    }
    let first = *pts.first()?;
    Some(pts.iter().fold((first, first), |(lo, hi), p| (lo.min(*p), hi.max(*p))))
}

pub struct Image {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn png(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, self.w as u32, self.h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().and_then(|mut w| w.write_image_data(&self.rgba)).expect("writing a PNG to memory cannot fail");
        out
    }

    fn blend(&mut self, x: usize, y: usize, c: [u8; 3], a: f64) {
        let i = (y * self.w + x) * 4;
        for k in 0..3 {
            self.rgba[i + k] = (self.rgba[i + k] as f64 * (1.0 - a) + c[k] as f64 * a) as u8;
        }
        self.rgba[i + 3] = 255;
    }

    /// An anti-aliased line between two pixel positions.
    pub fn line(&mut self, a: DVec2, b: DVec2, color: [u8; 3], width: f64) {
        let r = width / 2.0 + 1.0;
        let (x0, x1) = ((a.x.min(b.x) - r).floor().max(0.0) as usize, ((a.x.max(b.x) + r).ceil().min(self.w as f64 - 1.0)).max(0.0) as usize);
        let (y0, y1) = ((a.y.min(b.y) - r).floor().max(0.0) as usize, ((a.y.max(b.y) + r).ceil().min(self.h as f64 - 1.0)).max(0.0) as usize);
        if a.x.max(b.x) < -r || a.y.max(b.y) < -r || (x1 - x0) * (y1 - y0) > 64_000_000 {
            return;
        }
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d = crate::sketch::seg_dist(DVec2::new(x as f64 + 0.5, y as f64 + 0.5), a, b);
                let cover = (width / 2.0 + 0.5 - d).clamp(0.0, 1.0);
                if cover > 0.0 {
                    self.blend(x, y, color, cover);
                }
            }
        }
    }
}

pub const BODY: [u8; 3] = [176, 186, 200];
pub const BODY_SELECTED: [u8; 3] = [120, 170, 240];
pub const BACKGROUND: [u8; 3] = [246, 247, 249];
pub const EDGE: [u8; 3] = [38, 44, 54];
pub const SKETCH: [u8; 3] = [0, 104, 214];
pub const CONSTRUCTION: [u8; 3] = [222, 130, 30];

/// Brightness of a surface with unit normal `n` seen by `cam`.
pub fn shade(n: DVec3, cam: &Camera) -> f64 {
    let (eye, right, up) = cam.basis();
    let n = if n.dot(eye) < 0.0 { -n } else { n };
    0.5 + 0.5 * n.dot((eye + right * 0.35 + up * 0.55).normalize()).max(0.0)
}

/// Draws shaded bodies with outlined edges onto `img`.
pub fn draw_bodies<'a>(img: &mut Image, bodies: impl IntoIterator<Item = &'a Body>, cam: &Camera, selected: Option<Id>) {
    let (w, h) = (img.w, img.h);
    let half = DVec2::new(w as f64, h as f64) / 2.0;
    // Per pixel: depth and the index of the face in front.
    let mut depth = vec![f32::MIN; w * h];
    let mut face = vec![u32::MAX; w * h];
    // Normal, plane offset, body, and the exact face the triangle is part of (if known).
    let mut faces: Vec<(DVec3, f64, Id, Option<u32>)> = Vec::new();
    for b in bodies {
        let groups = b.mesh.groups();
        for (ti, t) in b.mesh.tris().enumerate() {
            let n = b.mesh.normal(ti);
            let p: Vec<(DVec2, f64)> = t.iter().map(|v| cam.project(*v)).map(|(s, d)| (s + half, d)).collect();
            let (a, bb, c) = (p[0].0, p[1].0, p[2].0);
            let area = (bb - a).perp_dot(c - a);
            if area.abs() < 1e-9 || n == DVec3::ZERO {
                continue;
            }
            let (x0, x1) = (a.x.min(bb.x).min(c.x).floor().max(0.0) as usize, (a.x.max(bb.x).max(c.x).ceil().min(w as f64 - 1.0)) as isize);
            let (y0, y1) = (a.y.min(bb.y).min(c.y).floor().max(0.0) as usize, (a.y.max(bb.y).max(c.y).ceil().min(h as f64 - 1.0)) as isize);
            let id = faces.len() as u32;
            faces.push((n, n.dot(t[0]), b.id, groups.as_ref().map(|g| g[ti])));
            for y in y0 as isize..=y1 {
                for x in x0 as isize..=x1 {
                    let q = DVec2::new(x as f64 + 0.5, y as f64 + 0.5);
                    let w0 = (bb - q).perp_dot(c - q) / area;
                    let w1 = (c - q).perp_dot(a - q) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < -1e-9 || w1 < -1e-9 || w2 < -1e-9 {
                        continue;
                    }
                    let z = (w0 * p[0].1 + w1 * p[1].1 + w2 * p[2].1) as f32;
                    let i = y as usize * w + x as usize;
                    if z > depth[i] {
                        depth[i] = z;
                        face[i] = id;
                    }
                }
            }
        }
    }
    // Two faces meet at a visible edge when they turn sharply, step apart or belong to different bodies.
    let differ = |a: u32, b: u32| {
        if a == b {
            return false;
        }
        if a == u32::MAX || b == u32::MAX {
            return true;
        }
        let (fa, fb) = (faces[a as usize], faces[b as usize]);
        if fa.2 != fb.2 {
            return true;
        }
        // Exact bodies know their faces; meshes are judged by how sharply they turn.
        if let (Some(ga), Some(gb)) = (fa.3, fb.3) {
            return ga != gb;
        }
        let dot = fa.0.dot(fb.0);
        dot < 0.94 || (dot > 0.9999 && (fa.1 - fb.1).abs() > 0.02)
    };
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let edge = (x + 1 < w && differ(face[i], face[i + 1])) || (y + 1 < h && differ(face[i], face[i + w]));
            if face[i] != u32::MAX {
                let f = faces[face[i] as usize];
                let base = if selected == Some(f.2) { BODY_SELECTED } else { BODY };
                let s = shade(f.0, cam);
                img.blend(x, y, base.map(|c| (c as f64 * s) as u8), 1.0);
            }
            if edge {
                img.blend(x, y, EDGE, 0.85);
            }
        }
    }
}

/// A picture of the whole scene: bodies, visible sketches and the origin axes.
/// With no camera, the scene is framed from the home view.
pub fn snapshot(s: &Session, cam: Option<Camera>, w: usize, h: usize) -> Image {
    let cam = cam.unwrap_or_else(|| {
        let mut c = Camera::iso();
        if let Some((lo, hi)) = scene_bounds(s) {
            c.fit(lo, hi, w as f64, h as f64);
        }
        c
    });
    let mut img = Image { w, h, rgba: BACKGROUND.iter().copied().chain([255]).cycle().take(w * h * 4).collect() };
    let half = DVec2::new(w as f64, h as f64) / 2.0;
    let px = |p: DVec3| cam.project(p).0 + half;
    let reach = w.max(h) as f64 / cam.scale;
    for (axis, color) in [(DVec3::X, [214, 60, 60]), (DVec3::Y, [60, 170, 70]), (DVec3::Z, [60, 100, 220])] {
        img.line(px(cam.target.project_onto(axis) - axis * reach), px(cam.target.project_onto(axis) + axis * reach), color, 1.0);
    }
    draw_bodies(&mut img, s.visible_bodies(), &cam, None);
    // Plane overlays are deliberately faint and do not participate in mesh export.
    for (id,p) in &s.built.planes {
        if !s.built.component_visible(p.component) || !matches!(s.doc.feature(*id).map(|f|&f.kind),Some(crate::FeatureKind::Plane(c)) if c.visible) {continue;}
        let corners=p.corners.map(px);
        let (mut lo,mut hi)=(DVec2::splat(f64::INFINITY),DVec2::splat(f64::NEG_INFINITY));
        for c in corners {lo=lo.min(c);hi=hi.max(c);}
        let x0=lo.x.floor().max(0.) as usize; let y0=lo.y.floor().max(0.) as usize;
        let x1=hi.x.ceil().max(0.).min(w as f64) as usize; let y1=hi.y.ceil().max(0.).min(h as f64) as usize;
        for y in y0.min(h)..y1 {for x in x0.min(w)..x1 {
            if crate::profile::inside(&corners,DVec2::new(x as f64+0.5,y as f64+0.5)) {img.blend(x,y,CONSTRUCTION,0.1);}
        }}
        for i in 0..4 {img.line(corners[i],corners[(i+1)%4],CONSTRUCTION,1.2);}
    }
    for (f, sk) in s.doc.sketches().filter(|(f,sk)| sk.visible && s.built.component_visible(f.owner)) {
        let Some(plane)=s.built.sketch_plane(&s.doc,f.id) else {continue};
        for (id, e) in &sk.entities {
            let color = if e.construction { CONSTRUCTION } else { SKETCH };
            for seg in sk.polyline(*id).windows(2) {
                img.line(px(plane.to_world(seg[0])), px(plane.to_world(seg[1])), color, 2.0);
            }
        }
    }
    img
}
