//! Screw threads: a catalog of standard sizes with the hole sizes that go
//! with them, and the thread shape itself.
//!
//! The thread is generated directly as triangles. OpenCascade can sweep one,
//! but takes seconds per thread, fails on long ones and sometimes hands back
//! an empty solid; a generated mesh is immediate and always closed.

use std::f64::consts::TAU;

use glam::DVec3;

use crate::mesh::Mesh;

/// One standard thread, in millimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThreadSpec {
    /// As it is written on a drawing: `M3x0.5`, `1/4-20`.
    pub name: &'static str,
    pub family: Family,
    /// The coarse pitch for its diameter, which is what a bare `M3` means.
    pub coarse: bool,
    pub major: f64,
    pub pitch: f64,
    /// The drill for a hole that will be tapped.
    pub tap_drill: f64,
    /// Holes a screw passes through: close, normal and loose fits.
    pub clearance: [f64; 3],
    /// A counterbore that takes a socket cap screw's head.
    pub counterbore: f64,
    pub counterbore_depth: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// ISO metric, 60 degrees.
    Metric,
    /// Unified inch (UNC and UNF), 60 degrees.
    Unified,
}

impl Family {
    pub fn label(self) -> &'static str {
        match self {
            Family::Metric => "ISO Metric",
            Family::Unified => "Unified Inch",
        }
    }
}

impl ThreadSpec {
    /// A countersink that takes a flat head screw: across the top, and the included angle in degrees.
    pub fn countersink(&self) -> (f64, f64) {
        // Flat heads are twice the thread diameter across in both families; the rest is clearance.
        (round2(self.major * 2.1), if self.family == Family::Metric { 90.0 } else { 82.0 })
    }

    /// The diameter at the bottom of the thread.
    pub fn minor(&self) -> f64 {
        self.major - 1.25 * 3f64.sqrt() / 2.0 * self.pitch
    }

