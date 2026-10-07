//! The JSON command API. Everything the app can do to a document is a
//! command here, so the MCP server and the built-in assistant drive the
//! same code as the user interface.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use base64::Engine;
use glam::{DVec2, DVec3};
use serde_json::{Value as J, json};

use crate::doc::{Axis, Blend, Combine, Document, Extrude, FeatureKind, Hole, HoleFit, HoleShape, Op, Pattern, PatternKind, Revolve, Session, Shell, Thread, Transform};
use crate::measure::{self, Item};
use crate::threads;
use crate::exact;
use crate::face::Face;
use crate::expr::{Kind, Value};
use crate::profile::profiles;
use crate::render::{self, Camera};
use crate::sketch::{CKind, Geom, Id, Plane, Sketch};
use crate::units::{Unit, fmt_len, trim_num};
use crate::{io, solver};

/// Given to AI clients so they know what they can send.
pub const REFERENCE: &str = r#"Ferrender is a parametric CAD program: sketches on planes, constrained and dimensioned, turned into solid bodies by extrude and revolve features. Commands are JSON objects with an "op". Z is up.

VALUES. Lengths are numbers in the document's units, or strings with units and arithmetic: 10, "10 mm", "2 in", "$width / 2 + 1cm". Parameters are referenced as $name. Angles are degrees.

WHAT STAYS PARAMETRIC. Dimension constraints and feature values (extrude distance, revolve angle, transform amounts) keep their expressions: change a parameter and they follow. Coordinates in add_geometry only place the geometry once; a "$name" there is evaluated and forgotten. So for a size the user may want to change, draw the shape roughly, then add distance / radius / diameter / angle constraints whose values use the parameter.

DOCUMENT
{"op":"new","units":"mm"}                         units: mm | cm | in
{"op":"set_units","units":"in"}
{"op":"set_parameter","name":"width","expr":"40 mm"}
{"op":"delete_parameter","name":"width"}
{"op":"undo"} {"op":"redo"}
{"op":"save","path":"/abs/part.ferr"} {"op":"open","path":"/abs/part.ferr"}
   In the GUI, new and open refuse to discard unsaved work. Save first or set "discard_unsaved":true on that command to explicitly discard it, including inside a batch.
{"op":"export_stl","path":"/abs/part.stl","units":"mm"}   units default to mm, which is what slicers expect
{"op":"import_stl","path":"/abs/in.stl","units":"mm"}     adds the mesh as a body
{"op":"export_stl","path":"...","union":true}      merges overlapping bodies into one shell first (exact bodies only)
{"op":"export_step","path":"/abs/part.step"}       exact bodies as true surfaces; meshes are skipped
{"op":"batch","commands":[...]}                    runs several commands; stops at the first error
   Requests are limited to 1000 commands including batch containers and 16 nesting levels. A rejected limit applies no commands; an execution error leaves earlier commands applied.
   Inside any command, "$last_sketch", "$last_feature" and "$last_body" stand for the newest sketch, feature and body, so a batch can use what it just made without knowing ids ahead of time.

RESULTS. Commands that change bodies return only "changed_bodies" (and "removed_bodies", "body_count"), not the whole list; use get_scene_info for everything.

BODIES are "exact" (made from sketches: true planes, cylinders and blends) or "mesh" (imported STL, a tapered extrude, or anything combined with a mesh). Only exact bodies can be filleted, chamfered, shelled or written to STEP.

SKETCHES
{"op":"create_sketch","plane":"XY"}                XY (top) | XZ (front) | YZ (right); optional "offset" along the plane normal, or "plane":{"origin":[x,y,z],"normal":[x,y,z],"x":[x,y,z]}
   Returns the sketch id and the plane's origin, x, y and normal in space, so you know which way sketch x and y point. Plane normals: XY is +Z, XZ is -Y (so a positive offset on XZ moves toward -Y), YZ is +X.
{"op":"add_geometry","sketch":ID,"items":[...],"construction":false}
   items: {"type":"line","from":[x,y],"to":[x,y]}
          {"type":"polyline","points":[[x,y],...],"closed":true}
          {"type":"rect","from":[x,y],"to":[x,y]}          gets horizontal/vertical constraints; or "center":[x,y],"size":[w,h]
          {"type":"circle","center":[x,y],"radius":r}      or "diameter"
          {"type":"arc","center":[x,y],"start":[x,y],"end":[x,y]}   counter-clockwise from start to end
          {"type":"ngon","center":[x,y],"radius":r,"sides":6,"rotation":0}   regular polygon, corners on the radius
          {"type":"point","at":[x,y]}
   Endpoints at the same coordinates share one point, so shapes drawn end to end are closed. The sketch origin is point 0 and is fixed. Returns the new entity and point ids for each item.
{"op":"add_constraint","sketch":ID,"kind":KIND,"refs":[ids],"value":V}
   geometric kinds: coincident (2 points | point+line | point+curve), horizontal / vertical (line | 2 points), parallel, perpendicular, collinear (2 lines), tangent (line+curve | 2 curves), equal (2 lines | 2 curves), midpoint (point+line), concentric (2 curves), symmetric (2 points + mirror line), fix (point | line)
   dimension kinds (need "value"): distance (line = its length | 2 points | point+line | 2 parallel lines), radius, diameter (circle or arc), angle (2 lines)
   A constraint that conflicts with existing ones is rejected. Returns its id and the degrees of freedom left (0 = fully constrained).
{"op":"set_dimension","sketch":ID,"constraint":ID,"value":V}
{"op":"delete","sketch":ID,"ids":[...]}            points, entities or constraints
{"op":"move","sketch":ID,"ids":[...],"by":[dx,dy]}  shifts entities in place; features built on them keep working
{"op":"project","sketch":ID,"body":BODY,"point":[x,y,z]}   copies the outline of the body's face nearest that point into the sketch, fixed in place; round outlines become circles
{"op":"trim","sketch":ID,"entity":ID,"near":[x,y]}  removes the stretch of the entity nearest that point, back to where other geometry crosses it
{"op":"mirror","sketch":ID,"ids":[...],"axis":LINE}  mirrored copies, kept symmetric
{"op":"offset","sketch":ID,"ids":[...],"distance":V} parallel copies of lines and circles; positive is outward for a closed outline
{"op":"fillet","sketch":ID,"point":ID,"radius":V}   rounds a corner where two lines meet; {"op":"chamfer",...,"distance":V} cuts it straight

