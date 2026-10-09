//! Deterministic text outlines from the bundled Noto Sans Bold font.
//! Documents store text and dimensions, never an external font path.

use glam::DVec2;
use serde::{Deserialize, Serialize};
use skrifa::{
    FontRef, GlyphId, MetadataProvider,
    instance::{LocationRef, Size},
    outline::{DrawSettings, OutlinePen},
};

use crate::profile::{Profile, Seg, inside, signed_area};

const FONT: &[u8] = include_bytes!("../assets/fonts/NotoSans-Bold.ttf");
pub const MAX_CHARACTERS: usize = 128;
pub const MAX_VERTICES: usize = 20_000;
const MAX_CONTOURS: usize = 1_024;
const MAX_SUBDIVISION: u8 = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// Closed XY regions for one line of text, with the baseline at y=0.
/// `height` is the font's capital height, in model millimeters; curved letters
/// can overshoot it slightly and descenders extend below the baseline.
/// `spacing` adds millimeters between characters. Alignment uses visible ink
/// bounds, so Left starts at x=0, Center straddles x=0, and Right ends at x=0.
/// Glyph advances are used without contextual shaping. Unsupported characters,
/// combining marks, control characters and multiline text are rejected.
pub fn profiles(
    content: &str,
    height: f64,
    spacing: f64,
    align: Align,
) -> Result<Vec<Profile>, String> {
    let count = content.chars().take(MAX_CHARACTERS + 1).count();
    if count == 0 || content.chars().all(char::is_whitespace) {
        return Err("text needs at least one visible character".into());
    }
    if count > MAX_CHARACTERS {
        return Err(format!("text is limited to {MAX_CHARACTERS} characters"));
    }
    if content
        .chars()
        .any(|c| c.is_control() || (c.is_whitespace() && c != ' '))
    {
        return Err("text must be a single line using ordinary spaces; control characters are not supported".into());
    }
    if !height.is_finite() || height <= 0.0 || height > 10_000.0 {
        return Err("text height must be finite, greater than zero, and at most 10000 mm".into());
    }
    if !spacing.is_finite() || !(0.0..=10_000.0).contains(&spacing) {
        return Err("text spacing must be finite and between 0 and 10000 mm".into());
    }
    let font = FontRef::new(FONT).map_err(|_| "the bundled text font could not be read")?;
    // Font units, unhinted and at the default instance: outlines depend only
    // on the bundled file, never on a rasterizer's grid.
    let (size, location) = (Size::unscaled(), LocationRef::default());
    let cap = font
        .metrics(size, location)
        .cap_height
        .filter(|h| *h > 0.0)
        .ok_or("the bundled font has no capital-height metric")? as f64;
    let (charmap, advances, outlines) = (
        font.charmap(),
        font.glyph_metrics(size, location),
        font.outline_glyphs(),
    );
    let scale = height / cap;
    // Relative precision for small labels; at most 0.02 mm curve deviation on
    // large lettering. Subdivision and aggregate vertex limits bound the work.
    let tolerance = (height * 0.001).min(0.02);
    let mut all = Vec::new();
    let mut cursor = 0.0;
    let mut vertices = 0;
    let mut contours = 0;
    for (index, character) in content.chars().enumerate() {
        let glyph = charmap
            .map(character)
            .filter(|id| *id != GlyphId::NOTDEF)
            .ok_or_else(|| {
                format!(
                    "the built-in font does not support '{character}' (U+{:04X})",
                    character as u32
                )
            })?;
        let advance = advances.advance_width(glyph).filter(|n| *n > 0.0)
            .ok_or_else(|| format!("'{character}' needs contextual positioning; use precomposed letters instead of combining marks"))? as f64 * scale;
        if character != ' ' {
            let mut outline = Outline::new(scale, cursor, tolerance, MAX_VERTICES - vertices);
            let drawn = outlines
                .get(glyph)
                .is_some_and(|g| g.draw(DrawSettings::unhinted(size, location), &mut outline).is_ok());
            outline.finish();
            if !drawn || outline.contours.is_empty() {
                return Err(format!("'{character}' has no usable text outline"));
            }
            if let Some(error) = outline.error {
                return Err(error);
            }
            vertices += outline.vertices;
            contours += outline.contours.len();
            if contours > MAX_CONTOURS {
                return Err("text has too many outline contours".into());
            }
            all.extend(regions(outline.contours)?);
        }
        cursor += advance + if index + 1 < count { spacing } else { 0.0 };
        if !cursor.is_finite() || cursor > 1_000_000.0 {
            return Err("text is too wide".into());
        }
    }
    let (lo, hi) = all
        .iter()
        .flat_map(|p| &p.outer)
        .map(|p| p.x)
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), x| {
            (lo.min(x), hi.max(x))
        });
    if !lo.is_finite() || !hi.is_finite() {
        return Err("text has no visible outlines".into());
    }
    let shift = match align {
        Align::Left => lo,
        Align::Center => (lo + hi) / 2.0,
        Align::Right => hi,
    };
    for profile in &mut all {
        for p in profile
            .outer
            .iter_mut()
            .chain(profile.holes.iter_mut().flatten())
        {
            p.x -= shift;
        }
        profile.path = lines(&profile.outer);
        profile.hole_paths = profile.holes.iter().map(|ring| lines(ring)).collect();
    }
    Ok(all)
}

fn lines(ring: &[DVec2]) -> Vec<Seg> {
    (0..ring.len())
        .map(|i| Seg::Line(ring[i], ring[(i + 1) % ring.len()]))
        .collect()
}