    pub fn size(&self) -> &'static str {
        let cut = self.name.find(if self.family == Family::Metric { 'x' } else { '-' }).unwrap_or(self.name.len());
        &self.name[..cut]
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

const fn m(name: &'static str, coarse: bool, major: f64, pitch: f64, tap_drill: f64, clearance: [f64; 3], counterbore: f64) -> ThreadSpec {
    // A socket cap head is as tall as the thread is wide.
    ThreadSpec { name, family: Family::Metric, coarse, major, pitch, tap_drill, clearance, counterbore, counterbore_depth: major + 0.4 }
}

const fn u(name: &'static str, coarse: bool, major_in: f64, tpi: f64, tap_in: f64, close_in: f64, free_in: f64, head_in: f64) -> ThreadSpec {
    let k = 25.4;
    ThreadSpec {
        name,
        family: Family::Unified,
        coarse,
        major: major_in * k,
        pitch: k / tpi,
        tap_drill: tap_in * k,
        // Inch tables give a close and a free fit; the free fit serves for both looser ones.
        clearance: [close_in * k, free_in * k, free_in * k],
        counterbore: (head_in + 1.0 / 32.0) * k,
        counterbore_depth: major_in * k + 0.4,
    }
}

/// Tap drills and clearance holes follow ISO 273 and the usual inch drill charts;
/// counterbores take DIN 912 / ASME B18.3 socket heads.
pub const CATALOG: &[ThreadSpec] = &[
    m("M1.6x0.35", true, 1.6, 0.35, 1.25, [1.7, 1.8, 2.0], 3.5),
    m("M2x0.4", true, 2.0, 0.4, 1.6, [2.2, 2.4, 2.6], 4.4),
    m("M2.5x0.45", true, 2.5, 0.45, 2.05, [2.7, 2.9, 3.1], 5.5),
    m("M3x0.5", true, 3.0, 0.5, 2.5, [3.2, 3.4, 3.6], 6.5),
    m("M3x0.35", false, 3.0, 0.35, 2.65, [3.2, 3.4, 3.6], 6.5),
    m("M4x0.7", true, 4.0, 0.7, 3.3, [4.3, 4.5, 4.8], 8.0),
    m("M4x0.5", false, 4.0, 0.5, 3.5, [4.3, 4.5, 4.8], 8.0),
    m("M5x0.8", true, 5.0, 0.8, 4.2, [5.3, 5.5, 5.8], 10.0),
    m("M5x0.5", false, 5.0, 0.5, 4.5, [5.3, 5.5, 5.8], 10.0),
    m("M6x1", true, 6.0, 1.0, 5.0, [6.4, 6.6, 7.0], 11.0),
    m("M6x0.75", false, 6.0, 0.75, 5.25, [6.4, 6.6, 7.0], 11.0),
    m("M8x1.25", true, 8.0, 1.25, 6.8, [8.4, 9.0, 10.0], 15.0),
    m("M8x1", false, 8.0, 1.0, 7.0, [8.4, 9.0, 10.0], 15.0),
    m("M10x1.5", true, 10.0, 1.5, 8.5, [10.5, 11.0, 12.0], 18.0),
    m("M10x1.25", false, 10.0, 1.25, 8.8, [10.5, 11.0, 12.0], 18.0),
    m("M10x1", false, 10.0, 1.0, 9.0, [10.5, 11.0, 12.0], 18.0),
    m("M12x1.75", true, 12.0, 1.75, 10.2, [13.0, 13.5, 14.5], 20.0),
    m("M12x1.5", false, 12.0, 1.5, 10.5, [13.0, 13.5, 14.5], 20.0),
    m("M12x1.25", false, 12.0, 1.25, 10.8, [13.0, 13.5, 14.5], 20.0),
    u("#4-40", true, 0.112, 40.0, 0.089, 0.116, 0.1285, 0.183),
    u("#6-32", true, 0.138, 32.0, 0.1065, 0.144, 0.1495, 0.226),
    u("#8-32", true, 0.164, 32.0, 0.136, 0.1695, 0.177, 0.270),
    u("#10-24", true, 0.190, 24.0, 0.1495, 0.196, 0.201, 0.312),
    u("#10-32", false, 0.190, 32.0, 0.159, 0.196, 0.201, 0.312),
    u("1/4-20", true, 0.25, 20.0, 0.201, 0.257, 0.266, 0.375),
    u("1/4-28", false, 0.25, 28.0, 0.213, 0.257, 0.266, 0.375),
    u("5/16-18", true, 0.3125, 18.0, 0.257, 0.323, 0.332, 0.469),
    u("5/16-24", false, 0.3125, 24.0, 0.272, 0.323, 0.332, 0.469),
    u("3/8-16", true, 0.375, 16.0, 0.3125, 0.386, 0.397, 0.562),
    u("3/8-24", false, 0.375, 24.0, 0.332, 0.386, 0.397, 0.562),
    u("1/2-13", true, 0.5, 13.0, 0.4219, 0.5156, 0.5312, 0.750),
    u("1/2-20", false, 0.5, 20.0, 0.4531, 0.5156, 0.5312, 0.750),
];

/// The catalog entry for a name as people write it: `M3`, `m3x0.5`, `M3 x 0.5`,
/// `1/4-20`, `#6-32 UNC`. A size alone means its coarse pitch.
pub fn find(name: &str) -> Result<&'static ThreadSpec, String> {
    let key: String = name.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_uppercase();
    let key = key.trim_end_matches("UNC").trim_end_matches("UNF").replace('×', "X");
    let same = |s: &str| s.to_uppercase() == key;
    CATALOG
        .iter()
        .find(|t| same(t.name))
        .or_else(|| CATALOG.iter().find(|t| t.coarse && same(t.size())))
        .ok_or_else(|| format!("there is no thread called '{name}' in the catalog; list_threads shows what there is (for example M3x0.5 or 1/4-20)"))
}

/// The coarse thread that a rod or hole `diameter` across was most likely
/// made for. A hole may have been drilled at the tap drill size, at the
/// thread's own diameter, or as a clearance hole for it.
pub fn nearest(diameter: f64, internal: bool) -> &'static ThreadSpec {
    let off = |t: &ThreadSpec| {
        // A 5 mm hole could be M6's tap drill or M5's own size; the tap drill is the likelier reason to thread it.
        let sizes = if internal { vec![(t.tap_drill, 0.0), (t.major, 1e-6), (t.clearance[0], 1e-6), (t.clearance[1], 1e-6), (t.clearance[2], 1e-6)] } else { vec![(t.major, 0.0)] };
        sizes.into_iter().map(|(s, less_likely)| (s - diameter).abs() + less_likely).fold(f64::MAX, f64::min)
    };
    // Metric comes first in the catalog and wins a tie.
    CATALOG.iter().filter(|t| t.coarse).min_by(|a, b| off(a).total_cmp(&off(b))).expect("the catalog is not empty")
}