FEATURES
{"op":"extrude","sketch":ID,"distance":V,"operation":"new","symmetric":false,"profiles":[indices]}
   operation: new | join | cut | intersect. Negative distance goes the other way. With "symmetric":true the distance is the total thickness, half each side. A shape drawn inside another in the SAME sketch becomes a hole; shapes in different sketches never do. "profiles" are indices from get_object_info on the sketch; when omitted, every outer region is used and regions nested inside become holes; "all" fills them in.
{"op":"revolve","sketch":ID,"axis":"x","angle":360,"operation":"new","profiles":[...]}
   axis: "x" or "y" (the sketch's axes), the id of a line in the sketch, or {"from":[x,y],"to":[x,y]}. The profile must not cross the axis.
   extrude also takes "extent":"all" (go through every body; the sign of distance picks the side), "taper":DEGREES (walls lean outward, negative inward), and instead of a sketch, "face":{"body":BODY,"point":[x,y,z]} to pull the flat face nearest that point out (or, with a negative distance, push it in and cut).
{"op":"create_sketch","face":{"body":BODY,"point":[x,y,z]}}   sketch on a flat face
{"op":"pattern","feature":ID,"type":"circular","axis":"z","count":6,"angle":360}   repeats an extrude, revolve or import around a world axis through the origin; count includes the original
{"op":"pattern","feature":ID,"type":"linear","axis":"x","count":4,"spacing":V}
{"op":"pattern","feature":ID,"type":"mirror","normal":"x"}   one reflected copy through the origin plane with that normal
{"op":"edit_feature","feature":ID, ...}            any of distance, angle, operation, symmetric, extent, taper, axis, name, suppressed
{"op":"delete_feature","feature":ID}
{"op":"rollback","to":ID}                          shows the model as it was just after that feature ("start" = before any, "end" = everything); features added while rolled back are inserted at that point
{"op":"transform","body":ID,"translate":[x,y,z],"rotate":[rx,ry,rz],"scale":1}   scale about the origin, rotate about X then Y then Z, then translate
{"op":"combine","target":BODY,"tools":[BODY],"operation":"join","keep_tools":false}   join | cut | intersect between bodies, including imported meshes
{"op":"fillet_edges","body":BODY,"edges":[[x,y,z],...],"radius":V}   rounds the edges nearest those points; "edges":"all" takes every edge
{"op":"chamfer_edges","body":BODY,"edges":[[x,y,z],...],"distance":V}
{"op":"shell","body":BODY,"open_faces":[[x,y,z],...],"thickness":V}   hollows the body, leaving the faces nearest those points open
   get_object_info on an exact body lists its edges and faces under "topology", each with a "point" to use here. Edges and faces are found again by position on every rebuild (where they were, or the same place within the body's bounds), so they survive a body changing size but can be lost if an earlier feature reshapes that area; put fillets, chamfers and shells last.
{"op":"hole","body":BODY,"at":[x,y,z],"thread":"M3","fit":"normal","through":true}   drills a hole entering at that point on the body's surface
   at: one point or a list of points (one hole each). direction: [x,y,z] the way the drill goes; left out, it is straight into the flat face at the first point.
   depth: V, or "through":true. type: simple | counterbore | countersink. fit: plain (give "diameter") | close | normal | loose (clearance for the "thread" size) | tapped (drilled at the tap drill size).
   "modeled":true on a tapped hole adds the real thread, for printing; "left_hand":true reverses it. Without it the hole is left at the tap drill size, to be tapped or to let a screw cut its own thread.
   head_diameter, head_depth, head_angle override the counterbore or countersink, which otherwise fit a socket cap or flat head screw of the thread size. tip_angle: 118 gives a drill point. "allowance": V (also "extra") widens every diameter, the modeled thread's included, for printers that make holes small.
   Example, a countersunk M3 clearance hole: {"op":"hole","body":B,"at":[10,10,5],"thread":"M3","type":"countersink","through":true}
{"op":"thread","body":BODY,"face":[x,y,z],"thread":"M6"}   cuts a real thread on the cylinder nearest that point: outside a rod or inside a hole
   "thread" left out picks the catalog size the cylinder was most likely made for. A hole of any size is remade to suit the thread (filled in and re-drilled if it is too wide); a rod wider than the thread is turned down to it. "offset":V and "length":V thread part of it, measured from the cylinder's outer end (a rod's tip, a hole's mouth).
   "allowance":V gives the thread room to turn: a hole's thread is made that much wider across, a rod's that much thinner. Threads are exact without it, and two printed parts at exact sizes will not go together: use about 0.2 mm on each for 3D printing.
   A modeled thread is a closed shell of its own, overlapping the body: slicers join the two, the body stays exact, and STEP shows a plain hole or rod. It does not follow later cuts through it.
{"op":"list_threads"}                              the thread catalog: sizes, pitch, tap drill, clearance, counterbore and countersink sizes
{"op":"set_visible","id":ID,"visible":false}       a sketch or a body

QUERIES
{"op":"get_scene_info"}                            units, parameters, features (with any errors) and bodies
{"op":"measure","from":ITEM,"to":ITEM}             ITEM is {"point":[x,y,z]}, {"body":B,"edge":[x,y,z]} or {"body":B,"face":[x,y,z]} (the edge or face nearest that point)
   "distance" is the shortest between the two, found at "from" and "to". "apart" is the perpendicular gap when they are parallel (two flat faces, two straight edges, a point off a face); "angle" is between straight or flat items.
{"op":"get_object_info","id":ID}                   a sketch (points, entities, constraints, profiles), feature or body
{"op":"get_viewport_screenshot","view":"iso","width":900,"height":650}   view: iso | top | front | right | back | left | bottom | current
   The view is fitted to the bodies. For a custom camera add "azimuth" and "elevation" in degrees (the eye's bearing around Z and height above the XY plane), "target":[x,y,z] to centre on a point, and "zoom" to magnify (2 = twice as close).
{"op":"get_reference"}                             this text

Bodies are named by the id of the feature that created them. Check get_scene_info for feature errors after changes, and look at a screenshot to confirm the shape."#;

type R<T> = Result<T, String>;

fn id_of(c: &J, key: &str) -> R<Id> {
    c[key].as_u64().map(|v| v as Id).ok_or(format!("\"{key}\" should be an id"))
}

fn ids_of(v: &J, key: &str) -> R<Vec<Id>> {
    v[key].as_array().ok_or(format!("\"{key}\" should be a list of ids"))?.iter().map(|x| x.as_u64().map(|v| v as Id).ok_or(format!("\"{key}\" should be a list of ids"))).collect()
}

/// A JSON number or expression string as expression text.
fn text_of(v: &J) -> R<String> {
    match v {
        J::Number(n) => Ok(n.to_string()),
        J::String(s) => Ok(s.clone()),
        _ => Err("expected a number or an expression in quotes".into()),
    }
}

fn mm(doc: &Document, v: &J) -> R<f64> {
    doc.eval(&text_of(v)?, Kind::Length)
}

fn xy(doc: &Document, v: &J) -> R<DVec2> {
    match v.as_array().map(Vec::as_slice) {
        Some([x, y]) => Ok(DVec2::new(mm(doc, x)?, mm(doc, y)?)),
        _ => Err("expected a point as [x, y]".into()),
    }
}

fn xyz(v: &J) -> R<DVec3> {
    match v.as_array().map(Vec::as_slice) {
        Some([x, y, z]) => Ok(DVec3::new(x.as_f64().ok_or("expected numbers")?, y.as_f64().ok_or("expected numbers")?, z.as_f64().ok_or("expected numbers")?)),
        _ => Err("expected [x, y, z]".into()),
    }
}

fn unit_of(c: &J, default: Unit) -> R<Unit> {
    match &c["units"] {
        J::Null => Ok(default),
        J::String(s) => Unit::parse(s).ok_or(format!("unknown units '{s}'; use mm, cm or in")),
        _ => Err("\"units\" should be mm, cm or in".into()),
    }
}

/// The sketch a command names, or the newest one.
fn sketch_id(doc: &Document, c: &J) -> R<Id> {
    let id = match &c["sketch"] {
        J::Null => doc.sketches().last().map(|(f, _)| f.id).ok_or("there is no sketch yet")?,
        _ => id_of(c, "sketch")?,
    };
    doc.sketch(id).map(|_| id).ok_or(format!("feature {id} is not a sketch"))
}

fn sk_mut(doc: &mut Document, id: Id) -> &mut Sketch {
    doc.sketch_mut(id).expect("checked by sketch_id")
}

fn len_out(doc: &Document, mm: f64) -> J {
    json!(fmt_len(mm, doc.units).parse::<f64>().unwrap_or(0.0))
}

fn pt_out(doc: &Document, p: DVec2) -> J {
    json!([len_out(doc, p.x), len_out(doc, p.y)])
}

fn sketch_info(doc: &Document, id: Id) -> J {
    let sk = doc.sketch(id).unwrap();
    let report = solver::solve(&mut sk.clone(), &[]);
    let ents: Vec<J> = sk
        .entities
        .iter()
        .map(|(eid, e)| {
            let mut o = match e.geom {
                Geom::Line { a, b } => json!({"type": "line", "a": a, "b": b, "length": len_out(doc, sk.pos(a).distance(sk.pos(b)))}),
                Geom::Circle { c, r } => json!({"type": "circle", "center": c, "radius": len_out(doc, r)}),
                Geom::Arc { c, s, e } => json!({"type": "arc", "center": c, "start": s, "end": e, "radius": len_out(doc, sk.pos(c).distance(sk.pos(s)))}),
            };
            o["id"] = json!(eid);
            if e.construction {
                o["construction"] = json!(true);
            }
            o
        })
        .collect();
    let cons: Vec<J> = sk
        .constraints
        .iter()
        .map(|(cid, c)| {
            let mut o = json!({"id": cid, "kind": c.kind.name(), "refs": c.refs});
            if let (Some(v), Some(kind)) = (&c.value, c.kind.value_kind()) {
                o["expr"] = json!(v.expr);
                o["value"] = if kind == Kind::Length { len_out(doc, v.v) } else { json!(v.v) };
            }
            o
        })
        .collect();
    let profs: Vec<J> = profiles(sk)
        .iter()
        .enumerate()
        .map(|(i, p)| json!({"index": i, "edges": p.edges, "nested_depth": p.depth, "holes": p.holes.len(), "area": trim_num(p.area() / doc.units.mm().powi(2), 4).parse::<f64>().unwrap_or(0.0), "center": pt_out(doc, p.centroid())}))
        .collect();
    json!({
        "plane": {"origin": sk.plane.origin.to_array(), "x": sk.plane.x.to_array(), "y": sk.plane.y.to_array()},
        "visible": sk.visible,
        "points": sk.points.iter().map(|(i, p)| (i.to_string(), pt_out(doc, *p))).collect::<serde_json::Map<_, _>>(),
        "entities": ents,
        "constraints": cons,
        "degrees_of_freedom": report.dof,
        "fully_constrained": report.ok && report.dof == 0,
        "profiles": profs,
    })
}

fn body_info(s: &Session, id: Id) -> Option<J> {
    let b = s.built.body(id)?;
    let u = s.doc.units.mm();
    let (lo, hi) = b.mesh.bbox().unwrap_or_default();
    let r = |v: f64| trim_num(v, 4).parse::<f64>().unwrap_or(0.0);
    let volume = if b.is_exact() { b.solids.iter().map(|x| x.volume()).sum() } else { b.mesh.volume() };
    Some(json!({
        "id": b.id,
        "name": b.name,
        "visible": !s.doc.hidden_bodies.contains(&id),
        // Exact bodies can be filleted, chamfered, shelled and written to STEP; meshes cannot.
        "kind": if b.is_exact() { "exact" } else { "mesh" },
        "triangles": b.mesh.tris.len(),
        "volume": r(volume / u.powi(3)),
        "surface_area": r(b.mesh.area() / u.powi(2)),
        "min": (lo / u).to_array().map(r),
        "max": (hi / u).to_array().map(r),
        "size": ((hi - lo) / u).to_array().map(r),
        "open_edges": b.mesh.open_edges(),
        // Modeled threads are shells of their own over the body; the volume does not count them.
        "threads": b.threads.len(),
    }))
}

/// The edges and faces of an exact body, each with a point that names it in commands.
fn topology(s: &Session, id: Id) -> Option<J> {
    let b = s.built.body(id).filter(|b| b.is_exact())?;
    let u = s.doc.units.mm();
    let r = |v: f64| trim_num(v, 4).parse::<f64>().unwrap_or(0.0);
    let p = |v: DVec3| (v / u).to_array().map(r);
    let edges: Vec<J> = exact::edges(&b.solids).iter().take(400).map(|e| json!({"point": p(e.mid), "length": r(e.length / u), "shape": if e.straight { "line" } else { "curve" }})).collect();
    let faces: Vec<J> = exact::faces(&b.solids).iter().take(400).map(|f| json!({"point": p(f.at), "normal": f.normal.to_array().map(r), "area": r(f.area / u.powi(2)), "shape": f.kind})).collect();
    Some(json!({"edges": edges, "faces": faces}))
}

/// What each body looked like before a command, to report only what it changed.
fn marks(s: &Session) -> Vec<(Id, u64)> {
    s.built
        .bodies
        .iter()
        .map(|b| {
            let mut mark = DefaultHasher::new();
            b.mesh.tris.len().hash(&mut mark);
            for vertex in b.mesh.tris.iter().flatten() {
                vertex.to_array().map(f64::to_bits).hash(&mut mark);
            }
            // Kernel face IDs contain process-local identities that change on every
            // rebuild. Hash the grouping of triangles, not those transient numbers.
            let mut faces = std::collections::BTreeMap::new();
            for id in &b.mesh.face_ids {
                let next = faces.len();
                faces.entry(*id).or_insert(next).hash(&mut mark);
            }
            for edge in &b.edges {
                edge.len().hash(&mut mark);
                for vertex in edge {
                    vertex.to_array().map(f64::to_bits).hash(&mut mark);
                }
            }
            b.solids.len().hash(&mut mark);
            b.threads.len().hash(&mut mark);
            b.name.hash(&mut mark);
            s.doc.hidden_bodies.contains(&b.id).hash(&mut mark);
            (b.id, mark.finish())
        })
        .collect()
}

fn changed(s: &Session, before: &[(Id, u64)]) -> J {
    let now = marks(s);
    let touched: Vec<J> = now
        .iter()
        .filter(|m| !before.contains(m))
        .filter_map(|m| body_info(s, m.0))
        .collect();
    let removed: Vec<Id> = before
        .iter()
        .map(|m| m.0)
        .filter(|id| !now.iter().any(|m| m.0 == *id))
        .collect();
    json!({"changed_bodies": touched, "removed_bodies": removed, "body_count": now.len()})
}

/// Fills in `$last_sketch`, `$last_feature` and `$last_body`, so a batch can use what it just made.
fn resolve(s: &Session, v: &mut J) {
    match v {
        J::String(t) => {
            let id = match t.as_str() {
                "$last_sketch" => s.doc.sketches().last().map(|(f, _)| f.id),
                "$last_feature" => s.doc.features.last().map(|f| f.id),
                "$last_body" => s.built.bodies.last().map(|b| b.id),
                _ => None,
            };
            if let Some(id) = id {
                *v = json!(id);
            }
        }
        J::Array(list) => list.iter_mut().for_each(|x| resolve(s, x)),
        J::Object(map) => map.values_mut().for_each(|x| resolve(s, x)),
        _ => {}
    }
}

/// Points in space given as [[x, y, z], ...] in document units.
fn points_of(s: &Session, v: &J, key: &str) -> R<Vec<DVec3>> {
    v[key].as_array().ok_or(format!("\"{key}\" should be a list of [x, y, z] points"))?.iter().map(|p| xyz(p).map(|p| p * s.doc.units.mm())).collect()
}

fn feature_info(s: &Session, id: Id) -> R<J> {
    let doc = &s.doc;
    let f = doc.feature(id).ok_or(format!("nothing has id {id}"))?;
    let mut o = json!({"id": f.id, "name": f.name, "type": f.type_name()});
    if f.suppressed {
        o["suppressed"] = json!(true);
    }
    if let Some(e) = s.built.errors.get(&id) {
        o["error"] = json!(e);
    }
    if doc.features.iter().position(|x| x.id == id).is_some_and(|i| i >= doc.active()) {
        o["rolled_back"] = json!(true);
    }
    let axis = |a: Axis| match a {
        Axis::X => json!("x"),
        Axis::Y => json!("y"),
        Axis::Line(l) => json!(l),
    };
    match &f.kind {
        FeatureKind::Sketch(_) => {}
        FeatureKind::Extrude(e) => {
            o["sketch"] = json!(e.sketch);
            o["distance"] = json!({"expr": e.distance.expr, "value": len_out(doc, e.distance.v)});
            o["symmetric"] = json!(e.symmetric);
            o["operation"] = json!(e.op.name());
            o["extent"] = json!(if e.through_all { "all" } else { "distance" });
            if let Some(t) = &e.taper {
                o["taper"] = json!({"expr": t.expr, "value": t.v});
            }
        }
        FeatureKind::Revolve(r) => {
            o["sketch"] = json!(r.sketch);
            o["angle"] = json!({"expr": r.angle.expr, "value": r.angle.v});
            o["axis"] = axis(r.axis);
            o["operation"] = json!(r.op.name());
        }
        FeatureKind::Import(m) => o["triangles"] = json!(m.tris.len()),
        FeatureKind::Transform(t) => {
            o["body"] = json!(t.body);
            o["translate"] = json!(t.translate.iter().map(|v| v.expr.clone()).collect::<Vec<_>>());
            o["rotate"] = json!(t.rotate.iter().map(|v| v.expr.clone()).collect::<Vec<_>>());
            o["scale"] = json!(t.scale.expr);
        }
        FeatureKind::Pattern(p) => {
            o["feature"] = json!(p.source);
            let name = |a: &usize| ["x", "y", "z"].get(*a).copied().unwrap_or("?");
            match &p.kind {
                PatternKind::Circular { axis, count, angle } => o["pattern"] = json!({"type": "circular", "axis": name(axis), "count": count, "angle": angle.expr}),
                PatternKind::Linear { axis, count, spacing } => o["pattern"] = json!({"type": "linear", "axis": name(axis), "count": count, "spacing": spacing.expr}),
                PatternKind::Mirror { axis } => o["pattern"] = json!({"type": "mirror", "normal": name(axis)}),
            }
        }
        FeatureKind::Blend(b) => {
            o["body"] = json!(b.body);
            o["edges"] = json!(b.edges.len());
            o[if b.chamfer { "distance" } else { "radius" }] = json!({"expr": b.size.expr, "value": len_out(doc, b.size.v)});
        }
        FeatureKind::Shell(sh) => {
            o["body"] = json!(sh.body);
            o["open_faces"] = json!(sh.faces.len());
            o["thickness"] = json!({"expr": sh.thickness.expr, "value": len_out(doc, sh.thickness.v)});
        }
        FeatureKind::Hole(h) => {
            let u = doc.units.mm();
            o["body"] = json!(h.body);
            o["at"] = json!(h.at.iter().map(|p| (*p / u).to_array()).collect::<Vec<_>>());
            o["direction"] = json!(h.dir.to_array());
            o["hole_type"] = json!(h.shape.name());
            o["fit"] = json!(h.fit.name());
            if !h.thread.is_empty() {
                o["thread"] = json!(h.thread);
            }
            o["modeled"] = json!(h.modeled);
            o["depth"] = h.depth.as_ref().map_or(json!("through"), |d| json!({"expr": d.expr, "value": len_out(doc, d.v)}));
            if let Ok(z) = h.sizes() {
                o["diameter"] = len_out(doc, z.diameter);
                match z.head {
                    Some(exact::DrillHead::Counterbore { diameter, depth }) => o["counterbore"] = json!({"diameter": len_out(doc, diameter), "depth": len_out(doc, depth)}),
                    Some(exact::DrillHead::Countersink { diameter, angle }) => o["countersink"] = json!({"diameter": len_out(doc, diameter), "angle": angle}),
                    None => {}
                }
            }
        }
        FeatureKind::Thread(t) => {
            o["body"] = json!(t.body);
            o["thread"] = json!(t.thread);
            o["left_hand"] = json!(t.left);
            if let Some(e) = &t.extra {
                o["allowance"] = json!(e.v / doc.units.mm());
            }
            if let Some(l) = &t.length {
                o["length"] = json!({"expr": l.expr, "value": len_out(doc, l.v)});
            }
        }
        FeatureKind::Combine(c) => {
            o["target"] = json!(c.target);
            o["tools"] = json!(c.tools);
            o["operation"] = json!(c.op.name());
        }
    }
    Ok(o)
}

fn scene_info(s: &Session) -> J {
    let doc = &s.doc;
    json!({
        "units": doc.units.name(),
        "file": s.path.as_ref().map(|p| p.display().to_string()),
        "parameters": doc.params.iter().map(|p| json!({"name": p.name, "expr": p.expr, "value": doc.show_param(&p.name)})).collect::<Vec<_>>(),
        "features": doc.features.iter().filter_map(|f| feature_info(s, f.id).ok()).collect::<Vec<_>>(),
        "bodies": s.built.bodies.iter().filter_map(|b| body_info(s, b.id)).collect::<Vec<_>>(),
    })
}

fn add_item(doc: &mut Document, sid: Id, item: &J, construction: bool) -> R<J> {
    const TOL: f64 = 1e-6;
    let p = |doc: &Document, key: &str| xy(doc, &item[key]).map_err(|e| format!("\"{key}\": {e}"));
    let kind = item["type"].as_str().ok_or("each item needs a \"type\"")?;
    let construction = item["construction"].as_bool().unwrap_or(construction);
    let mut ents = Vec::new();
    let mut pts = Vec::new();
    match kind {
        "line" => {
            let (a, b) = (p(doc, "from")?, p(doc, "to")?);
            if a.distance(b) < TOL {
                return Err("a line needs two different points".into());
            }
            let sk = sk_mut(doc, sid);
            pts = vec![sk.point_at(a, TOL), sk.point_at(b, TOL)];
            ents.push(sk.add(Geom::Line { a: pts[0], b: pts[1] }, construction));
        }
        "polyline" | "polygon" => {
            let list = item["points"].as_array().ok_or("a polyline needs \"points\"")?;
            let ps: Vec<DVec2> = list.iter().map(|v| xy(doc, v)).collect::<R<_>>()?;
            let closed = item["closed"].as_bool().unwrap_or(kind == "polygon");
            if ps.len() < 2 || (closed && ps.len() < 3) {
                return Err("not enough points".into());
            }
            let sk = sk_mut(doc, sid);
            pts = ps.iter().map(|q| sk.point_at(*q, TOL)).collect();
            let n = if closed { pts.len() } else { pts.len() - 1 };
            for i in 0..n {
                let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                if a != b {
                    ents.push(sk.add(Geom::Line { a, b }, construction));
                }
            }
        }
        "rect" | "rectangle" => {
            let (a, c) = match (&item["center"], item["size"].as_array().map(Vec::as_slice)) {
                (J::Null, _) => (p(doc, "from")?, p(doc, "to")?),
                (_, Some([w, h])) => {
                    let (mid, half) = (p(doc, "center")?, DVec2::new(mm(doc, w)?, mm(doc, h)?) / 2.0);
                    (mid - half, mid + half)
                }
                _ => return Err("a centred rect needs \"size\": [width, height]".into()),
            };
            if (a.x - c.x).abs() < TOL || (a.y - c.y).abs() < TOL {
                return Err("a rectangle needs width and height".into());
            }
            let sk = sk_mut(doc, sid);
            let (pa, pc) = (sk.point_at(a, TOL), sk.point_at(c, TOL));
            ents = sk.add_rect(pa, pc, construction);
            pts = ents.iter().map(|l| sk.ent_points(*l)[0]).collect();
        }
        "circle" => {
            let c = p(doc, "center")?;
            let r = match (&item["radius"], &item["diameter"]) {
                (J::Null, J::Null) => return Err("a circle needs a \"radius\" or \"diameter\"".into()),
                (J::Null, d) => mm(doc, d)? / 2.0,
                (r, _) => mm(doc, r)?,
            };
            if r <= TOL {
                return Err("the radius must be positive".into());
            }
            let sk = sk_mut(doc, sid);
            pts = vec![sk.point_at(c, TOL)];
            ents.push(sk.add(Geom::Circle { c: pts[0], r }, construction));
        }
        "arc" => {
            let (c, s, e) = (p(doc, "center")?, p(doc, "start")?, p(doc, "end")?);
            if c.distance(s) < TOL {
                return Err("the arc's start is on its centre".into());
            }
            // The end is put on the arc's circle, at the bearing given.
            let e = c + (e - c).normalize_or(DVec2::X) * c.distance(s);
            let sk = sk_mut(doc, sid);
            pts = vec![sk.point_at(c, TOL), sk.point_at(s, TOL), sk.point_at(e, TOL)];
            ents.push(sk.add(Geom::Arc { c: pts[0], s: pts[1], e: pts[2] }, construction));
        }
        "ngon" => {
            let c = p(doc, "center")?;
            let sides = item["sides"].as_u64().filter(|n| (3..=64).contains(n)).ok_or("an ngon needs \"sides\" between 3 and 64")? as usize;
            let r = mm(doc, &item["radius"]).map_err(|_| "an ngon needs a \"radius\"")?;
            if r <= TOL {
                return Err("the radius must be positive".into());
            }
            let turn = item["rotation"].as_f64().unwrap_or(0.0).to_radians();
            let sk = sk_mut(doc, sid);
            let centre = sk.point_at(c, TOL);
            ents = sk.add_polygon(centre, c + DVec2::from_angle(turn) * r, sides, construction);
            pts = ents.iter().map(|l| sk.ent_points(*l)[0]).collect();
        }
        "point" => {
            let at = p(doc, "at")?;
            pts.push(sk_mut(doc, sid).point_at(at, TOL));
        }
        other => return Err(format!("unknown item type '{other}'")),
    }
    Ok(json!({"entities": ents, "points": pts}))
}

/// Solves a sketch after a change, failing if its constraints now conflict.
fn settle(doc: &mut Document, sid: Id) -> R<solver::Report> {
    let report = solver::solve(sk_mut(doc, sid), &[]);
    if report.ok { Ok(report) } else { Err("that conflicts with the sketch's other constraints".into()) }
}

/// Which profiles of a sketch a feature command selects, as boundary edge sets.
fn pick_profiles(doc: &Document, sid: Id, c: &J) -> R<Vec<Vec<Id>>> {
    let all = profiles(doc.sketch(sid).unwrap());
    if all.is_empty() {
        return Err("the sketch has no closed profile".into());
    }
    match &c["profiles"] {
        J::Null => Ok(all.iter().filter(|p| p.depth % 2 == 0).map(|p| p.edges.clone()).collect()),
        J::String(s) if s == "all" => Ok(all.iter().map(|p| p.edges.clone()).collect()),
        J::Array(list) => list.iter().map(|v| v.as_u64().and_then(|i| all.get(i as usize)).map(|p| p.edges.clone()).ok_or(format!("\"profiles\" should be indices below {}", all.len()))).collect(),
        _ => Err("\"profiles\" should be a list of indices or \"all\"".into()),
    }
}

fn op_of(c: &J, default: Op) -> R<Op> {
    match &c["operation"] {
        J::Null => Ok(default),
        J::String(s) => Op::parse(s).ok_or(format!("unknown operation '{s}'; use new, join, cut or intersect")),
        _ => Err("\"operation\" should be new, join, cut or intersect".into()),
    }
}

fn axis_of(doc: &Document, sid: Id, v: &J) -> R<Axis> {
    match v {
        J::Null => Ok(Axis::Y),
        J::String(s) if s.eq_ignore_ascii_case("x") => Ok(Axis::X),
        J::String(s) if s.eq_ignore_ascii_case("y") => Ok(Axis::Y),
        J::Number(n) => {
            let id = n.as_u64().unwrap_or(u64::MAX) as Id;
            doc.sketch(sid).and_then(|s| s.line(id)).map(|_| Axis::Line(id)).ok_or(format!("{id} is not a line in the sketch"))
        }
        _ => Err("\"axis\" should be \"x\", \"y\" or a line id".into()),
    }
}

/// The face a command names with {"body": ID, "point": [x, y, z]}: the one nearest the point.
fn face_of(s: &Session, v: &J) -> R<Face> {
    let id = id_of(v, "body")?;
    let body = s.built.body(id).ok_or(format!("there is no body {id}"))?;
    let p = xyz(&v["point"]).map_err(|e| format!("\"point\": {e}"))? * s.doc.units.mm();
    Face::near(body, p).ok_or("the body has no faces".to_owned())
}

fn zero(doc: &Document, kind: Kind) -> Value {
    doc.value("0", kind).expect("zero is always valid")
}

/// Runs one command. `cam` is the app's current view, for screenshots.
pub fn execute(s: &mut Session, c: &J, cam: Option<Camera>) -> R<J> {
    validate_request(c)?;
    execute_validated(s, c, cam)
}

/// Check a complete request before any command is applied, including headless batches.
pub fn validate_request(c: &J) -> R<()> {
    let mut pending = vec![(c, 0)];
    let mut count = 0;
    while let Some((command, depth)) = pending.pop() {
        count += 1;
        if count > 1000 || depth > 16 {
            return Err("Automation is limited to 1000 commands and 16 nested batches per request.".into());
        }
        if command["op"] == "batch" {
            let commands = command["commands"].as_array().ok_or("batch needs \"commands\"")?;
            if commands.len() > 1000 { return Err("Automation is limited to 1000 commands per request.".into()); }
            pending.extend(commands.iter().map(|c| (c, depth + 1)));
        }
    }
    Ok(())
}

fn execute_validated(s: &mut Session, c: &J, cam: Option<Camera>) -> R<J> {
    let op = c["op"].as_str().ok_or("the command needs an \"op\"")?;
    // A batch resolves each command as it reaches it, after the ones before have run.
    let mut filled = c.clone();
    if op != "batch" {
        resolve(s, &mut filled);
    }
    let c = &filled;
    let before = marks(s);
    match op {
        "get_scene_info" => Ok(scene_info(s)),
        "get_object_info" => {
            let id = id_of(c, "id")?;
            let mut o = feature_info(s, id)?;
            if s.doc.sketch(id).is_some() {
                o["sketch"] = sketch_info(&s.doc, id);
            }
            if let Some(b) = body_info(s, id) {
                o["body"] = b;
            }
            if let Some(t) = topology(s, id) {
                o["topology"] = t;
            }
            Ok(o)
        }
        "get_viewport_screenshot" => {
            let w = c["width"].as_u64().unwrap_or(900).clamp(64, 2400) as usize;
            let h = c["height"].as_u64().unwrap_or(650).clamp(64, 2400) as usize;
            let view = c["view"].as_str().unwrap_or(if cam.is_some() { "current" } else { "iso" });
            let custom = !c["azimuth"].is_null() || !c["elevation"].is_null();
            let cam = match (view, cam) {
                ("current", Some(cam)) if !custom => Some(cam),
                ("current", None) if !custom => None,
                (name, _) => {
                    let (mut yaw, mut pitch) = Camera::named(if name == "current" { "iso" } else { name }).ok_or(format!("unknown view '{name}'"))?;
                    if custom {
                        yaw = c["azimuth"].as_f64().unwrap_or(yaw.to_degrees()).to_radians();
                        pitch = c["elevation"].as_f64().unwrap_or(pitch.to_degrees()).clamp(-90.0, 90.0).to_radians();
                    }
                    let mut cam = Camera { yaw, pitch, ..Camera::iso() };
                    // Frame the bodies; sketches only count when there is nothing else to look at.
                    let bodies = s.visible_bodies().filter_map(|b| b.mesh.bbox()).reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)));
                    if let Some((lo, hi)) = bodies.or(render::scene_bounds(s)) {
                        cam.fit(lo, hi, w as f64, h as f64);
                    }
                    if !c["target"].is_null() {
                        cam.target = xyz(&c["target"])? * s.doc.units.mm();
                    }
                    cam.scale *= c["zoom"].as_f64().unwrap_or(1.0).clamp(0.05, 100.0);
                    Some(cam)
                }
            };
            let png = render::snapshot(s, cam, w, h).png();
            Ok(json!({"width": w, "height": h, "png_base64": base64::engine::general_purpose::STANDARD.encode(png)}))
        }
        "batch" => {
            let list = c["commands"].as_array().ok_or("batch needs \"commands\"")?;
            let mut out = Vec::new();
            for (i, cmd) in list.iter().enumerate() {
                out.push(execute_validated(s, cmd, cam).map_err(|e| format!("command {i} ({}) failed: {e}. The {i} before it were applied.", cmd["op"].as_str().unwrap_or("?")))?);
            }
            Ok(json!(out))
        }
        "undo" => Ok(json!({"undone": s.undo()})),
        "redo" => Ok(json!({"redone": s.redo()})),
        "new" => {
            let units = unit_of(c, Unit::Mm)?;
            *s = Session::new(Document::new(units));
            Ok(json!({"units": units.name()}))
        }
        "open" => {
            let path = c["path"].as_str().ok_or("open needs a \"path\"")?;
            *s = Session::open(path.as_ref())?;
            Ok(scene_info(s))
        }
        "save" => {
            let path = c["path"].as_str().map(std::path::PathBuf::from).or(s.path.clone()).ok_or("save needs a \"path\"")?;
            s.save(&path)?;
            Ok(json!({"saved": path.display().to_string()}))
        }
        "export_stl" => {
            let path = c["path"].as_str().ok_or("export_stl needs a \"path\"")?;
            let unit = unit_of(c, Unit::Mm)?;
            let only = match &c["bodies"] {
                J::Null => None,
                _ => Some(ids_of(c, "bodies")?),
            };
            let picked: Vec<&crate::doc::Body> = s.visible_bodies().filter(|b| only.as_ref().is_none_or(|o| o.contains(&b.id))).collect();
            if c["union"].as_bool().unwrap_or(false) && picked.len() > 1 {
                // Overlapping bodies become one shell, as a slicer wants.
                if !picked.iter().all(|b| b.is_exact()) {
                    return Err("\"union\" needs every exported body to be exact; one of these is a mesh".into());
                }
                let mut all = picked[0].solids.clone();
                for b in &picked[1..] {
                    all = exact::boolean(&all, &b.solids, crate::csg::Bool::Union)?;
                }
                let mut mesh = exact::tessellate(&all)?.0;
                // Threads are shells of their own and go along as they are.
                mesh.tris.extend(picked.iter().flat_map(|b| b.threads.iter().flat_map(|t| t.tris.iter().copied())));
                mesh.face_ids.clear();
                let merged = crate::doc::Body { id: 0, name: "union".into(), mesh, solids: Vec::new(), edges: Vec::new(), threads: Vec::new(), plain: 0 };
                let n = io::write_stl([&merged], unit, path.as_ref())?;
                return Ok(json!({"path": path, "triangles": n, "units": unit.name(), "shells": all.len(), "open_edges": merged.mesh.open_edges()}));
            }
            let n = io::write_stl(picked, unit, path.as_ref())?;
            Ok(json!({"path": path, "triangles": n, "units": unit.name()}))
        }
        "export_step" => {
            let path = c["path"].as_str().ok_or("export_step needs a \"path\"")?;
            let only = match &c["bodies"] {
                J::Null => None,
                _ => Some(ids_of(c, "bodies")?),
            };
            let picked: Vec<&crate::doc::Body> = s.visible_bodies().filter(|b| only.as_ref().is_none_or(|o| o.contains(&b.id))).collect();
            let skipped: Vec<Id> = picked.iter().filter(|b| !b.is_exact()).map(|b| b.id).collect();
            let solids: Vec<_> = picked.iter().flat_map(|b| &b.solids).collect();
            if solids.is_empty() {
                return Err("there are no exact bodies to write; STEP cannot hold meshes".into());
            }
            let bytes = exact::step(solids.iter().copied())?;
            std::fs::write(path, &bytes).map_err(|e| format!("could not write {path}: {e}"))?;
            Ok(json!({"path": path, "solids": solids.len(), "bytes": bytes.len(), "skipped_mesh_bodies": skipped}))
        }
        "get_reference" => Ok(json!(REFERENCE)),
        "rollback" => {
            // After a feature id, or to the "start" or "end" of the timeline.
            let count = match &c["to"] {
                J::String(t) if t == "start" => 0,
                J::Null => s.doc.features.len(),
                J::String(t) if t == "end" => s.doc.features.len(),
                _ => {
                    let id = id_of(c, "to")?;
                    s.doc.features.iter().position(|f| f.id == id).ok_or(format!("there is no feature {id}"))? + 1
                }
            };
            s.edit(|d| {
                d.roll_to(count);
                Ok(())
            })?;
            let mut out = changed(s, &before);
            out["built_features"] = json!(s.doc.active());
            out["total_features"] = json!(s.doc.features.len());
            Ok(out)
        }
        "fillet_edges" | "chamfer_edges" => {
            let body = id_of(c, "body")?;
            let chamfer = op == "chamfer_edges";
            let key = if chamfer { "distance" } else { "radius" };
            let size = s.doc.value(&text_of(&c[key]).map_err(|_| format!("{op} needs a \"{key}\""))?, Kind::Length)?;
            let edges = match &c["edges"] {
                J::String(all) if all == "all" => s.built.body(body).map(|b| exact::edges(&b.solids).iter().map(|e| e.mid).collect()).ok_or(format!("there is no body {body}"))?,
                _ => points_of(s, c, "edges")?,
            };
            let frame = s.built.frame(body);
            let id = s.edit_feature(|d| {
                let id = d.add_feature(FeatureKind::Blend(Blend { body, edges, size, chamfer, frame }));
                Ok((id, id))
            })?;
            let mut out = changed(s, &before);
            out["feature"] = json!(id);
            Ok(out)
        }
        "shell" => {
            let body = id_of(c, "body")?;
            let thickness = s.doc.value(&text_of(&c["thickness"]).map_err(|_| "shell needs a \"thickness\"")?, Kind::Length)?;
            let faces = points_of(s, c, "open_faces")?;
            let frame = s.built.frame(body);
            let id = s.edit_feature(|d| {
                let id = d.add_feature(FeatureKind::Shell(Shell { body, faces, thickness, frame }));
                Ok((id, id))
            })?;
            let mut out = changed(s, &before);
            out["feature"] = json!(id);
            Ok(out)
        }
        "measure" => {
            let u = s.doc.units.mm();
            let item = |key: &str| -> R<Item> {
                let v = &c[key];
                let at = |k: &str| xyz(&v[k]).map(|p| p * u).map_err(|e| format!("\"{key}\".\"{k}\": {e}"));
                if v["body"].is_null() {
                    return at("point").map(Item::Point).map_err(|_| format!("\"{key}\" should be {{\"point\":[x,y,z]}}, or a body with an \"edge\" or \"face\" point"));
                }
                let id = id_of(v, "body")?;
                let b = s.built.body(id).ok_or(format!("there is no body {id}"))?;
                if !v["edge"].is_null() {
                    let p = at("edge")?;
                    let near = |e: &Vec<DVec3>| e.windows(2).map(|w| (w[0] + (w[1] - w[0]) * ((p - w[0]).dot(w[1] - w[0]) / (w[1] - w[0]).length_squared().max(1e-18)).clamp(0.0, 1.0)).distance(p)).fold(f64::MAX, f64::min);
                    return b.edges.iter().min_by(|x, y| near(x).total_cmp(&near(y))).map(|e| Item::Path(e.clone())).ok_or("that body is a mesh and has no edges to measure; use a face or a point".into());
                }
                let face = Face::near(b, at("face")?).ok_or("the body has no faces")?;
                Ok(Item::Surface(face.tris.iter().map(|t| b.mesh.tris[*t]).collect()))
            };
            let m = measure::between(&item("from")?, &item("to")?);
            let r = |v: f64| trim_num(v / u, 4).parse::<f64>().unwrap_or(0.0);
            let mut out = json!({"units": s.doc.units.name(), "distance": r(m.distance), "from": (m.from / u).to_array().map(|v| r(v * u)), "to": (m.to / u).to_array().map(|v| r(v * u)), "delta": ((m.to - m.from) / u).to_array().map(|v| r(v * u))});
            if let Some(a) = m.apart {
                out["apart"] = json!(r(a));
            }
            if let Some(a) = m.angle {
                out["angle"] = json!(trim_num(a, 4).parse::<f64>().unwrap_or(0.0));
            }
            Ok(out)
        }
        "list_threads" => {
            let u = s.doc.units.mm();
            let r = |v: f64| trim_num(v / u, 4).parse::<f64>().unwrap_or(0.0);
            let list: Vec<J> = threads::CATALOG
                .iter()
                .map(|t| {
                    let (sink, angle) = t.countersink();
                    json!({"thread": t.name, "family": t.family.label(), "coarse": t.coarse, "major": r(t.major), "pitch": r(t.pitch), "tap_drill": r(t.tap_drill),
                        "clearance": {"close": r(t.clearance[0]), "normal": r(t.clearance[1]), "loose": r(t.clearance[2])},
                        "counterbore": {"diameter": r(t.counterbore), "depth": r(t.counterbore_depth)}, "countersink": {"diameter": r(sink), "angle": angle}})
                })
                .collect();
            Ok(json!({"units": s.doc.units.name(), "threads": list}))
        }
        "hole" => {
            let body = id_of(c, "body")?;
            let at = match xyz(&c["at"]) {
                Ok(p) => vec![p * s.doc.units.mm()],
                Err(_) => points_of(s, c, "at").map_err(|_| "hole needs \"at\": a point [x, y, z] on the body, or a list of them")?,
            };
            let first = *at.first().ok_or("hole needs at least one point in \"at\"")?;
            let dir = match &c["direction"] {
                J::Null => {
                    let b = s.built.body(body).ok_or(format!("there is no body {body}"))?;
                    let face = Face::near(b, first).ok_or("the body has no faces")?;
                    -face.plane.ok_or("the face at that point is not flat, so the hole needs a \"direction\": [x, y, z]")?.normal()
                }
                v => xyz(v).map_err(|e| format!("\"direction\": {e}"))?,
            };
            let word = |key: &str| c[key].as_str().map(str::to_owned);
            let shape = match word("type") {
                None => HoleShape::Simple,
                Some(t) => HoleShape::ALL.into_iter().find(|h| h.name() == t).ok_or("\"type\" should be simple, counterbore or countersink")?,
            };
            let thread = match word("thread") {
                Some(t) => threads::find(&t)?.name.to_owned(),
                None => String::new(),
            };
            let fit = match word("fit") {
                Some(t) => HoleFit::ALL.into_iter().find(|h| h.name() == t).ok_or("\"fit\" should be plain, close, normal, loose or tapped")?,
                // A thread size with nothing said means a hole the screw passes through.
                None if thread.is_empty() || !c["diameter"].is_null() => HoleFit::Plain,
                None => HoleFit::Normal,
            };
            let opt = |key: &str, kind: Kind| -> R<Option<Value>> {
                match &c[key] {
                    J::Null => Ok(None),
                    v => Ok(Some(s.doc.value(&text_of(v).map_err(|e| format!("\"{key}\": {e}"))?, kind)?)),
                }
            };
            let through = c["through"].as_bool().unwrap_or(false);
            let depth = if through { None } else { Some(opt("depth", Kind::Length)?.ok_or("hole needs a \"depth\", or \"through\": true")?) };
            let hole = Hole {
                body,
                at,
                dir,
                shape,
                fit,
                thread,
                diameter: opt("diameter", Kind::Length)?,
                depth,
                tip_angle: opt("tip_angle", Kind::Angle)?,
                head_diameter: opt("head_diameter", Kind::Length)?,
                head_depth: opt("head_depth", Kind::Length)?,
                head_angle: opt("head_angle", Kind::Angle)?,
                modeled: c["modeled"].as_bool().unwrap_or(false),
                left: c["left_hand"].as_bool().unwrap_or(false),
                extra: match opt("allowance", Kind::Length)? {
                    None => opt("extra", Kind::Length)?,
                    some => some,
                },
            };
            let sizes = hole.sizes()?;
            let id = s.edit_feature(|d| {
                let id = d.add_feature(FeatureKind::Hole(hole));
                Ok((id, id))
            })?;
            let mut out = changed(s, &before);
            out["feature"] = json!(id);
            out["diameter"] = len_out(&s.doc, sizes.diameter);
            Ok(out)
        }
        "thread" => {
            let body = id_of(c, "body")?;
            let face = xyz(&c["face"]).map_err(|_| "thread needs \"face\": a point [x, y, z] on the cylinder")? * s.doc.units.mm();
            let b = s.built.body(body).ok_or(format!("there is no body {body}"))?;
            if !b.is_exact() {
                return Err("this body is a mesh, so its cylinders cannot be found; thread it before anything else turns it into a mesh, or use a tapped hole".into());
            }
            let barrel = exact::barrel(&b.solids, &[face, face])?;
            let thread = match c["thread"].as_str() {
                Some(t) => threads::find(t)?,
                None => threads::nearest(barrel.radius * 2.0, barrel.internal),
            }
            .name
            .to_owned();
            let opt = |key: &str| -> R<Option<Value>> {
                match &c[key] {
                    J::Null => Ok(None),
                    v => Ok(Some(s.doc.value(&text_of(v).map_err(|e| format!("\"{key}\": {e}"))?, Kind::Length)?)),
                }
            };
            let t = Thread { body, face, frame: s.built.frame(body), thread: thread.clone(), offset: opt("offset")?, length: opt("length")?, left: c["left_hand"].as_bool().unwrap_or(false), extra: opt("allowance")? };
            let id = s.edit_feature(|d| {
                let id = d.add_feature(FeatureKind::Thread(t));
                Ok((id, id))
            })?;
            let mut out = changed(s, &before);
            out["feature"] = json!(id);
            out["thread"] = json!(thread);
            out["internal"] = json!(barrel.internal);
            Ok(out)
        }
        "move" => {
            let sid = sketch_id(&s.doc, c)?;
            let ids = ids_of(c, "ids")?;
            let by = xy(&s.doc, &c["by"]).map_err(|e| format!("\"by\": {e}"))?;
            s.edit(|d| {
                sk_mut(d, sid).translate(&ids, by);
                let report = settle(d, sid)?;
                Ok(json!({"moved": ids, "degrees_of_freedom": report.dof}))
            })?;
            let mut out = changed(s, &before);
            out["feature_errors"] = json!(s.built.errors);
            Ok(out)
        }
        "set_units" => {
            let units = unit_of(c, s.doc.units)?;
            s.edit(|d| {
                d.units = units;
                Ok(json!({"units": units.name()}))
            })
        }
        "set_parameter" => {
            let name = c["name"].as_str().ok_or("set_parameter needs a \"name\"")?.trim_start_matches('$').to_owned();
            let expr = text_of(&c["expr"]).or(text_of(&c["value"])).map_err(|_| "set_parameter needs an \"expr\"")?;
            s.edit(|d| d.set_param(&name, &expr))?;
            let broken: Vec<_> = s.built.errors.values().cloned().collect();
            if !broken.is_empty() {
                s.abort();
                return Err(format!("that value breaks the model: {}", broken.join("; ")));
            }
            Ok(json!({"name": name, "value": s.doc.show_param(&name)}))
        }
        "delete_parameter" => {
            let name = c["name"].as_str().ok_or("delete_parameter needs a \"name\"")?.trim_start_matches('$').to_owned();
            s.edit(|d| {
                let n = d.params.len();
                d.params.retain(|p| p.name != name);
                if d.params.len() == n { Err(format!("no parameter named '{name}'")) } else { Ok(()) }
            })?;
            if let Some(e) = s.built.errors.values().next().cloned() {
                s.abort();
                return Err(format!("'{name}' is still in use: {e}"));
            }
            Ok(json!({"deleted": name}))
        }
        "create_sketch" => {
            let plane = match &c["plane"] {
                J::Null if !c["face"].is_null() => face_of(s, &c["face"])?.plane.ok_or("sketches need a flat face")?,
                J::Null => Plane::XY,
                J::String(p) => match p.to_ascii_uppercase().as_str() {
                    "XY" | "TOP" => Plane::XY,
                    "XZ" | "FRONT" => Plane::XZ,
                    "YZ" | "RIGHT" => Plane::YZ,
                    _ => return Err(format!("unknown plane '{p}'; use XY, XZ or YZ")),
                },
                o => {
                    let n = xyz(&o["normal"])?;
                    if n.length() < 1e-9 {
                        return Err("the plane normal has no length".into());
                    }
                    let origin = match &o["origin"] {
                        J::Null => DVec3::ZERO,
                        v => xyz(v)? * s.doc.units.mm(),
                    };
                    let plane = Plane::from_normal(origin, n);
                    match &o["x"] {
                        J::Null => plane,
                        v => {
                            // The asked-for x direction, flattened into the plane.
                            let n = plane.normal();
                            let x = xyz(v)?.reject_from(n).try_normalize().ok_or("the plane's \"x\" must not be parallel to its normal")?;
                            Plane { origin, x, y: n.cross(x) }
                        }
                    }
                }
            };
            let plane = match &c["offset"] {
                J::Null => plane,
                v => plane.offset(mm(&s.doc, v)?),
            };
            let name = c["name"].as_str().map(str::to_owned);
            s.edit(|d| {
                let id = d.add_feature(FeatureKind::Sketch(Sketch::new(plane)));
                if let Some(n) = name {
                    d.feature_mut(id).unwrap().name = n;
                }
                let u = d.units.mm();
                let tidy = |v: DVec3| v.to_array().map(|c| (c * 1e9).round() / 1e9 + 0.0);
                Ok(json!({"sketch": id, "origin": tidy(plane.origin / u), "x": tidy(plane.x), "y": tidy(plane.y), "normal": tidy(plane.normal())}))
            })
        }
        "add_geometry" => {
            let sid = sketch_id(&s.doc, c)?;
            let items = c["items"].as_array().ok_or("add_geometry needs \"items\"")?.clone();
            let construction = c["construction"].as_bool().unwrap_or(false);
            s.edit(|d| {
                let out: Vec<J> = items.iter().enumerate().map(|(i, it)| add_item(d, sid, it, construction).map_err(|e| format!("item {i}: {e}"))).collect::<R<_>>()?;
                let report = settle(d, sid)?;
                Ok(json!({"sketch": sid, "items": out, "degrees_of_freedom": report.dof}))
            })
        }
        "add_constraint" => {
            let sid = sketch_id(&s.doc, c)?;
            let kind = c["kind"].as_str().and_then(CKind::parse).ok_or("unknown constraint \"kind\"")?;
            let refs = ids_of(c, "refs")?;
            let value = c["value"].clone();
            s.edit(|d| {
                let refs = d.sketch(sid).unwrap().normalize(kind, &refs)?;
                let value = match (kind.value_kind(), &value) {
                    (None, _) => None,
                    (Some(k), J::Null) => {
                        let now = d.sketch(sid).unwrap().measure(kind, &refs);
                        Some(d.value(&if k == Kind::Length { format!("{now} mm") } else { now.to_string() }, k)?)
                    }
                    (Some(k), v) => Some(d.enter(&text_of(v)?, k)?),
                };
                let id = sk_mut(d, sid).add_constraint(kind, &refs, value)?;
                let report = settle(d, sid)?;
                Ok(json!({"id": id, "degrees_of_freedom": report.dof}))
            })
        }
        "set_dimension" => {
            let sid = sketch_id(&s.doc, c)?;
            let cid = id_of(c, "constraint")?;
            let text = text_of(&c["value"])?;
            let r = s.edit(|d| {
                let kind = d.sketch(sid).unwrap().constraints.get(&cid).and_then(|c| c.kind.value_kind()).ok_or(format!("{cid} is not a dimension"))?;
                let v = d.enter(&text, kind)?;
                sk_mut(d, sid).constraints.get_mut(&cid).unwrap().value = Some(v);
                let report = settle(d, sid)?;
                Ok(json!({"id": cid, "degrees_of_freedom": report.dof}))
            })?;
            Ok(r)
        }
        "delete" => {
            let sid = sketch_id(&s.doc, c)?;
            let ids = ids_of(c, "ids")?;
            s.edit(|d| {
                sk_mut(d, sid).remove(&ids);
                settle(d, sid)?;
                Ok(json!({"deleted": ids}))
            })
        }
        "extrude" | "revolve" => {
            let extrude = op == "extrude";
            let face = match &c["face"] {
                J::Null => None,
                _ if !extrude => return Err("only extrude takes a \"face\"".into()),
                f => Some(face_of(s, f)?),
            };
            let through_all = match c["extent"].as_str() {
                None | Some("distance") => false,
                Some("all") => true,
                Some(o) => return Err(format!("unknown extent '{o}'; use \"distance\" or \"all\"")),
            };
            let text = match (&c["distance"], through_all) {
                (J::Null, true) => "1".to_owned(),
                (v, _) if extrude => text_of(v).map_err(|_| "extrude needs a \"distance\"")?,
                _ => String::new(),
            };
            let c = c.clone();
            let has_bodies = !s.built.bodies.is_empty();
            let id = s.edit_feature(|d| {
                let (sid, profs) = match &face {
                    Some(face) => {
                        let (sk, profs) = face.sketch()?;
                        let sid = d.add_feature(FeatureKind::Sketch(sk));
                        d.feature_mut(sid).unwrap().name = format!("Face{sid}");
                        (sid, profs)
                    }
                    None => {
                        let sid = sketch_id(d, &c)?;
                        (sid, pick_profiles(d, sid, &c)?)
                    }
                };
                let kind = if extrude {
                    let distance = d.enter(&text, Kind::Length)?;
                    // Pushing a face in removes material unless told otherwise.
                    let default = if face.is_some() && distance.v < 0.0 { Op::Cut } else if has_bodies { Op::Join } else { Op::New };
                    let taper = match &c["taper"] {
                        J::Null => None,
                        v => Some(d.value(&text_of(v)?, Kind::Angle)?),
                    };
                    FeatureKind::Extrude(Extrude { sketch: sid, profiles: profs, distance, symmetric: c["symmetric"].as_bool().unwrap_or(false), op: op_of(&c, default)?, taper, through_all })
                } else {
                    let angle = match &c["angle"] {
                        J::Null => d.value("360", Kind::Angle)?,
                        v => d.value(&text_of(v)?, Kind::Angle)?,
                    };
                    let axis = match &c["axis"] {
                        // An axis given by two points is drawn into the sketch as a construction line.
                        J::Object(o) => {
                            let (a, b) = (xy(d, &o["from"]).map_err(|e| format!("axis \"from\": {e}"))?, xy(d, &o["to"]).map_err(|e| format!("axis \"to\": {e}"))?);
                            if a.distance(b) < 1e-9 {
                                return Err("the axis needs two different points".into());
                            }
                            let sk = sk_mut(d, sid);
                            let (pa, pb) = (sk.add_point(a), sk.add_point(b));
                            Axis::Line(sk.add(Geom::Line { a: pa, b: pb }, true))
                        }
                        v => axis_of(d, sid, v)?,
                    };
                    FeatureKind::Revolve(Revolve { sketch: sid, profiles: profs, axis, angle, op: op_of(&c, if has_bodies { Op::Join } else { Op::New })? })
                };
                let id = d.add_feature(kind);
                sk_mut(d, sid).visible = false;
                Ok((id, id))
            })?;
            {
                let mut out = changed(s, &before);
                out["feature"] = json!(id);
                Ok(out)
            }
        }
        "pattern" => {
            let source = id_of(c, "feature")?;
            let axis = |key: &str, default: usize| match c[key].as_str().map(str::to_ascii_lowercase).as_deref() {
                None => Ok(default),
                Some("x") => Ok(0),
                Some("y") => Ok(1),
                Some("z") => Ok(2),
                Some(o) => Err(format!("unknown axis '{o}'; use x, y or z")),
            };
            let count = c["count"].as_u64().unwrap_or(0) as u32;
            let kind = match c["type"].as_str() {
                Some("circular") => PatternKind::Circular {
                    axis: axis("axis", 2)?,
                    count,
                    angle: match &c["angle"] {
                        J::Null => s.doc.value("360", Kind::Angle)?,
                        v => s.doc.value(&text_of(v)?, Kind::Angle)?,
                    },
                },
                Some("linear" | "rectangular") => PatternKind::Linear { axis: axis("axis", 0)?, count, spacing: s.doc.value(&text_of(&c["spacing"]).map_err(|_| "a linear pattern needs a \"spacing\"")?, Kind::Length)? },
                Some("mirror") => PatternKind::Mirror { axis: axis("normal", 0)? },
                _ => return Err("pattern needs a \"type\": circular, linear or mirror".into()),
            };
            let id = s.edit_feature(|d| {
                let id = d.add_feature(FeatureKind::Pattern(Pattern { source, kind }));
                Ok((id, id))
            })?;
            {
                let mut out = changed(s, &before);
                out["feature"] = json!(id);
                Ok(out)
            }
        }
        "trim" | "mirror" | "offset" | "fillet" | "chamfer" | "project" => {
            let sid = sketch_id(&s.doc, c)?;
            let face = if op == "project" { Some(face_of(s, c)?) } else { None };
            let c = c.clone();
            s.edit(|d| {
                let mut sk = d.sketch(sid).unwrap().clone();
                let made: Vec<Id> = match op {
                    "trim" => {
                        let at = xy(d, &c["near"]).map_err(|e| format!("\"near\": {e}"))?;
                        sk.trim(id_of(&c, "entity")?, at)?;
                        vec![]
                    }
                    "mirror" => sk.mirror(&ids_of(&c, "ids")?, id_of(&c, "axis")?)?,
                    "offset" => sk.offset(&ids_of(&c, "ids")?, d.enter(&text_of(&c["distance"])?, Kind::Length)?)?,
                    "project" => sk.project(&face.as_ref().unwrap().outline),
                    _ => {
                        let key = if op == "fillet" { "radius" } else { "distance" };
                        let size = d.enter(&text_of(&c[key]).map_err(|_| format!("{op} needs a \"{key}\""))?, Kind::Length)?;
                        vec![sk.round_corner(id_of(&c, "point")?, size, op == "chamfer")?]
                    }
                };
                *sk_mut(d, sid) = sk;
                let report = settle(d, sid)?;
                Ok(json!({"new": made, "degrees_of_freedom": report.dof}))
            })
        }
        "edit_feature" => {
            let id = id_of(c, "feature")?;
            let c = c.clone();
            s.edit_feature(|d| {
                let probe = d.clone();
                let f = d.feature_mut(id).ok_or(format!("there is no feature {id}"))?;
                if let Some(n) = c["name"].as_str() {
                    f.name = n.to_owned();
                }
                if let Some(v) = c["suppressed"].as_bool() {
                    f.suppressed = v;
                }
                match &mut f.kind {
                    FeatureKind::Extrude(e) => {
                        if !c["distance"].is_null() {
                            e.distance = probe.value(&text_of(&c["distance"])?, Kind::Length)?;
                        }
                        if let Some(v) = c["symmetric"].as_bool() {
                            e.symmetric = v;
                        }
                        if !c["taper"].is_null() {
                            e.taper = Some(probe.value(&text_of(&c["taper"])?, Kind::Angle)?);
                        }
                        if let Some(v) = c["extent"].as_str() {
                            e.through_all = v == "all";
                        }
                        e.op = op_of(&c, e.op)?;
                    }
                    FeatureKind::Revolve(r) => {
                        if !c["angle"].is_null() {
                            r.angle = probe.value(&text_of(&c["angle"])?, Kind::Angle)?;
                        }
                        if !c["axis"].is_null() {
                            r.axis = axis_of(&probe, r.sketch, &c["axis"])?;
                        }
                        r.op = op_of(&c, r.op)?;
                    }
                    _ => {}
                }
                Ok((id, ()))
            })?;
            feature_info(s, id)
        }
        "delete_feature" => {
            let id = id_of(c, "feature")?;
            s.edit(|d| {
                let n = d.features.len();
                d.features.retain(|f| f.id != id);
                if d.features.len() == n { Err(format!("there is no feature {id}")) } else { Ok(()) }
            })?;
            Ok(json!({"deleted": id, "errors": s.built.errors}))
        }
        "import_stl" => {
            let path = c["path"].as_str().ok_or("import_stl needs a \"path\"")?;
            let mesh = io::read_stl(path.as_ref(), unit_of(c, Unit::Mm)?)?;
            let name = std::path::Path::new(path).file_stem().map(|n| n.to_string_lossy().into_owned());
            let id = s.edit(|d| {
                let id = d.add_feature(FeatureKind::Import(mesh));
                if let Some(n) = name {
                    d.feature_mut(id).unwrap().name = n;
                }
                Ok(id)
            })?;
            Ok(json!({"feature": id, "body": body_info(s, id)}))
        }
        "transform" => {
            let body = id_of(c, "body")?;
            let triple = |d: &Document, key: &str, kind: Kind| -> R<[Value; 3]> {
                match c[key].as_array().map(Vec::as_slice) {
                    None => Ok([zero(d, kind), zero(d, kind), zero(d, kind)]),
                    Some([x, y, z]) => Ok([d.value(&text_of(x)?, kind)?, d.value(&text_of(y)?, kind)?, d.value(&text_of(z)?, kind)?]),
                    Some(_) => Err(format!("\"{key}\" should be [x, y, z]")),
                }
            };
            let t = Transform {
                body,
                translate: triple(&s.doc, "translate", Kind::Length)?,
                rotate: triple(&s.doc, "rotate", Kind::Angle)?,
                scale: match &c["scale"] {
                    J::Null => s.doc.value("1", Kind::Scalar)?,
                    v => s.doc.value(&text_of(v)?, Kind::Scalar)?,
                },
            };
            let id = s.edit_feature(|d| {
                let id = d.add_feature(FeatureKind::Transform(t));
                Ok((id, id))
            })?;
            Ok(json!({"feature": id, "body": body_info(s, body)}))
        }
        "combine" => {
            let target = id_of(c, "target")?;
            let comb = Combine { target, tools: ids_of(c, "tools")?, op: op_of(c, Op::Join)?, keep_tools: c["keep_tools"].as_bool().unwrap_or(false) };
            let id = s.edit_feature(|d| {
                let id = d.add_feature(FeatureKind::Combine(comb));
                Ok((id, id))
            })?;
            Ok(json!({"feature": id, "body": body_info(s, target)}))
        }
        "set_visible" => {
            let id = id_of(c, "id")?;
            let visible = c["visible"].as_bool().unwrap_or(true);
            if s.doc.sketch(id).is_none() && s.built.body(id).is_none() {
                return Err(format!("{id} is not a sketch or a body"));
            }
            s.edit(|d| {
                if let Some(sk) = d.sketch_mut(id) {
                    sk.visible = visible;
                } else {
                    d.hidden_bodies.retain(|b| *b != id);
                    if !visible {
                        d.hidden_bodies.push(id);
                    }
                }
                Ok(json!({"id": id, "visible": visible}))
            })
        }
        other => Err(format!("unknown op '{other}'")),
    }
}
