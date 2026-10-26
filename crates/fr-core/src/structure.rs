//! CAD adapter for preliminary planar timber-member estimates. Geometry is read
//! from the current solved sketch/extrude, never a nominal-size lookup or mesh bbox.
//! The mechanics engine lives in fr-structure and has no CAD or GUI dependencies.

use std::collections::{BTreeMap, BTreeSet};

use glam::{DVec2, DVec3};
use serde::{Deserialize, Serialize};
use serde_json::{Value as J, json};

use crate::{FeatureKind, Id, Op, Session};

type R<T> = Result<T, String>;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Factors {
    #[serde(default)]
    pub bending: BTreeMap<String, f64>,
    #[serde(default)]
    pub shear: BTreeMap<String, f64>,
    #[serde(default)]
    pub elasticity: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignValues {
    pub e_mpa: f64,
    pub fb_mpa: f64,
    pub fv_mpa: f64,
    /// NDS edition/table/row or other provenance, supplied and verified by the caller.
    pub source: String,
    #[serde(default)]
    pub factors: Factors,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberTag {
    pub species: String,
    pub grade: String,
    /// Label only. Actual section dimensions always come from the extrusion.
    pub nominal_size: String,
    pub design_values: DesignValues,
    #[serde(default)]
    pub density_kg_m3: Option<f64>,
    /// Measured bearing lengths from the two physical member ends. Reporting
    /// only: supports must still be placed explicitly in the analysis request.
    #[serde(default)]
    pub end_bearing_mm: Option<[f64; 2]>,
}

fn positive(v: f64, label: &str) -> R<()> {
    if !v.is_finite() || v <= 0.0 {
        Err(format!("{label} must be positive and finite"))
    } else {
        Ok(())
    }
}

impl MemberTag {
    pub fn validate(&self) -> R<()> {
        for (label, value) in [
            ("species", &self.species),
            ("grade", &self.grade),
            ("nominal size", &self.nominal_size),
            ("design value source", &self.design_values.source),
        ] {
            if value.trim().is_empty() || value.len() > 1024 {
                return Err(format!("{label} must be nonempty and at most 1024 bytes"));
            }
        }
        let d = &self.design_values;
        for (value, label) in [(d.e_mpa, "E"), (d.fb_mpa, "Fb"), (d.fv_mpa, "Fv")] {
            positive(value, label)?;
        }
        for (map, allowed) in [
            (
                &d.factors.bending,
                &["CD", "CM", "Ct", "CL", "CF", "Cfu", "Ci", "Cr"][..],
            ),
            (&d.factors.shear, &["CD", "CM", "Ct", "Ci"][..]),
            (&d.factors.elasticity, &["CM", "Ct", "Ci"][..]),
        ] {
            for (name, &value) in map {
                if !allowed.contains(&name.as_str()) {
                    return Err(format!(
                        "factor {name} is not supported for this property (sawn-lumber ASD only)"
                    ));
                }
                positive(value, name)?;
            }
        }
        for value in self.adjusted() {
            positive(value, "adjusted design value")?;
        }
        if let Some(density) = self.density_kg_m3 {
            positive(density, "density")?;
        }
        if let Some(bearing) = self.end_bearing_mm {
            for value in bearing {
                positive(value, "end bearing")?;
            }
        }
        Ok(())
    }

    /// E', Fb', Fv'. Omitted factors multiply by one, not an inferred allowance.
    pub fn adjusted(&self) -> [f64; 3] {
        let d = &self.design_values;
        [
            d.e_mpa * d.factors.elasticity.values().product::<f64>(),
            d.fb_mpa * d.factors.bending.values().product::<f64>(),
            d.fv_mpa * d.factors.shear.values().product::<f64>(),
        ]
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Geometry {
    pub body: Id,
    pub start_mm: DVec3,
    pub end_mm: DVec3,
    pub length_mm: f64,
    pub section_axes: [DVec3; 2],
    pub section_mm: [f64; 2],
    pub area_mm2: f64,
    /// Only populated when measured bearing lengths have been supplied.
    pub clear_span_mm: Option<f64>,
    pub support_centers_from_start_mm: Option<[f64; 2]>,
}

pub fn geometry(s: &Session, body: Id, tag: &MemberTag) -> R<Geometry> {
    tag.validate()?;
    if s.read_only {
        return Err("unverified cached previews cannot be structurally analyzed".into());
    }
    if !s.built.errors.is_empty() {
        return Err("resolve feature errors before structural analysis".into());
    }
    let actual = s.built.body(body).ok_or(format!(
        "member {body} is missing, suppressed, consumed or rolled back"
    ))?;
    let feature = s
        .doc
        .feature(body)
        .ok_or("members must be direct extrusions; pattern copies are not yet supported")?;
    let FeatureKind::Extrude(e) = &feature.kind else {
        return Err("a structural member must be a rectangular extrusion along its length".into());
    };
    if e.op != Op::New
        || e.through_all
        || e.taper.as_ref().is_some_and(|v| v.v.abs() > 1e-9)
        || !actual.is_exact()
        || e.profiles.len() != 1
    {
        return Err("members require one untapered rectangular New Body extrusion with an explicit distance".into());
    }
    // Conservative first pass: do not silently analyze the original extrusion
    // after a notch, cut, body transform, shell, hole, or other geometry edit.
    for f in s
        .doc
        .features
        .iter()
        .take(s.doc.active())
        .skip_while(|f| f.id != body)
        .skip(1)
        .filter(|f| !s.doc.is_suppressed(f.id))
    {
        let modifies = match &f.kind {
            FeatureKind::Transform(v) => v.body == body,
            FeatureKind::Blend(v) => v.body == body,
            FeatureKind::Shell(v) => v.body == body,
            FeatureKind::Tilt(v) => v.body == body,
            FeatureKind::Hole(v) => v.body == body,
            FeatureKind::Thread(v) => v.body == body,
            FeatureKind::MeshOp(v) => v.body == body,
            FeatureKind::Split(v) => v.body == body,
            FeatureKind::Remove(v) => v.bodies.contains(&body),
            FeatureKind::Combine(v) => {
                v.target == body || (!v.keep_tools && v.tools.contains(&body))
            }
            FeatureKind::Text(v) => v.body == Some(body),
            FeatureKind::Emboss(v) => v.body == body,
            FeatureKind::Pipe(v) => v.op != Op::New && f.owner == feature.owner,
            FeatureKind::Extrude(v) => v.op != Op::New && f.owner == feature.owner,
            FeatureKind::Revolve(v) => v.op != Op::New && f.owner == feature.owner,
            FeatureKind::Primitive(v) => v.op != Op::New && f.owner == feature.owner,
            FeatureKind::Sweep(v) => v.op != Op::New && f.owner == feature.owner,
            FeatureKind::Loft(v) => v.op != Op::New && f.owner == feature.owner,
            FeatureKind::Relief(v) => v.op != Op::New && f.owner == feature.owner,
            FeatureKind::Sketch(_)
            | FeatureKind::Plane(_)
            | FeatureKind::Component(_)
            | FeatureKind::Pattern(_)
            | FeatureKind::Import(_)
            | FeatureKind::ScriptRun(_) => false,
        };
        if modifies {
            return Err(format!(
                "member {body} has a later {} feature; only unmodified extrusions are supported (use sketch/component placement)",
                f.type_name()
            ));
        }
    }
    let sk = s
        .doc
        .sketch(e.sketch)
        .ok_or("the member sketch is missing")?;
    let plane = s
        .built
        .sketch_plane(&s.doc, e.sketch)
        .ok_or("the member sketch plane is unavailable")?;
    let all = crate::profile::profiles(sk);
    let p = all
        .iter()
        .find(|p| p.edges == e.profiles[0])
        .ok_or("the member profile is no longer closed")?;
    if p.outer.len() != 4
        || !p.holes.is_empty()
        || p.path.len() != 4
        || p.path
            .iter()
            .any(|seg| !matches!(seg, crate::profile::Seg::Line(..)))
    {
        return Err("the member profile must be a four-sided rectangle without holes".into());
    }
    let edges: [DVec2; 4] = std::array::from_fn(|i| p.outer[(i + 1) % 4] - p.outer[i]);
    let width = edges[0].length();
    let depth = edges[1].length();
    positive(width, "section width")?;
    positive(depth, "section depth")?;
    let tolerance = 1e-6 * width.max(depth).max(1.0);
    if edges[0].normalize().dot(edges[1].normalize()).abs() > 1e-8
        || (edges[0] + edges[2]).length() > tolerance
        || (edges[1] + edges[3]).length() > tolerance
    {
        return Err("the member profile must be rectangular".into());
    }
    let center = plane.to_world(p.outer.iter().sum::<DVec2>() / 4.0);
    let z0 = if e.symmetric {
        -e.distance.v / 2.0
    } else {
        0.0
    };
    let start = center + plane.normal() * z0;
    let end = start + plane.normal() * e.distance.v;
    let length = start.distance(end);
    positive(length, "member length")?;
    let clear_span = tag.end_bearing_mm.map(|[a, b]| length - a - b);
    if let Some(clear) = clear_span {
        positive(clear, "clear span after subtracting end bearing")?;
    }
    let axes = [edges[0] / width, edges[1] / depth].map(|v| plane.x * v.x + plane.y * v.y);
    Ok(Geometry {
        body,
        start_mm: start,
        end_mm: end,
        length_mm: length,
        section_axes: axes,
        section_mm: [width, depth],
        area_mm2: width * depth,
        clear_span_mm: clear_span,
        support_centers_from_start_mm: tag.end_bearing_mm.map(|[a, b]| [a / 2.0, length - b / 2.0]),
    })
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub enum AnalysisPlane {
    XY,
    XZ,
    YZ,
}

impl AnalysisPlane {
    fn axes(self) -> (DVec3, DVec3) {
        match self {
            Self::XY => (DVec3::X, DVec3::Y),
            Self::XZ => (DVec3::X, DVec3::Z),
            Self::YZ => (DVec3::Y, DVec3::Z),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub body: Id,
    /// Fraction of the physical extrusion from start to end, in [0,1].
    pub at: f64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Support {
    pub location: Location,
    pub fix: [bool; 3],
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointLoad {
    pub location: Location,
    pub force_n: [f64; 2],
    #[serde(default)]
    pub moment_n_mm: f64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UniformLoad {
    pub body: Id,
    pub force_n_per_mm: [f64; 2],
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AreaLoad {
    pub body: Id,
    pub pressure_pa: f64,
    pub tributary_width_mm: f64,
    /// A unit direction in the analysis plane. Per actual length, not projected.
    pub direction: [f64; 2],
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Analysis {
    pub plane: AnalysisPlane,
    /// None selects all tagged bodies, including hidden members.
    #[serde(default)]
    pub bodies: Option<Vec<Id>>,
    pub supports: Vec<Support>,
    #[serde(default)]
    pub point_loads: Vec<PointLoad>,
    #[serde(default)]
    pub uniform_loads: Vec<UniformLoad>,
    #[serde(default)]
    pub area_loads: Vec<AreaLoad>,
    /// Rigid joints, explicitly opted in. Coincident geometry never implies a joint.
    #[serde(default)]
    pub connections: Vec<[Location; 2]>,
    /// Mandatory decision. When true, every member needs density; gravity is -Z.
    pub include_self_weight: bool,
    #[serde(default = "default_scale")]
    pub deformation_scale: f64,
    /// A what-if: every member is analysed with this width (across the frame) and depth
    /// (in it) instead of its measured section. The design itself is not changed, and
    /// the result says that it is a trial.
    #[serde(default)]
    pub trial_section_mm: Option<[f64; 2]>,
}

fn default_scale() -> f64 {
    20.0
}

struct Prepared {
    geometry: Geometry,
    tag: MemberTag,
    width: f64,
    depth: f64,
    area: f64,
    stations: Vec<f64>,
    nodes: Vec<usize>,
    q: [f64; 2],
}

fn locate(members: &BTreeMap<Id, Prepared>, location: Location) -> R<usize> {
    let m = members
        .get(&location.body)
        .ok_or("location refers to an unselected member")?;
    let i = m
        .stations
        .iter()
        .position(|t| (*t - location.at).abs() < 1e-10)
        .ok_or("location has no analysis node")?;
    Ok(m.nodes[i])
}

fn root(parent: &[usize], mut i: usize) -> usize {
    while parent[i] != i {
        i = parent[i];
    }
    i
}

pub fn analyze(s: &Session, request: &Analysis) -> R<J> {
    let ids = request
        .bodies
        .clone()
        .unwrap_or_else(|| s.doc.structural_members.keys().copied().collect());
    if ids.is_empty() || ids.len() > 64 || ids.iter().collect::<BTreeSet<_>>().len() != ids.len() {
        return Err("select between 1 and 64 distinct tagged bodies".into());
    }
    if request.supports.len() + request.point_loads.len() + request.connections.len() > 256
        || request.uniform_loads.len() + request.area_loads.len() > 256
    {
        return Err(
            "too many structural supports, loads or connections (limit 256 each group)".into(),
        );
    }
    if !request.deformation_scale.is_finite()
        || !(0.0..=10000.0).contains(&request.deformation_scale)
    {
        return Err("deformation_scale must be between 0 and 10000".into());
    }
    let (x_axis, y_axis) = request.plane.axes();
    let normal = x_axis.cross(y_axis);
    let project = |v: DVec3| [v.dot(x_axis), v.dot(y_axis)];
    let mut members = BTreeMap::new();
    for id in ids {
        let tag = s
            .doc
            .structural_members
            .get(&id)
            .ok_or(format!("body {id} has no structural member tag"))?
            .clone();
        let g = geometry(s, id, &tag)?;
        let axis = (g.end_mm - g.start_mm) / g.length_mm;
        if axis.dot(normal).abs() > 1e-8 {
            return Err(format!("member {id} is out of the selected analysis plane"));
        }
        // One cross-section edge must be normal to the analysis plane. Reject
        // biaxial bending instead of substituting a bounding rectangle or strong axis.
        let depth_axis = if g.section_axes[0].dot(normal).abs() > 1.0 - 1e-8 {
            1
        } else if g.section_axes[1].dot(normal).abs() > 1.0 - 1e-8 {
            0
        } else {
            return Err(format!(
                "member {id} section is rotated out of plane; biaxial bending is not supported"
            ));
        };
        let (width, depth) = (g.section_mm[1 - depth_axis], g.section_mm[depth_axis]);
        let (width, depth) = match request.trial_section_mm {
            Some([w, d]) => {
                positive(w, "trial width")?;
                positive(d, "trial depth")?;
                (w, d)
            }
            None => (width, depth),
        };
        let area = width * depth;
        let mut q = [0.0; 2];
        if request.include_self_weight {
            if normal.dot(DVec3::Z).abs() > 1e-8 {
                return Err("self-weight acts outside an XY frame; use XZ or YZ".into());
            }
            let density = tag
                .density_kg_m3
                .ok_or(format!("member {id} needs density_kg_m3 for self-weight"))?;
            q = project(-DVec3::Z * density * area * 1e-9 * 9.80665);
        }
        members.insert(
            id,
            Prepared {
                geometry: g,
                tag,
                width,
                depth,
                area,
                stations: vec![0.0, 1.0],
                nodes: Vec::new(),
                q,
            },
        );
    }
    let locations = request
        .supports
        .iter()
        .map(|v| v.location)
        .chain(request.point_loads.iter().map(|v| v.location))
        .chain(request.connections.iter().flatten().copied());
    for location in locations {
        if !location.at.is_finite() || !(0.0..=1.0).contains(&location.at) {
            return Err("a location's at must be a fraction between 0 and 1".into());
        }
        members
            .get_mut(&location.body)
            .ok_or("location refers to an unselected member")?
            .stations
            .push(location.at);
    }
    for load in &request.uniform_loads {
        if load.force_n_per_mm.iter().any(|v| !v.is_finite()) {
            return Err("uniform loads must be finite".into());
        }
        let m = members
            .get_mut(&load.body)
            .ok_or("uniform load refers to an unselected member")?;
        for i in 0..2 {
            m.q[i] += load.force_n_per_mm[i];
        }
    }
    for load in &request.area_loads {
        if !load.pressure_pa.is_finite()
            || load.pressure_pa < 0.0
            || load.direction.iter().any(|v| !v.is_finite())
            || (load.direction[0].hypot(load.direction[1]) - 1.0).abs() > 1e-8
        {
            return Err("area loads need nonnegative finite pressure and a unit direction".into());
        }
        positive(load.tributary_width_mm, "tributary width")?;
        let m = members
            .get_mut(&load.body)
            .ok_or("area load refers to an unselected member")?;
        for i in 0..2 {
            m.q[i] += load.pressure_pa * load.tributary_width_mm * 1e-6 * load.direction[i];
        }
    }
    let mut world_nodes = Vec::new();
    for m in members.values_mut() {
        m.stations.sort_by(f64::total_cmp);
        m.stations.dedup_by(|a, b| (*a - *b).abs() < 1e-10);
        for &t in &m.stations {
            m.nodes.push(world_nodes.len());
            world_nodes.push(m.geometry.start_mm.lerp(m.geometry.end_mm, t));
        }
    }
    if world_nodes.len() > fr_structure::MAX_NODES {
        return Err(
            "the analysis exceeds 128 nodes; use fewer members or load/support stations".into(),
        );
    }
    let mut parents: Vec<_> = (0..world_nodes.len()).collect();
    for [a, b] in &request.connections {
        let i = locate(&members, *a)?;
        let j = locate(&members, *b)?;
        if world_nodes[i].distance(world_nodes[j]) > 1e-6 {
            return Err("rigid joints require coincident 3D centerline locations; eccentric connections are unsupported".into());
        }
        let (i, j) = (root(&parents, i), root(&parents, j));
        parents[j] = i;
    }
    let mut frame = fr_structure::Frame::default();
    let mut compact = BTreeMap::new();
    let mut node_map = Vec::new();
    let mut node_world = Vec::new();
    for i in 0..world_nodes.len() {
        let key = root(&parents, i);
        let n = *compact.entry(key).or_insert_with(|| {
            let n = frame.nodes.len();
            node_world.push(world_nodes[key]);
            frame.nodes.push(fr_structure::Node {
                position: project(world_nodes[key]),
                restrained: [false; 3],
                load: [0.0; 3],
            });
            n
        });
        node_map.push(n);
    }
    for support in &request.supports {
        if !support.fix.iter().any(|v| *v) {
            return Err("a support must restrain at least one DOF".into());
        }
        let n = node_map[locate(&members, support.location)?];
        for i in 0..3 {
            frame.nodes[n].restrained[i] |= support.fix[i];
        }
    }
    for load in &request.point_loads {
        if load
            .force_n
            .iter()
            .chain([load.moment_n_mm].iter())
            .any(|v| !v.is_finite())
        {
            return Err("point loads must be finite".into());
        }
        let n = node_map[locate(&members, load.location)?];
        frame.nodes[n].load[0] += load.force_n[0];
        frame.nodes[n].load[1] += load.force_n[1];
        frame.nodes[n].load[2] += load.moment_n_mm;
    }
    let mut mapping = Vec::new();
    for (&id, m) in &members {
        for i in 0..m.nodes.len() - 1 {
            frame.members.push(fr_structure::Member {
                nodes: [node_map[m.nodes[i]], node_map[m.nodes[i + 1]]],
                e: m.tag.adjusted()[0],
                area: m.area,
                inertia: m.width * m.depth.powi(3) / 12.0,
                uniform_load: m.q,
            });
            mapping.push((id, i));
        }
    }
    let solution = frame.solve()?;
    let mut results = Vec::new();
    for (&id, m) in &members {
        let mut max_moment = 0.0_f64;
        let mut max_shear = 0.0_f64;
        let mut max_axial = 0.0_f64;
        let mut max_displacement = 0.0_f64;
        let mut samples = Vec::new();
        for (element, &(body, i)) in mapping
            .iter()
            .enumerate()
            .filter(|(_, (body, _))| *body == id)
        {
            debug_assert_eq!(body, id);
            let out = &solution.members[element];
            max_moment = max_moment.max(out.max_abs_moment);
            max_shear = max_shear.max(out.max_abs_shear);
            max_axial = max_axial.max(out.max_abs_axial);
            for p in &out.samples {
                let at = m.stations[i] + p.fraction * (m.stations[i + 1] - m.stations[i]);
                let original = m.geometry.start_mm.lerp(m.geometry.end_mm, at);
                let displacement = x_axis * p.displacement[0] + y_axis * p.displacement[1];
                max_displacement = max_displacement.max(displacement.length());
                samples.push(
                    json!({"at":at,"position_mm":original,"displacement_mm":displacement,
                    "deformed_mm":original + displacement * request.deformation_scale,
                    "axial_n":p.axial_force,"shear_n":p.shear_force,"moment_n_mm":p.moment}),
                );
            }
        }
        let section_modulus = m.width * m.depth.powi(2) / 6.0;
        let fb = max_moment / section_modulus;
        let fv = 1.5 * max_shear / m.area;
        let [e, fb_allow, fv_allow] = m.tag.adjusted();
        let utilization = (fb / fb_allow).max(fv / fv_allow);
        if !utilization.is_finite() {
            return Err("stress utilization exceeds numeric range".into());
        }
        results.push(json!({"body":id,"geometry":m.geometry,"material":m.tag,
            "section":{"width_mm":m.width,"depth_mm":m.depth,"area_mm2":m.area,"inertia_mm4":m.width * m.depth.powi(3) / 12.0,"section_modulus_mm3":section_modulus},
            "adjusted_design_values":{"e_mpa":e,"fb_mpa":fb_allow,"fv_mpa":fv_allow},
            "uniform_load_n_per_mm":m.q,"max_moment_n_mm":max_moment,"max_shear_n":max_shear,"max_axial_n":max_axial,
            "bending_stress_mpa":fb,"shear_stress_mpa":fv,"axial_stress_mpa":max_axial / m.area,
            "bending_utilization":fb / fb_allow,"shear_utilization":fv / fv_allow,"bending_shear_utilization":utilization,
            "color_rgb":utilization_color(utilization),"sampled_max_displacement_mm":max_displacement,
            "axial_design_required":max_axial > 1e-6,"samples":samples}));
    }
    let nodes: Vec<_> = frame.nodes.iter().enumerate().map(|(i,n)| json!({"node":i,"position_mm":node_world[i],"fix":n.restrained,
        "displacement_mm":[solution.displacements[i][0],solution.displacements[i][1]],"rotation_rad":solution.displacements[i][2],
        "reaction_n":[solution.reactions[i][0],solution.reactions[i][1]],"reaction_moment_n_mm":solution.reactions[i][2]})).collect();
    Ok(
        json!({"method":"linear_2d_euler_bernoulli","units":{"length":"mm","force":"N","moment":"N mm","stress":"MPa"},
        "analysis_axes":[x_axis,y_axis],"trial_section_mm":request.trial_section_mm,"deformation_scale":request.deformation_scale,"include_self_weight":request.include_self_weight,
        "members":results,"nodes":nodes,"design_checks_complete":false,
        "limitations":["Preliminary bending/shear estimates using caller-supplied values and factors; not a Forte calculation or permit approval.",
            "No buckling, axial/bending interaction, bearing capacity, connections, sistering fasteners, creep, load combinations, torsion or 3D behavior.",
            "Deflection is sampled; no serviceability limit is assumed. Omitted design factors equal one. Hidden members are included when selected.",
            "Connections are rigid only where explicitly declared. No automatic load transfer through touching solids or deck sheathing."]}),
    )
}

/// Green through yellow to red for the reported bending/shear ratio only.
pub fn utilization_color(ratio: f64) -> [u8; 3] {
    let t = ratio.clamp(0.0, 1.0);
    if t <= 0.5 {
        [((t * 2.0) * 220.0 + 35.0) as u8, 190, 55]
    } else {
        [255, ((1.0 - t) * 2.0 * 150.0 + 40.0) as u8, 55]
    }
}

/// A quick load case for a floor or loft: one pressure on the deck, carried by every
/// tagged member as a simple span between its two ends. It is kept with the design.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    /// What is stored or stood on the deck, in Pa.
    pub live_pa: f64,
    /// The deck and anything fixed to it, in Pa. The members' own weight is separate.
    #[serde(default)]
    pub dead_pa: f64,
    #[serde(default)]
    pub include_self_weight: bool,
    /// The width of deck a member carries, in mm, by body. A member not named here takes
    /// its width from the spacing of the members beside it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tributary_mm: BTreeMap<Id, f64>,
    /// See [`Analysis::trial_section_mm`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trial_section_mm: Option<[f64; 2]>,
    #[serde(default = "default_scale")]
    pub deformation_scale: f64,
}

impl Case {
    pub fn validate(&self) -> R<()> {
        for (value, label) in [(self.live_pa, "the live load"), (self.dead_pa, "the dead load")] {
            if !value.is_finite() || !(0.0..=1e6).contains(&value) {
                return Err(format!("{label} must be between 0 and 1 000 000 Pa"));
            }
        }
        if self.tributary_mm.len() > 1024 {
            return Err("at most 1024 tributary widths are supported".into());
        }
        for width in self.tributary_mm.values() {
            positive(*width, "tributary width")?;
        }
        if let Some([w, d]) = self.trial_section_mm {
            positive(w, "trial width")?;
            positive(d, "trial depth")?;
        }
        if !self.deformation_scale.is_finite() || !(0.0..=10000.0).contains(&self.deformation_scale) {
            return Err("deformation_scale must be between 0 and 10000".into());
        }
        Ok(())
    }
}

/// The width of deck each member carries when nothing says otherwise: half the gap to
/// the member on each side. A member at the end of the row has one neighbour, and is
/// taken to carry as much again on its open side, as a joist next to a wall does.
/// A member with no parallel neighbour has no width to offer and is left out.
pub fn spacing_tributary(members: &[Geometry], across: DVec3) -> BTreeMap<Id, f64> {
    let mut row: Vec<(f64, Id)> = members.iter().map(|g| (((g.start_mm + g.end_mm) / 2.0).dot(across), g.body)).collect();
    row.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out = BTreeMap::new();
    for (i, &(at, body)) in row.iter().enumerate() {
        let before = i.checked_sub(1).map(|j| at - row[j].0);
        let after = row.get(i + 1).map(|next| next.0 - at);
        let width = match (before, after) {
            (Some(a), Some(b)) => (a + b) / 2.0,
            (Some(gap), None) | (None, Some(gap)) => gap,
            (None, None) => continue,
        };
        if width > 1e-6 {
            out.insert(body, width);
        }
    }
    out
}

/// Runs a [`Case`] over every tagged member that can be analysed. The members must be
/// level and run the same way, along X or along Y. Each is pinned at one end and free
/// to slide at the other, at the middle of its measured bearing when it has one.
pub fn estimate(s: &Session, case: &Case) -> R<J> {
    case.validate()?;
    let (mut members, mut skipped) = (Vec::new(), Vec::new());
    for (&id, tag) in &s.doc.structural_members {
        match geometry(s, id, tag) {
            Ok(g) => members.push(g),
            Err(e) => skipped.push(json!({"body": id, "error": e})),
        }
    }
    if members.is_empty() {
        return Err(match skipped.first() {
            Some(first) => format!("no tagged member can be analysed: {}", first["error"].as_str().unwrap_or_default()),
            None => "tag a rectangular extrusion as a structural member first".into(),
        });
    }
    let along = |g: &Geometry, axis: DVec3| ((g.end_mm - g.start_mm) / g.length_mm).dot(axis).abs() > 1.0 - 1e-8;
    let (plane, across) = if members.iter().all(|g| along(g, DVec3::X)) {
        (AnalysisPlane::XZ, DVec3::Y)
    } else if members.iter().all(|g| along(g, DVec3::Y)) {
        (AnalysisPlane::YZ, DVec3::X)
    } else {
        return Err("the quick estimate needs level members that all run along X, or all along Y; use analyze_structure for other frames".into());
    };
    let mut tributary = spacing_tributary(&members, across);
    for (body, width) in &case.tributary_mm {
        if members.iter().any(|g| g.body == *body) {
            tributary.insert(*body, *width);
        }
    }
    let (mut supports, mut area_loads) = (Vec::new(), Vec::new());
    for g in &members {
        let ends = g.support_centers_from_start_mm.map_or([0.0, 1.0], |c| c.map(|v| v / g.length_mm));
        for (at, fix) in [(ends[0], [true, true, false]), (ends[1], [false, true, false])] {
            supports.push(Support { location: Location { body: g.body, at }, fix });
        }
        if let Some(&width) = tributary.get(&g.body).filter(|_| case.live_pa + case.dead_pa > 0.0) {
            area_loads.push(AreaLoad { body: g.body, pressure_pa: case.live_pa + case.dead_pa, tributary_width_mm: width, direction: [0.0, -1.0] });
        }
    }
    let request = Analysis {
        plane,
        bodies: Some(members.iter().map(|g| g.body).collect()),
        supports,
        point_loads: Vec::new(),
        uniform_loads: Vec::new(),
        area_loads,
        connections: Vec::new(),
        include_self_weight: case.include_self_weight,
        deformation_scale: case.deformation_scale,
        trial_section_mm: case.trial_section_mm,
    };
    let mut out = analyze(s, &request)?;
    out["case"] = json!(case);
    out["tributary_mm"] = json!(tributary);
    out["skipped"] = json!(skipped);
    Ok(out)
}