/// How far a thread's shell reaches into the body it sits on, so that the
/// two overlap rather than merely touch.
pub const BED: f64 = 0.05;

/// A threaded rod as a closed mesh: along +Z from the origin for `length`,
/// `major` across the crests and the standard depth, cut square at both ends.
pub fn rod(major: f64, pitch: f64, length: f64, left: bool) -> Result<Mesh, String> {
    form(major, major - 1.25 * 3f64.sqrt() / 2.0 * pitch, pitch, length, left, None, [false, false])
}

/// An external thread whose free ends ease in over one pitch. The root diameter
/// is unchanged, so a matching pilot can guide it without a full-height square crest.
pub fn rod_with_lead(major: f64, pitch: f64, length: f64, left: bool, ends: [bool; 2]) -> Result<Mesh, String> {
    form(major, major - 1.25 * 3f64.sqrt() / 2.0 * pitch, pitch, length, left, None, ends)
}

/// The thread of a tapped hole as a closed mesh: a tube along +Z whose bore
/// is the thread, `minor` across its crests and `major` across its roots,
/// and whose outside is a plain cylinder `outer` across.
pub fn sleeve(major: f64, minor: f64, outer: f64, pitch: f64, length: f64, left: bool) -> Result<Mesh, String> {
    if outer <= major {
        return Err("the thread's sleeve must be wider than the thread".into());
    }
    form(major, minor, pitch, length, left, Some(outer / 2.0), [false, false])
}