/// Classify one glyph independently, keeping counters out of the material and
/// preserving separate components such as the dot over an i or an accent.
fn regions(mut rings: Vec<Vec<DVec2>>) -> Result<Vec<Profile>, String> {
    if rings.is_empty() {
        return Err("a character has no closed outline".into());
    }
    let areas: Vec<f64> = rings.iter().map(|ring| signed_area(ring).abs()).collect();
    if areas.iter().any(|area| !area.is_finite() || *area <= 1e-16) {
        return Err("text is too small to form reliable closed outlines".into());
    }
    let parents: Vec<Option<usize>> = (0..rings.len())
        .map(|i| {
            (0..rings.len())
                .filter(|j| areas[*j] > areas[i] && inside(&rings[*j], rings[i][0]))
                .min_by(|a, b| areas[*a].total_cmp(&areas[*b]))
        })
        .collect();
    let depths: Vec<usize> = parents
        .iter()
        .map(|parent| {
            let (mut at, mut depth) = (*parent, 0);
            while let Some(index) = at {
                depth += 1;
                at = parents[index];
            }
            depth
        })
        .collect();
    for (i, ring) in rings.iter_mut().enumerate() {
        let positive = depths[i] % 2 == 0;
        if (signed_area(ring) > 0.0) != positive {
            ring.reverse();
        }
    }
    Ok((0..rings.len())
        .filter(|i| depths[*i] % 2 == 0)
        .map(|i| Profile {
            outer: rings[i].clone(),
            holes: (0..rings.len())
                .filter(|j| parents[*j] == Some(i))
                .map(|j| rings[j].clone())
                .collect(),
            edges: Vec::new(),
            depth: 0,
            path: Vec::new(),
            hole_paths: Vec::new(),
            path_ids: Vec::new(),
            hole_path_ids: Vec::new(),
        })
        .collect())
}

struct Outline {
    contours: Vec<Vec<DVec2>>,
    current: Vec<DVec2>,
    scale: f64,
    x: f64,
    tolerance: f64,
    vertices: usize,
    budget: usize,
    error: Option<String>,
}

impl Outline {
    fn new(scale: f64, x: f64, tolerance: f64, budget: usize) -> Self {
        Self {
            contours: Vec::new(),
            current: Vec::new(),
            scale,
            x,
            tolerance,
            vertices: 0,
            budget,
            error: None,
        }
    }

    fn point(&self, x: f32, y: f32) -> DVec2 {
        DVec2::new(x as f64 * self.scale + self.x, y as f64 * self.scale)
    }

    fn push(&mut self, point: DVec2) {
        if self.error.is_some() {
            return;
        }
        if !point.is_finite() {
            self.error = Some("text outline coordinates are not finite".into());
            return;
        }
        if self
            .current
            .last()
            .is_some_and(|last| last.distance_squared(point) <= (self.tolerance * 1e-4).powi(2))
        {
            return;
        }
        if self.vertices >= self.budget {
            self.error = Some(format!(
                "text is too detailed (maximum {MAX_VERTICES} outline vertices); shorten it or reduce its height"
            ));
            return;
        }
        self.current.push(point);
        self.vertices += 1;
    }

    fn finish(&mut self) {
        if self.current.len() > 1
            && self.current[0].distance_squared(*self.current.last().unwrap())
                <= (self.tolerance * 1e-4).powi(2)
        {
            self.current.pop();
        }
        if !self.current.is_empty() {
            if self.current.len() < 3 {
                self.error = Some("a text outline is not closed".into());
            }
            self.contours.push(std::mem::take(&mut self.current));
        }
    }

    fn quadratic(&mut self, a: DVec2, b: DVec2, c: DVec2, depth: u8) {
        if self.error.is_some() {
            return;
        }
        if distance_to_segment(b, a, c) <= self.tolerance {
            self.push(c);
            return;
        }
        if depth == MAX_SUBDIVISION {
            self.error = Some("a text curve exceeds the subdivision limit".into());
            return;
        }
        let (ab, bc) = ((a + b) / 2.0, (b + c) / 2.0);
        let mid = (ab + bc) / 2.0;
        self.quadratic(a, ab, mid, depth + 1);
        self.quadratic(mid, bc, c, depth + 1);
    }

    fn cubic(&mut self, a: DVec2, b: DVec2, c: DVec2, d: DVec2, depth: u8) {
        if self.error.is_some() {
            return;
        }
        if distance_to_segment(b, a, d).max(distance_to_segment(c, a, d)) <= self.tolerance {
            self.push(d);
            return;
        }
        if depth == MAX_SUBDIVISION {
            self.error = Some("a text curve exceeds the subdivision limit".into());
            return;
        }
        let (ab, bc, cd) = ((a + b) / 2.0, (b + c) / 2.0, (c + d) / 2.0);
        let (abc, bcd) = ((ab + bc) / 2.0, (bc + cd) / 2.0);
        let mid = (abc + bcd) / 2.0;
        self.cubic(a, ab, abc, mid, depth + 1);
        self.cubic(mid, bcd, cd, d, depth + 1);
    }
}

fn distance_to_segment(p: DVec2, a: DVec2, b: DVec2) -> f64 {
    let ab = b - a;
    let len = ab.length_squared();
    if len == 0.0 {
        return p.distance(a);
    }
    p.distance(a + ab * ((p - a).dot(ab) / len).clamp(0.0, 1.0))
}

impl OutlinePen for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.finish();
        self.push(self.point(x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.push(self.point(x, y));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        if let Some(a) = self.current.last().copied() {
            self.quadratic(a, self.point(x1, y1), self.point(x, y), 0);
        }
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        if let Some(a) = self.current.last().copied() {
            self.cubic(
                a,
                self.point(x1, y1),
                self.point(x2, y2),
                self.point(x, y),
                0,
            );
        }
    }
    fn close(&mut self) {
        self.finish();
    }
}