/// The thread surface between `minor` and `major`, with 60 degree flanks and
/// an eighth of a pitch flat at the major diameter; the flat at the minor
/// diameter is whatever that leaves. Closed across the ends to the axis, or
/// with `wall`, out to a cylinder of that radius and turned inside out.
fn form(major: f64, minor: f64, pitch: f64, length: f64, left: bool, wall: Option<f64>, leads: [bool; 2]) -> Result<Mesh, String> {
    let (crest, root) = (major / 2.0, minor / 2.0);
    let depth = crest - root;
    if ![major, minor, pitch, length].iter().all(|n| n.is_finite()) || !(pitch > 0.0 && length > 0.0 && root > 0.0) {
        return Err("the thread's pitch is too coarse for its diameter".into());
    }
    if length / pitch > 2000.0 {
        return Err("the thread is too long for its pitch".into());
    }
    // Across one pitch: the root flat, the rising flank, the crest flat, the falling flank.
    let flank = depth / 3f64.sqrt() / pitch;
    let flat = 0.875 - 2.0 * flank;
    if depth <= 1e-6 || flat < 0.01 {
        return Err("the hole or rod is the wrong diameter for this thread".into());
    }
    let breaks = [0.0, flat, flat + flank, flat + flank + 0.125];
    let n = if major > 8.0 { 64 } else if major > 4.0 { 48 } else { 32 };
    // The radius at a height and angle; `u` is how far through a pitch the point is.
    let radius = |turn: f64, z: f64| {
        let u = (z / pitch - turn).rem_euclid(1.0);
        if u <= breaks[1] {
            root
        } else if u < breaks[2] {
            root + depth * (u - breaks[1]) / (breaks[2] - breaks[1])
        } else if u <= breaks[3] {
            crest
        } else {
            crest - depth * (u - breaks[3]) / (1.0 - breaks[3])
        }
    };
    let around = |r: f64, turn: f64, z: f64| DVec3::new(r * (turn * TAU).cos(), r * (turn * TAU).sin(), z);
    // Rows run along the helix, so every corner of the form is an edge of the mesh. A row
    // that would run past an end stops at it, and the triangles that flattens are dropped.
    let rows = 4 * ((length / pitch).ceil() as i64 + 2);
    let vertex = |col: usize, row: i64| {
        // A full turn round is the same place as one pitch up, so the seam shares its points.
        let (col, row) = if col == n { (0, row + 4) } else { (col, row) };
        let turn = col as f64 / n as f64;
        let on_form = row.div_euclid(4) as f64 + breaks[row.rem_euclid(4) as usize];
        let free = pitch * (turn + on_form);
        let z = free.clamp(0.0, length);
        let r = if z == free { [root, root, crest, crest][row.rem_euclid(4) as usize] } else { radius(turn, z) };
        let lead = pitch.min(length / 2.0);
        let r = if leads[0] { r.min(root + depth * (z / lead).min(1.0)) } else { r };
        let r = if leads[1] { r.min(root + depth * ((length - z) / lead).min(1.0)) } else { r };
        around(r, turn, z)
    };
    let mut tris = Vec::new();
    // Which part of the thread each triangle belongs to, so that its corners draw as edges.
    let mut parts = Vec::new();
    let mut push = |t: [DVec3; 3], part: u64| {
        if t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
            tris.push(t);
            parts.push(part);
        }
    };
    for col in 0..n {
        for row in -4..rows {
            let (a, b, c, d) = (vertex(col, row), vertex(col + 1, row), vertex(col + 1, row + 1), vertex(col, row + 1));
            let part = row.rem_euclid(4) as u64;
            push([a, b, c], part);
            push([a, c, d], part);
        }
        // The ends, from the rim (where the lowest and highest rows stopped) to the axis or the wall.
        let (lo0, lo1) = (vertex(col, -4), vertex(col + 1, -4));
        let (hi0, hi1) = (vertex(col, rows), vertex(col + 1, rows));
        match wall {
            None => {
                push([DVec3::ZERO, lo1, lo0], 4);
                push([DVec3::Z * length, hi0, hi1], 5);
            }
            Some(w) => {
                let (t0, t1) = (col as f64 / n as f64, ((col + 1) % n) as f64 / n as f64);
                let (a0, a1, b0, b1) = (around(w, t0, 0.0), around(w, t1, 0.0), around(w, t0, length), around(w, t1, length));
                push([lo0, a1, lo1], 4);
                push([lo0, a0, a1], 4);
                push([hi0, hi1, b1], 5);
                push([hi0, b1, b0], 5);
                push([a0, b0, b1], 6);
                push([a0, b1, a1], 6);
            }
        }
    }
    let mut mesh = Mesh::from_tris(tris);
    mesh.face_ids = parts;
    // A sleeve is the same surface seen from the other side.
    if wall.is_some() {
        mesh.flip();
    }
    if left {
        mesh.map(|v| DVec3::new(v.x, -v.y, v.z));
        mesh.flip();
    }
    Ok(mesh)
}

/// What a correct [`rod`] must enclose, for checking one.
pub fn rod_volume(major: f64, pitch: f64, length: f64) -> f64 {
    let depth = 0.625 * 3f64.sqrt() / 2.0 * pitch;
    let root = major / 2.0 - depth;
    // The ridge is a trapezoid three quarters of a pitch wide at the root and an eighth at the crest.
    let area = depth * 0.4375 * pitch;
    let centre = root + depth / 3.0 * (0.75 + 0.25) / 0.875;
    std::f64::consts::PI * root * root * length + area * TAU * centre * length / pitch
}

/// Analytical volume of [`rod_with_lead`], before circular tessellation error.
/// Around each full cross-section the root/crest flats occupy 1/4 and 1/8
/// of a turn, and the two linear flanks occupy 5/8. Integrating their squared
/// radius under a linear root-to-crest envelope gives the loss below.
pub fn rod_with_lead_volume(major: f64, pitch: f64, length: f64, ends: [bool; 2]) -> f64 {
    let depth = 0.625 * 3f64.sqrt() / 2.0 * pitch;
    let root = major / 2.0 - depth;
    let lead = pitch.min(length / 2.0);
    let loss = std::f64::consts::PI * lead * (root * depth / 3.0 + 3.0 * depth * depth / 16.0);
    rod_volume(major, pitch, length) - ends.iter().filter(|end| **end).count() as f64 * loss
}
