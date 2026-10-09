//! The JSON command API. Everything the app can do to a document is a
//! command here, so the MCP server and the built-in assistant drive the
//! same code as the user interface.

mod planes_api;
mod components_api;
mod primitives_api;
mod body_ops_api;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use base64::Engine;
use glam::{DVec2, DVec3};
use serde_json::{Value as J, json};

use crate::doc::{Axis, Blend, Combine, Document, Extrude, FeatureKind, Hole, HoleFit, HoleShape, LinearDirection, Loft, LoftSection, Op, Pattern, PatternKind, Revolve, Session, Shell, Sweep, SweepOrient, Text, Thread, Transform};
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
pub const REFERENCE: &str = r#"Ferrender is a parametric CAD program: sketches on planes, constrained and dimensioned, turned into solid bodies by extrude, revolve and sweep features. Commands are JSON objects with an "op". Z is up.

VALUES. Lengths are numbers in the document's units, or strings with units and arithmetic: 10, "10 mm", "2 in", "$width / 2 + 1cm". Parameters are referenced as $name. Angles are degrees.

WHAT STAYS PARAMETRIC. Dimension constraints and feature values (extrude distance, revolve angle, transform amounts) keep their expressions: change a parameter and they follow. Coordinates in add_geometry only place the geometry once; a "$name" there is evaluated and forgotten. So for a size the user may want to change, draw the shape roughly, then add distance / radius / diameter / angle constraints whose values use the parameter.

DOCUMENT
{"op":"new","units":"mm"}                         units: mm | cm | in
{"op":"set_units","units":"in"}
{"op":"set_parameter","name":"width","expr":"40 mm"}
{"op":"delete_parameter","name":"width"}
{"op":"undo"} {"op":"redo"}
{"op":"save","path":"/abs/part.ferr"} {"op":"open","path":"/abs/part.ferr"}   save takes "cache":true|false to request or skip the geometry cache (by default it is written when the file is a container or the design is slow to rebuild); when a wanted cache cannot be written (modeled threads, geometry over the 32 MiB cache limit, feature errors) the response says why in "cache_skipped" and the design rebuilds when opened; open uses a matching locally authenticated cache; foreign/unsigned files rebuild. get_scene_info reports "from_cache" and "geometry_trust"; newer unsupported files are an "unverified_preview" and remain read-only
   A design with reference images or imported meshes is saved as a ZIP container (same JSON inside plus the blobs and a thumbnail); plain designs stay plain JSON. save returns "container" and, when it converted an older plain file, "backup" with the path of the kept original. open reads both forms.
   In the GUI, new and open refuse to discard unsaved work. Save first or set "discard_unsaved":true on that command to explicitly discard it, including inside a batch.
{"op":"export_stl","path":"/abs/part.stl","units":"mm"}   units default to mm, which is what slicers expect
{"op":"import_mesh","path":"/abs/in.stl","units":"mm"}    adds an STL, OBJ or 3MF as a mesh body, repaired (degenerate and duplicate triangles dropped, orientation made consistent) with a report; import_stl is the same command

Mesh editing. Each is a timeline feature that takes a mesh body and replaces it (an exact body becomes a mesh). Coordinates are in document units.
{"op":"mesh_measure","body":BODY,"at":[x,y,z]}        triangles, vertices, shells, open and non-manifold edges, watertight, volume, area, bounds; with "at", the wall thickness under that point
{"op":"mesh_repair","body":BODY,"fill_holes":12}      drops degenerate and duplicate triangles, makes orientation consistent, turns closed shells outward, fills holes of up to that many edges
{"op":"mesh_decimate","body":BODY,"target":50000,"method":"quadric","preserve_boundary":true}   quadric edge collapse to about that many triangles; "method":"cluster" is the fast coarse alternative
{"op":"mesh_smooth","body":BODY,"iterations":10,"strength":0.5,"region":REGION}   Taubin smoothing (no shrink); region optional
{"op":"mesh_subdivide","body":BODY,"levels":1,"scheme":"loop"}   loop | midpoint; each level quadruples the triangles
{"op":"mesh_cut","body":BODY,"plane":PLANE,"keep":"negative","cap":true}   plane as for split_body; keep negative | positive | both (both makes a second body); cap closes the cut flat
{"op":"mesh_mirror","body":BODY,"plane":PLANE,"weld":true}   adds the mirror image; weld joins the halves along the plane
{"op":"mesh_offset","body":BODY,"distance":2,"direction":[0,0,-1]}   thickens an open surface into a closed solid (walls along its rim), or hollows a closed one; direction optional, else along the surface normals
{"op":"mesh_extrude_region","body":BODY,"region":REGION,"distance":3,"direction":[0,0,1]}   moves the region's triangles with walls around it
   REGION is {"sphere":{"centre":[x,y,z],"radius":r}} | {"box":{"lo":[..],"hi":[..]}} | {"side":{"plane":PLANE}} (the plane's positive side) | {"normal":{"direction":[x,y,z],"degrees":30}} | {"connected":{"seed":[x,y,z]}}
{"op":"mesh_sculpt","body":BODY,"brush":"pull","at":[x,y,z],"radius":8,"strength":2}   one stroke: push | pull (along the surface normal under the point; strength is a length in document units or an explicit unit expression) | inflate (along each vertex's own normal) | smooth | flatten (strength 0..1); vertices within the radius move with a smooth falloff; each stroke is a feature
{"op":"mesh_from_image","path":"/abs/photo.png","width":100,"depth":4,"base":2,"resolution":300,"invert":false,"blur":1,"gamma":1,"plane":"XY","origin":[x,y,z],"operation":"new"}
   a relief: the image's luminance becomes height on a grid (resolution cells along the longer side, at most 1200), bright high (invert for a lithophane), on a slab "base" thick; "depth" is the height of the brightest pixel. The image (PNG or JPEG) is embedded, as a reference image is. A depth map rendered elsewhere works the same way. Then mesh_smooth, mesh_cut to trim, combine onto a plaque, export_stl.
{"op":"export_stl","path":"...","union":true}      merges overlapping bodies into one shell first (exact bodies only)
{"op":"export_step","path":"/abs/part.step"}       exact bodies as true surfaces; meshes are skipped
{"op":"batch","commands":[...]}                    runs several commands; stops at the first error
   Requests are limited to 1000 commands including batch containers and 16 nesting levels. A rejected limit applies no commands; an execution error leaves earlier commands applied.
   Inside any command, "$last_sketch", "$last_feature" and "$last_body" stand for the newest sketch, feature and body, so a batch can use what it just made without knowing ids ahead of time.

RESULTS. Commands that change bodies return only "changed_bodies" (and "removed_bodies", "body_count"), not the whole list; use get_scene_info for everything.

BODIES are "exact" (made from sketches: true planes, cylinders and blends) or "mesh" (imported STL, a tapered extrude, or anything combined with a mesh). Only exact bodies can be filleted, chamfered, shelled or written to STEP.

COMPONENTS (root id 0)
{"op":"create_component","name":"Bracket","parent":0,"activate":true}   omit parent for active component; returns component id
{"op":"activate_component","id":0}
{"op":"move_component","id":ID,"translate":[x,y,z],"rotate":[rx,ry,rz]}   expressions; rigid placement relative to parent; omitted values unchanged
   New sketches and solids belong to the active component. Join/Cut/Intersect and Through All affect only that component. Body-specific operations follow the target body's owner; patterns follow their source. Combine explicitly crosses components and keeps the target's owner. Sketch/origin/typed plane inputs and Transform/Pattern axes are component-local; sketch query plane axes are world-space (local_plane retains the local frame), with origin lengths in document units; face/edge/vertex picks and measurements are world coordinates. Moving a component preserves local history.
   get_scene_info returns the component tree and active_component; every feature/body reports component. get_object_info accepts component ids. set_visible hides a subtree; suppression removes it from the build. delete_feature on a component deletes its subtree and returns removed_features. STL/STEP exports accept optional component:ID; STEP stays flat. Reparenting and moving bodies between components are deferred.

CONSTRUCTION PLANES
{"op":"create_plane","kind":"offset","base":"XY","distance":"$gap"}   base can also be {"plane":ID} or {"face":{"body":ID,"point":[x,y,z]}}
{"op":"create_plane","kind":"midplane","faces":[{"body":ID,"point":[x,y,z]},{"body":ID,"point":[x,y,z]}],"flip":false}
{"op":"create_plane","kind":"three_point","points":[[x,y,z],{"body":ID,"point":[x,y,z]},{"sketch":ID,"point":POINT}]}
{"op":"create_sketch","plane":{"id":ID}}   stays attached to the construction plane; offset it with another plane feature
   create_plane returns feature id and resolved world origin/x/y/normal. edit_feature accepts the same plane fields, including partial edits. get_object_info reports inputs, axes, visibility and errors. References must precede the plane; missing references fail visibly and never build dependent geometry on a stale plane. Three-point coordinate arrays are in the active component frame; body picks are world points in document units.

SKETCHES
{"op":"create_sketch","plane":"XY"}                XY (top) | XZ (front) | YZ (right); optional "offset" along the plane normal, or "plane":{"origin":[x,y,z],"normal":[x,y,z],"x":[x,y,z]}
   Returns the sketch id and the plane's origin, x, y and normal in space, so you know which way sketch x and y point. Plane normals: XY is +Z, XZ is -Y (so a positive offset on XZ moves toward -Y), YZ is +X.
{"op":"add_geometry","sketch":ID,"items":[...],"construction":false}
   items: {"type":"line","from":[x,y],"to":[x,y]}
          {"type":"polyline","points":[[x,y],...],"closed":true}
          {"type":"rect","from":[x,y],"to":[x,y]}          gets horizontal/vertical constraints; or "center":[x,y],"size":[w,h]
          {"type":"circle","center":[x,y],"radius":r}      or "diameter"
          {"type":"arc","center":[x,y],"start":[x,y],"end":[x,y]}   counter-clockwise from start to end
          {"type":"arc3","start":[x,y],"through":[x,y],"end":[x,y]}   passes through three points, clockwise or counter-clockwise
          {"type":"tangent_arc","source":ENTITY,"start":POINT,"end":[x,y]}   extends a line or circular arc endpoint with a persistent tangent constraint
          {"type":"spline","points":[[x,y],[x,y],[x,y],[x,y]]}   native interpolating B-spline through four editable fit points
          {"type":"ngon","center":[x,y],"radius":r,"sides":6,"rotation":0}   regular polygon, corners on the radius
          {"type":"point","at":[x,y]}
   Endpoints at the same coordinates share one point, so shapes drawn end to end are closed. The sketch origin is point 0 and is fixed. Returns the new entity and point ids for each item.
{"op":"point_coordinates","sketch":ID,"point":POINT,"x":"$width / 2","y":"-3 mm"}
   Creates or updates signed position_x and position_y dimensions. Omit point to create a new point. Both expressions remain parametric, edits are one undo step, and conflicts leave the document unchanged. The fixed origin cannot be moved. Spline fit points use this same command; radius/tangent/equal constraints apply to circles and arcs, not spline entities.
{"op":"add_constraint","sketch":ID,"kind":KIND,"refs":[ids],"value":V}
   geometric kinds: coincident (2 points | point+line | point+curve), horizontal / vertical (line | 2 points), parallel, perpendicular, collinear (2 lines), tangent (line+curve | 2 curves), equal (2 lines | 2 curves), midpoint (point+line), concentric (2 curves), symmetric (2 points + mirror line), fix (point | line)
   dimension kinds (need "value"): distance (line = its length | 2 points | point+line | 2 parallel lines), radius, diameter (circle or arc), angle (one line = signed direction from sketch +X; one arc = sweep greater than 0 and less than 360 degrees; 2 lines = angle between them)
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
{"op":"primitive","type":"box","width":V,"depth":V,"height":V,"position":[0,0,0],"rotate":[0,0,0],"operation":"new"}
   Native exact solids: box(width,depth,height), cylinder(diameter,height), sphere(diameter), cone(bottom_diameter,top_diameter,height; top defaults to 0), torus(major_radius,tube_radius). Dimensions are lengths; position and rotate are optional three-value expression arrays. New Body is the default; join/cut/intersect affect the active component. Box position is its lower corner; cylinder/cone position is its base center (+Z height); sphere/torus position is its center (torus axis +Z). Rotate X then Y then Z about that origin, then translate, in the component's frame. Dimensions are 0.001–10000 mm; one cone end may be zero and equal cone diameters make a cylinder. Torus major radius must exceed tube radius by at least 0.001 mm. Position is bounded to ±1000000 mm. edit_feature changes dimensions, position, rotate and operation; type stays fixed. Values accept units and parameter expressions. get_object_info reports type:primitive, shape, dimensions and placement.
{"op":"text","text":"CAD","height":"6 mm","depth":"1 mm","operation":"new","plane":"XY"}
   Creates parametric solid lettering. plane: XY | XZ | YZ, with optional origin:[x,y,z] in document units. height defaults to 6 mm, depth to 1 mm. spacing, x, y default to 0; angle defaults to 0 degrees. align: left | center | right (default left). x/y shift the lettering within its plane; angle rotates it there. Numeric fields accept units and parameter expressions.
{"op":"text","text":"CAD","operation":"join","body":BODY,"face":[x,y,z],"height":"6 mm","depth":"1 mm"}
   join embosses outward and cut engraves inward on the selected exact body's flat face; only that body changes. face must be a point on the face, in document units, and anchors the text plane there. Attached text cannot also specify plane/origin. Use operation:new without body/face for free-standing lettering. Unsupported characters and failed placement are rejected without changing the document.
   edit_feature can change text, height, depth, spacing, x, y, angle, align, or operation. To attach previously free-standing text, also supply body and face; changing attached text to new detaches it while keeping its placement. New text can also change plane/origin.
   When editing body/face, coordinates refer to the base body immediately before the text feature. If later timeline features changed that body's geometry, first rollback to immediately before or after the text feature, choose its base face again, edit it, then rollback to "end". Face edits reject ambiguous downstream coordinates and cannot attach to the text's own raised faces. Text and dimension edits do not require rolling back.
{"op":"extrude","sketch":ID,"distance":V,"operation":"new","symmetric":false,"profiles":[indices]}
   operation: new | join | cut | intersect. Negative distance goes the other way. With "symmetric":true the distance is the total thickness, half each side. A shape drawn inside another in the SAME sketch becomes a hole; shapes in different sketches never do. "profiles" are indices from get_object_info on the sketch; when omitted, every outer region is used and regions nested inside become holes; "all" fills them in.
{"op":"revolve","sketch":ID,"axis":"x","angle":360,"operation":"new","profiles":[...]}
   axis: "x" or "y" (the sketch's axes), the id of a line in the sketch, or {"from":[x,y],"to":[x,y]}. The profile must not cross the axis.
{"op":"sweep","sketch":PROFILE_SKETCH,"path_sketch":PATH_SKETCH,"path":[entity ids],"spans":[[0.1,0.3],[0.6,0.7]],"orientation":"follow","operation":"new","profiles":[...]}
   carries the profile along a path drawn in ANOTHER sketch: lines, arcs and splines joined end to end, or one circle. Draw the path first, then the profile on a plane that crosses it (for a path on XY starting along X, a profile on YZ). "path" names the entities to follow; omitted, it is every non-construction entity of path_sketch, which must then be one unbranched run. A closed path gives a ring or a frame. Pieces that meet tangentially are followed exactly; a sharp corner is mitred like a picture frame (it may turn by at most 150 degrees). The profile may sit anywhere along the path and off to one side of it. orientation: follow (the profile turns with the path) | fixed (it keeps its orientation; the path may not run sideways to it). Refused with a reason when a bend is tighter than the profile reaches on its inside, or a stretch between corners is too short. get_object_info on the feature lists the path in the order it is walked. Each side face is named by its profile entity and path entity. "spans" sweeps only parts of the path: each pair is a start and an end as fractions of the path's length, 0 at its start and 1 at its end, in walking order ([[0,0.5]] is the first half, [[0.1,0.3],[0.6,0.7]] two separate pieces in one body). Omitted or [] is the whole path. Each piece is the part of the whole sweep that lies there, so the profile stays where it would be on the full sweep. Pieces that touch or overlap are joined. On a circle, 0 is at the sketch's +X side and the fractions run anticlockwise.
   extrude also takes "extent":"all" (go through bodies in its component; the sign of distance picks the side), "taper":DEGREES (walls lean outward, negative inward), and instead of a sketch, "face":{"body":BODY,"point":[x,y,z]} to pull the flat face nearest that point out (or, with a negative distance, push it in and cut).
{"op":"create_sketch","face":{"body":BODY,"point":[x,y,z]}}   sketch on a flat face
{"op":"loft","sections":[SKETCH,SKETCH,{"sketch":SKETCH,"profile":INDEX},{"sketch":SKETCH,"point":POINT}],"ruled":false,"operation":"new"}
   skins one solid through closed outlines drawn in sketches on DIFFERENT planes, in the order given: two or more sections, each a sketch id (the sketch's one outer region), a sketch with the index of a region from get_object_info, or a sketch with the id of one of its points, which makes the loft come to a tip there (a cone or pyramid whose tip is a flat 0.001 mm across). Stack the sketches with create_sketch's "offset", on construction planes, or on faces. Every section needs the same number of edges: four lines to four lines, a circle to a circle; a mismatch is refused with both counts. Sections are matched corner to nearest corner, so it does not matter where each outline was started or which way round it was drawn. A region with a hole cannot be a section. "ruled":true joins neighbouring sections with straight walls (a frustum between two squares); otherwise the surface curves smoothly through all of them, which only differs from ruled with three or more sections. The ends are flat caps on the first and last sections. Returns the feature and the body it made or changed.
{"op":"pattern","feature":ID,"type":"circular","axis":"z","count":6,"angle":360}   repeats an extrude, revolve, sweep, loft, primitive, import or standalone text around a component-local axis through its origin; count includes the original
{"op":"pattern","feature":ID,"type":"linear","axis":"x","count":4,"spacing":V}
{"op":"pattern","feature":ID,"type":"linear","axis":"x","count":2,"spacing":V,"axis2":"y","count2":2,"spacing2":V}
   Optional axis2/count2/spacing2 form a rectangular grid; supply all three together, with distinct component-local axes. Each count includes the source and must be at least 2; their product is at most 1000. Spacing is between adjacent instances and can be negative; both spacings in a grid must be nonzero. Without a second direction, count is at most 1000 and zero spacing remains allowed for compatibility (coincident copies).
{"op":"pattern","feature":ID,"type":"mirror","normal":"x"}   one reflected copy through the origin plane with that normal
{"op":"edit_feature","feature":ID, ...}            any of distance, angle, operation, symmetric, extent, taper, axis, name, suppressed; on a sweep, path, spans and orientation; on a loft, sections and ruled
{"op":"delete_feature","feature":ID}
{"op":"remove_body","bodies":[BODY_IDS]}           removes only these bodies at this timeline point; keeps their source features and previously patterned copies. body:ID is a single-body alias. Undo or suppress this feature to restore them.
{"op":"split_body","body":BODY,"plane":"XY"}       plane: XY | XZ | YZ in target-component axes, {"plane":CONSTRUCTION_ID}, or {"face":{"body":ID,"point":[x,y,z]}} with a world-space planar-face pick in document units. Uses the infinite plane. Exact unthreaded bodies only; tangent/nonintersecting planes are rejected. Each solid piece becomes an independent body in the target component; the first negative-side piece keeps the target ID, others have stable synthetic IDs. edit_feature accepts body/plane for Split and bodies for Remove; picks resolve before that operation. Combine with operation:join joins selected pieces again.
{"op":"rollback","to":ID}                          shows the model as it was just after that feature ("start" = before any, "end" = everything); features added while rolled back are inserted at that point
{"op":"transform","body":ID,"translate":[x,y,z],"rotate":[rx,ry,rz],"scale":1}   scale about the origin, rotate about X then Y then Z, then translate
{"op":"combine","target":BODY,"tools":[BODY],"operation":"join","keep_tools":false}   join | cut | intersect between bodies, including imported meshes; a mesh boolean splits only the triangles near the other surface, so a small tool against a scan of millions of triangles is quick
{"op":"fillet_edges","body":BODY,"edges":[[x,y,z],...],"radius":V}   rounds the edges nearest those points; "edges":"all" takes every edge. An entry can also be {"tag":EDGE_TAG} copied from get_object_info on the body: faces and edges carry tags saying how they were made (swept from a sketch entity, a cap, made by a feature), and a fillet, chamfer, shell, thread, text or face plane finds its faces by persistent tag when the body changes shape. Modern removed or ambiguous references fail rather than selecting an unrelated nearby face; untagged or legacy references learn a tag only from a unique saved-location match. get_object_info on such a feature reports "resolved": "tag" | "origin" | "position".
{"op":"chamfer_edges","body":BODY,"edges":[[x,y,z],...],"distance":V}
{"op":"shell","body":BODY,"open_faces":[[x,y,z],...],"thickness":V}   hollows the body, leaving the faces nearest those points open
   get_object_info on an exact body lists its edges and faces under "topology", each with a "point" to use here. Edges and faces retain their semantic/source identity through supported size and topology edits. Deleted or indistinguishable references request reselection. Placing fillets, chamfers and shells late can still reduce downstream dependencies.
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
{"op":"set_visible","id":ID,"visible":false}       a sketch, body, plane or component

QUERIES
{"op":"get_scene_info"}                            units, parameters, features (with any errors) and bodies
{"op":"measure","from":ITEM,"to":ITEM}             ITEM is {"point":[x,y,z]}, {"body":B,"edge":[x,y,z]} or {"body":B,"face":[x,y,z]} (the edge or face nearest that point)
   "distance" is the shortest between the two, found at "from" and "to". "apart" is the perpendicular gap when they are parallel (two flat faces, two straight edges, a point off a face); "angle" is between straight or flat items.
{"op":"get_object_info","id":ID}                   a sketch (points, entities, constraints, profiles), feature or body
{"op":"get_viewport_screenshot","view":"iso","width":900,"height":650}   view: iso | top | front | right | back | left | bottom | current
   The view is fitted to the bodies. For a custom camera add "azimuth" and "elevation" in degrees (the eye's bearing around Z and height above the XY plane), "target":[x,y,z] to centre on a point, and "zoom" to magnify (2 = twice as close).
{"op":"get_reference"}                             this text

Scripts. A Rhai script with a META map (name, description, inputs) and fn run(inputs) drives these same commands as functions: extrude(#{sketch: s, distance: 10}) returns what the command returns; new is new_design and thread is add_thread (Rhai keywords); command(#{op: ...}) dispatches a command (nested scripts and batches are excluded). Also scene(), info(id), params(), errors(), selection(), measure(a, b), screenshot(path, #{view: "iso"}); files read_text, write_text, read_csv, write_csv, list_files(dir, ".ferr"), exists, mkdir, join, basename, document_dir(), script_dir(), all inside the allowed folders; log, progress(0..1, msg), confirm, ask, fail, name_template("{a}-{b}", #{a: 1, b: 2}). Files a script writes (save, exports, screenshots, write_text, write_csv) are staged beside their targets and moved into place only when the run succeeds; a cancelled or failed run leaves none of them. Links inside the allowed folders are not followed. Inputs arrive as numbers in mm and degrees with the typed text in inputs.expr.NAME, so passing inputs.expr.width to a command keeps the parameter live. Declared input kinds: length, angle, number, integer, bool, choice (with choices), text, folder, file, body, face, sketch; each has an initial value.
{"op":"script_meta","source":"..."} or {"path":"/abs/x.rhai"}   the META of a script without running it
{"op":"run_script","path":"/abs/x.rhai","inputs":{"width":"30 mm"},"allow":["/abs/out"],"yes":true,"timeout":600}   runs the script as one undo step (source may be given instead of path); timeout is seconds, after which the run fails at its next operation; returns log, result, exports, features, errors
{"op":"add_feature","feature":{...},"mesh_path":"/abs/scan.stl","image_path":"/abs/depth.png"}   appends a feature exactly as the file format writes it (used by exported timeline scripts); mesh_path fills an import's mesh from a file (with "units"), image_path a relief's or sketch's image

Bodies are named by the id of the feature that created them. Check get_scene_info for feature errors after changes, and look at a screenshot to confirm the shape."#;

type R<T> = Result<T, String>;

/// Every command `execute` accepts. Scripts get one host function per entry, and a test
/// checks this list against the match arms and the reference text.
pub const OPS: &[&str] = &[
    "get_scene_info", "get_object_info", "get_viewport_screenshot", "batch", "undo", "redo", "new", "open", "save",
    "export_stl", "export_step", "get_reference", "text", "rollback", "fillet_edges", "chamfer_edges", "shell", "measure",
    "list_threads", "hole", "thread", "move", "set_units", "set_parameter", "delete_parameter", "create_component",
    "activate_component", "move_component", "create_plane", "create_sketch", "add_geometry", "point_coordinates",
    "add_constraint", "set_dimension", "delete", "extrude", "revolve", "sweep", "loft", "remove_body", "split_body", "primitive", "pattern",
    "trim", "mirror", "offset", "fillet", "chamfer", "project", "edit_feature", "delete_feature", "import_stl", "import_mesh",
    "mesh_measure", "mesh_repair", "mesh_decimate", "mesh_smooth", "mesh_subdivide", "mesh_cut", "mesh_mirror", "mesh_offset",
    "mesh_extrude_region", "mesh_sculpt", "mesh_from_image", "transform", "combine", "set_visible", "run_script", "script_meta", "add_feature",
];

/// An `edit_feature` that gives a name and nothing else.
fn renames_only(c: &J) -> bool {
    let keys = ["op", "feature", "name"];
    c["name"].is_string() && c.as_object().is_some_and(|o| o.iter().all(|(k, v)| keys.contains(&k.as_str()) || v.is_null()))
}

fn id_of(c: &J, key: &str) -> R<Id> {
    c[key].as_u64().and_then(|v| Id::try_from(v).ok()).ok_or(format!("\"{key}\" should be an id"))
}

fn ids_of(v: &J, key: &str) -> R<Vec<Id>> {
    v[key].as_array().ok_or(format!("\"{key}\" should be a list of ids"))?.iter().map(|x| x.as_u64().and_then(|v| Id::try_from(v).ok()).ok_or(format!("\"{key}\" should be a list of ids"))).collect()
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

fn sketch_info(s: &Session, id: Id) -> J {
    let doc = &s.doc;
    let sk = doc.sketch(id).unwrap();
    let plane = s.built.sketch_plane(doc, id);
    let report = solver::solve(&mut sk.clone(), &[]);
    let ents: Vec<J> = sk
        .entities
        .iter()
        .map(|(eid, e)| {
            let mut o = match e.geom {
                Geom::Line { a, b } => json!({"type": "line", "a": a, "b": b, "length": len_out(doc, sk.pos(a).distance(sk.pos(b)))}),
                Geom::Circle { c, r } => json!({"type": "circle", "center": c, "radius": len_out(doc, r)}),
                Geom::Arc { c, s, e } => json!({"type": "arc", "center": c, "start": s, "end": e, "radius": len_out(doc, sk.pos(c).distance(sk.pos(s)))}),
                Geom::Spline { a,b,c,d } => json!({"type":"spline","points":[a,b,c,d]}),
            };
            if let Some(guide) = sk.arc_guides.get(eid) { o["arc_guide"] = json!(guide); }
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
        "plane": plane.map(|p| json!({"origin":(p.origin/doc.units.mm()).to_array(), "x":p.x.to_array(), "y":p.y.to_array(), "normal":p.normal().to_array()})),
        "local_plane": {"origin":(sk.plane.origin/doc.units.mm()).to_array(), "x":sk.plane.x.to_array(), "y":sk.plane.y.to_array()},
        "on": sk.on,
        "visible": sk.visible,
        "points": sk.points.iter().map(|(i, p)| (i.to_string(), pt_out(doc, *p))).collect::<serde_json::Map<_, _>>(),
        "entities": ents,
        "constraints": cons,
        "degrees_of_freedom": report.dof,
        "fully_constrained": report.ok && report.dof == 0,
        "profiles": profs,
        "open_endpoints": sk.open_endpoints(),
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
        "component": b.component,
        "visible": !s.doc.hidden_bodies.contains(&id) && s.built.component_visible(b.component),
        // Exact bodies can be filleted, chamfered, shelled and written to STEP; meshes cannot.
        "kind": if b.is_exact() { "exact" } else { "mesh" },
        "triangles": b.mesh.len(),
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
    let edges: Vec<J> = exact::edges_tagged(&b.solids, &b.tags).iter().take(400).map(|e| json!({"point": p(e.mid), "length": r(e.length / u), "shape": if e.straight { "line" } else { "curve" }, "tag": e.tag, "made": e.tag.as_ref().map(|t| t.describe())})).collect();
    let faces: Vec<J> = exact::faces_tagged(&b.solids, &b.tags).iter().take(400).map(|f| json!({"point": p(f.at), "normal": f.normal.to_array().map(r), "area": r(f.area / u.powi(2)), "shape": f.kind, "tag": f.tag, "made": f.tag.as_ref().map(|t| t.describe())})).collect();
    Some(json!({"edges": edges, "faces": faces}))
}

/// What each body looked like before a command, to report only what it changed.
fn marks(s: &Session) -> Vec<(Id, u64)> {
    s.built
        .bodies
        .iter()
        .map(|b| {
            let mut mark = DefaultHasher::new();
            b.mesh.len().hash(&mut mark);
            // Every vertex of a small body; a sample of a large mesh, whose edits are features of their own anyway.
            let step = (b.mesh.vertex_count() / 100_000).max(1);
            for vertex in b.mesh.positions().iter().step_by(step) {
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
            b.component.hash(&mut mark);
            s.built.component_visible(b.component).hash(&mut mark);
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
/// Edge picks for a command: each item is a point `[x,y,z]` in document units, or
/// `{"tag": EDGE_TAG}` naming an edge of the body by how it was made (as `get_object_info` lists them).
/// Returns world points and the tags of the edges they name.
fn edge_picks(s: &Session, body: Id, v: &J, key: &str) -> R<(Vec<DVec3>, Vec<Option<crate::tag::EdgeTag>>)> {
    let items = v.as_array().ok_or_else(|| format!("\"{key}\" must be a list of points or tags"))?;
    let b = s.built.body(body).ok_or(format!("there is no body {body}"))?;
    let (mut points, mut tags) = (Vec::new(), Vec::new());
    for item in items {
        if item.is_object() && !item["tag"].is_null() {
            let tag: crate::tag::EdgeTag = serde_json::from_value(item["tag"].clone()).map_err(|e| format!("\"{key}\" has an edge tag that is not valid: {e}"))?;
            let all = exact::edges_tagged(&b.solids, &b.tags);
            let hit = all.into_iter().find(|e| e.tag.as_ref() == Some(&tag)).ok_or("no edge of the body has that tag; list them with get_object_info")?;
            points.push(b.placement.transform_point3(hit.mid));
            tags.push(Some(tag));
        } else {
            let a = item.as_array().filter(|a| a.len() == 3).ok_or_else(|| format!("\"{key}\" entries must be [x,y,z] or {{\"tag\":...}}"))?;
            let u = s.doc.units.mm();
            let p = DVec3::new(a[0].as_f64().ok_or("bad coordinate")? * u, a[1].as_f64().ok_or("bad coordinate")? * u, a[2].as_f64().ok_or("bad coordinate")? * u);
            // The point is kept on the edge it names, so that it can be told from another edge with the same tag.
            match exact::edge_at(&b.solids, &b.tags, b.to_local(p)) {
                Some((on, tag)) => { tags.push(tag); points.push(b.placement.transform_point3(on)); }
                None => { tags.push(None); points.push(p); }
            }
        }
    }
    Ok((points, tags))
}

/// Face picks for a command: points or `{"tag": TAG}` objects, as [`edge_picks`].
fn face_picks(s: &Session, body: Id, v: &J, key: &str) -> R<(Vec<DVec3>, Vec<Option<crate::tag::Tag>>)> {
    let items = v.as_array().ok_or_else(|| format!("\"{key}\" must be a list of points or tags"))?;
    let b = s.built.body(body).ok_or(format!("there is no body {body}"))?;
    let (mut points, mut tags) = (Vec::new(), Vec::new());
    for item in items {
        if item.is_object() && !item["tag"].is_null() {
            let tag: crate::tag::Tag = serde_json::from_value(item["tag"].clone()).map_err(|e| format!("\"{key}\" has a face tag that is not valid: {e}"))?;
            let all = exact::faces_tagged(&b.solids, &b.tags);
            let hit = all.into_iter().find(|f| f.tag.as_ref() == Some(&tag)).ok_or("no face of the body has that tag; list them with get_object_info")?;
            points.push(b.placement.transform_point3(hit.at));
            tags.push(Some(tag));
        } else {
            let a = item.as_array().filter(|a| a.len() == 3).ok_or_else(|| format!("\"{key}\" entries must be [x,y,z] or {{\"tag\":...}}"))?;
            let u = s.doc.units.mm();
            let p = DVec3::new(a[0].as_f64().ok_or("bad coordinate")? * u, a[1].as_f64().ok_or("bad coordinate")? * u, a[2].as_f64().ok_or("bad coordinate")? * u);
            match exact::face_at(&b.solids, &b.tags, b.to_local(p)) {
                Some((on, tag)) => { tags.push(tag); points.push(b.placement.transform_point3(on)); }
                None => { tags.push(None); points.push(p); }
            }
        }
    }
    Ok((points, tags))
}

/// A mesh region from its JSON (see the reference), with lengths scaled from document units.
fn region_of(s: &Session, v: &J, u: f64) -> R<crate::meshops::RegionSpec> {
    use crate::meshops::RegionSpec;
    let o = v.as_object().ok_or("\"region\" must be an object such as {\"sphere\":{\"centre\":[x,y,z],\"radius\":r}}")?;
    let (kind, body) = o.iter().next().ok_or("\"region\" is empty")?;
    Ok(match kind.as_str() {
        "sphere" => RegionSpec::Sphere { centre: xyz(&body["centre"])? * u, radius: body["radius"].as_f64().ok_or("the sphere region needs a \"radius\"")? * u },
        "box" => RegionSpec::Box { lo: xyz(&body["lo"])? * u, hi: xyz(&body["hi"])? * u },
        "side" => {
            let reference = planes_api::base(s, &body["plane"])?;
            let (plane, _) = s.doc.plane_reference(&reference, &s.built, s.doc.active_component)?;
            RegionSpec::Side { plane }
        }
        "normal" => RegionSpec::Normal { direction: xyz(&body["direction"])?, degrees: body["degrees"].as_f64().unwrap_or(30.0) },
        "connected" => RegionSpec::Connected { seed: xyz(&body["seed"])? * u },
        other => return Err(format!("unknown region kind {other}; use sphere, box, side, normal or connected")),
    })
}

fn points_of(s: &Session, v: &J, key: &str) -> R<Vec<DVec3>> {
    v[key].as_array().ok_or(format!("\"{key}\" should be a list of [x, y, z] points"))?.iter().map(|p| xyz(p).map(|p| p * s.doc.units.mm())).collect()
}

fn feature_info(s: &Session, id: Id) -> R<J> {
    let doc = &s.doc;
    let f = doc.feature(id).ok_or(format!("nothing has id {id}"))?;
    let mut o = json!({"id": f.id, "name": f.name, "type": f.type_name(), "component": f.owner});
    if let Some(key) = &f.script_key { o["script_key"] = json!(key); }
    if let Some(level) = s.built.resolutions.get(&id) { o["resolved"] = json!(level.name()); }
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
        FeatureKind::Sketch(sk) => { o["on"] = json!(sk.on); }
        FeatureKind::Plane(_) => planes_api::info(s,id,&mut o),
        FeatureKind::Component(_) => {o["component_info"]=components_api::node(s,id);}
        FeatureKind::Primitive(p) => primitives_api::info(s,p,&mut o),
        FeatureKind::Remove(_) | FeatureKind::Split(_) => body_ops_api::info(s,id,&f.kind,&mut o),
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
        FeatureKind::Sweep(w) => {
            o["sketch"] = json!(w.sketch);
            o["path_sketch"] = json!(w.path_sketch);
            // The path as it is walked, whether it was named or is the whole sketch.
            match s.doc.sketch(w.path_sketch).ok_or_else(|| "its path sketch was deleted".to_owned()).and_then(|sk| crate::profile::chain(sk, &w.path)) {
                Ok(chain) => { o["path"] = json!(chain.ids); o["path_closed"] = json!(chain.closed); }
                Err(e) => { o["path"] = json!(w.path); o["path_error"] = json!(e); }
            }
            o["orientation"] = json!(w.orient.name());
            // Empty: the whole path.
            o["spans"] = json!(w.spans);
            o["operation"] = json!(w.op.name());
        }
        FeatureKind::Loft(l) => {
            // Each section with the index its region has in its sketch now, as the command takes it.
            o["sections"] = J::Array(l.sections.iter().map(|section| {
                if let Some(point) = section.point {
                    let mut out = json!({"sketch": section.sketch, "point": point});
                    if !doc.sketch(section.sketch).is_some_and(|sk| sk.points.contains_key(&point)) { out["error"] = json!("its point was deleted"); }
                    return out;
                }
                let mut out = json!({"sketch": section.sketch, "edges": section.profile.len()});
                match doc.sketch(section.sketch).map(profiles).and_then(|all| all.iter().position(|p| p.edges == section.profile)) {
                    Some(index) => out["profile"] = json!(index),
                    None => out["error"] = json!("its outline is no longer closed"),
                }
                out
            }).collect());
            o["ruled"] = json!(l.ruled);
            o["operation"] = json!(l.op.name());
        }
        FeatureKind::Text(t) => {
            o["text"] = json!(t.text);
            for (key, v) in [("height", &t.height), ("depth", &t.depth), ("spacing", &t.spacing), ("x", &t.x), ("y", &t.y)] {
                o[key] = json!({"expr": v.expr, "value": len_out(doc, v.v)});
            }
            o["angle"] = json!({"expr": t.angle.expr, "value": t.angle.v});
            o["align"] = json!(match t.align { crate::text::Align::Left => "left", crate::text::Align::Center => "center", crate::text::Align::Right => "right" });
            o["operation"] = json!(t.op.name());
            o["plane"] = json!({"origin": (t.plane.origin / doc.units.mm()).to_array(), "x": t.plane.x.to_array(), "y": t.plane.y.to_array(), "normal": t.plane.normal().to_array()});
            if let Some(body) = t.body { o["body"] = json!(body); }
            if let Some(face) = t.face { o["face"] = json!((face / doc.units.mm()).to_array()); }
        }
        FeatureKind::Import(m) => { o["triangles"] = json!(m.len()); o["vertices"] = json!(m.vertex_count()); }
        FeatureKind::Transform(t) => {
            o["body"] = json!(t.body);
            o["translate"] = json!(t.translate.iter().map(|v| v.expr.clone()).collect::<Vec<_>>());
            o["rotate"] = json!(t.rotate.iter().map(|v| v.expr.clone()).collect::<Vec<_>>());
            o["scale"] = json!(t.scale.expr);
        }
        FeatureKind::ScriptRun(r) => {
            o["script"] = json!(r.script_name);
            o["inputs"] = r.inputs.clone();
            o["source_hash"] = json!(r.source_hash);
            o["made"] = json!(doc.features.iter().filter(|g| g.made_by == Some(f.id)).map(|g| g.id).collect::<Vec<_>>());
        }
        FeatureKind::MeshOp(m) => {
            o["body"] = json!(m.body);
            o["op"] = serde_json::to_value(&m.op).unwrap_or(J::Null);
            if let Some(r) = &m.region { o["region"] = serde_json::to_value(r).unwrap_or(J::Null); }
        }
        FeatureKind::Relief(r) => {
            o["image"] = json!({"name": r.image.name, "pixels": [r.image.pixel_width, r.image.pixel_height]});
            o["width"] = json!({"expr": r.width.expr, "value": len_out(doc, r.width.v)});
            o["depth"] = json!({"expr": r.depth.expr, "value": len_out(doc, r.depth.v)});
            o["base"] = json!({"expr": r.base.expr, "value": len_out(doc, r.base.v)});
            o["resolution"] = json!(r.resolution);
            o["invert"] = json!(r.invert);
            o["blur"] = json!(r.blur);
            o["gamma"] = json!(r.gamma);
            o["operation"] = json!(r.op.name());
            o["plane"] = json!({"origin": (r.plane.origin / doc.units.mm()).to_array(), "normal": r.plane.normal().to_array(), "x": r.plane.x.to_array()});
        }
        FeatureKind::Pattern(p) => {
            o["feature"] = json!(p.source);
            let name = |a: &usize| ["x", "y", "z"].get(*a).copied().unwrap_or("?");
            match &p.kind {
                PatternKind::Circular { axis, count, angle } => o["pattern"] = json!({"type": "circular", "axis": name(axis), "count": count, "angle": angle.expr}),
                PatternKind::Linear { axis, count, spacing, second } => {
                    o["pattern"] = json!({"type": "linear", "axis": name(axis), "count": count, "spacing": spacing.expr});
                    if let Some(second) = second {
                        o["pattern"]["axis2"] = json!(name(&second.axis));
                        o["pattern"]["count2"] = json!(second.count);
                        o["pattern"]["spacing2"] = json!(second.spacing.expr);
                    }
                },
                PatternKind::Mirror { axis } => o["pattern"] = json!({"type": "mirror", "normal": name(axis)}),
            }
        }
        FeatureKind::Blend(b) => {
            o["body"] = json!(b.body);
            o["edges"] = json!(b.edges.len());
            o[if b.chamfer { "distance" } else { "radius" }] = json!({"expr": b.size.expr, "value": len_out(doc, b.size.v)});
            o["tags"] = json!(b.tags);
        }
        FeatureKind::Shell(sh) => {
            o["body"] = json!(sh.body);
            o["open_faces"] = json!(sh.faces.len());
            o["thickness"] = json!({"expr": sh.thickness.expr, "value": len_out(doc, sh.thickness.v)});
            o["tags"] = json!(sh.tags);
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
            o["tag"] = json!(t.tag);
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
        "active_component":doc.active_component,
        "components":[components_api::node(s,0)],
        "file": s.path.as_ref().map(|p| p.display().to_string()),
        "container": s.container,
        "from_cache": s.from_cache,
        "read_only": s.read_only,
        "geometry_trust": s.geometry_trust(),
        "geometry_warning": if s.read_only { Some("Unverified cached preview: this newer timeline cannot be rebuilt by this Ferrender.") } else { None },
        "rebuild_ms": s.rebuild_ms,
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
        "arc3" => {
            let positions = [p(doc,"start")?, p(doc,"through")?, p(doc,"end")?];
            let sk = sk_mut(doc,sid);
            pts = positions.into_iter().map(|p| sk.point_at(p,TOL)).collect();
            ents.push(sk.add_arc3(pts[0],pts[1],pts[2],construction)?);
        }
        "tangent_arc" => {
            let source = id_of(item,"source")?;
            let start = id_of(item,"start")?;
            let end = p(doc,"end")?;
            let sk = sk_mut(doc,sid);
            let end = sk.point_at(end,TOL);
            pts = vec![start,end];
            ents.push(sk.add_tangent_arc(source,start,end,construction)?);
        }
        "spline" => {
            let points = item["points"].as_array().filter(|p| p.len() == 4).ok_or("a spline needs four fit points")?;
            let positions = points.iter().map(|p| xy(doc,p)).collect::<R<Vec<_>>>()?;
            let sk = sk_mut(doc,sid);
            pts = positions.into_iter().map(|p| sk.point_at(p,TOL)).collect();
            ents.push(sk.add_spline([pts[0],pts[1],pts[2],pts[3]],construction)?);
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
    sk_mut(doc,sid).validate()?;
    let report = solver::solve(sk_mut(doc, sid), &[]);
    sk_mut(doc,sid).validate()?;
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

/// A loft's "sections": sketches, each with the one closed region to use. A bare
/// sketch id means the sketch's only outer region.
fn loft_sections_of(doc: &Document, c: &J) -> R<Vec<LoftSection>> {
    let wrong = "\"sections\" should list two or more sketches in order, each a sketch id, {\"sketch\": ID, \"profile\": INDEX} or {\"sketch\": ID, \"point\": POINT}";
    let list = c["sections"].as_array().ok_or(wrong)?;
    if list.len() < 2 {
        return Err("a loft needs at least two sections, each in its own sketch on its own plane".into());
    }
    list.iter().enumerate().map(|(i, v)| {
        let n = i + 1;
        let (sid, index, point) = match v {
            J::Number(_) => (v.as_u64().ok_or(wrong)? as Id, None, None),
            J::Object(o) => (o.get("sketch").and_then(J::as_u64).ok_or(wrong)? as Id, match o.get("profile") {
                None | Some(J::Null) => None,
                Some(p) => Some(p.as_u64().ok_or("a section's \"profile\" should be the index of a region of its sketch")? as usize),
            }, match o.get("point") {
                None | Some(J::Null) => None,
                Some(p) => Some(p.as_u64().ok_or("a section's \"point\" should be the id of a point of its sketch")? as Id),
            }),
            _ => return Err(wrong.to_owned()),
        };
        let sk = doc.sketch(sid).ok_or(format!("section {n}: feature {sid} is not a sketch"))?;
        // A point section: the loft comes to a tip there.
        if let Some(point) = point {
            if !sk.points.contains_key(&point) {
                return Err(format!("section {n}: sketch {sid} has no point {point}"));
            }
            return Ok(LoftSection { sketch: sid, profile: Vec::new(), point: Some(point) });
        }
        let all = profiles(sk);
        if all.is_empty() {
            return Err(format!("section {n}: sketch {sid} has no closed outline"));
        }
        let profile = match index {
            Some(k) => all.get(k).ok_or(format!("section {n}: sketch {sid} has {} region{}, so \"profile\" should be below {}", all.len(), if all.len() == 1 { "" } else { "s" }, all.len()))?,
            None => {
                let outer: Vec<_> = all.iter().filter(|p| p.depth == 0).collect();
                match outer[..] {
                    [one] => one,
                    _ => return Err(format!("section {n}: sketch {sid} has {} separate outlines; say which with {{\"sketch\": {sid}, \"profile\": INDEX}}", outer.len())),
                }
            }
        };
        Ok(LoftSection { sketch: sid, profile: profile.edges.clone(), point: None })
    }).collect()
}

/// A sweep's "spans": the parts of its path to follow, as pairs of fractions of the
/// path's length. Absent leaves the feature as it is; an empty list is the whole path.
fn sweep_spans_of(c: &J) -> R<Option<Vec<[f64; 2]>>> {
    let wrong = "\"spans\" should be a list of [start, end] pairs of fractions between 0 and 1, such as [[0.1, 0.3], [0.6, 0.7]]";
    let list = match &c["spans"] {
        J::Null => return Ok(None),
        J::Array(list) => list,
        _ => return Err(wrong.into()),
    };
    let spans = list.iter().map(|pair| match pair.as_array().map(Vec::as_slice) {
        Some([a, b]) => a.as_f64().zip(b.as_f64()).map(|(a, b)| [a, b]).ok_or(wrong.to_owned()),
        _ => Err(wrong.to_owned()),
    }).collect::<R<Vec<_>>>()?;
    exact::sweep_spans(&spans)?;
    Ok(Some(spans))
}

fn op_of(c: &J, default: Op) -> R<Op> {
    match &c["operation"] {
        J::Null => Ok(default),
        J::String(s) => Op::parse(s).ok_or(format!("unknown operation '{s}'; use new, join, cut or intersect")),
        _ => Err("\"operation\" should be new, join, cut or intersect".into()),
    }
}

fn text_fields(s: &Session, c: &J, t: &mut Text) -> R<()> {
    if let Some(value) = c.get("text") { t.text = value.as_str().ok_or("\"text\" should be a string")?.to_owned(); }
    for (key, value, kind) in [("height", &mut t.height, Kind::Length), ("depth", &mut t.depth, Kind::Length), ("spacing", &mut t.spacing, Kind::Length), ("x", &mut t.x, Kind::Length), ("y", &mut t.y, Kind::Length), ("angle", &mut t.angle, Kind::Angle)] {
        if let Some(input) = c.get(key) { *value = s.doc.value(&text_of(input).map_err(|e| format!("\"{key}\": {e}"))?, kind)?; }
    }
    if let Some(value) = c.get("align") {
        t.align = match value.as_str() {
            Some("left") => crate::text::Align::Left,
            Some("center") => crate::text::Align::Center,
            Some("right") => crate::text::Align::Right,
            _ => return Err("\"align\" should be left, center or right".into()),
        };
    }
    if let Some(value) = c.get("operation") {
        t.op = match value.as_str().and_then(Op::parse) {
            Some(op @ (Op::New | Op::Join | Op::Cut)) => op,
            _ => return Err("text \"operation\" should be new, join or cut".into()),
        };
    }
    // Unlike numeric zero, an explicitly wrong type must not silently use a
    // default or leave the previous value in place.
    if c.get("name").is_some_and(|v| !v.is_string()) { return Err("\"name\" should be a string".into()); }
    if c.get("suppressed").is_some_and(|v| !v.is_boolean()) { return Err("\"suppressed\" should be true or false".into()); }
    if t.op == Op::New {
        if c.get("body").is_some() || c.get("face").is_some() { return Err("new text uses plane/origin; use join or cut with body/face to attach it".into()); }
        if let Some(value) = c.get("plane") {
            let origin = t.plane.origin;
            t.plane = match value.as_str().map(str::to_ascii_uppercase).as_deref() {
                Some("XY") => Plane::XY,
                Some("XZ") => Plane::XZ,
                Some("YZ") => Plane::YZ,
                _ => return Err("text \"plane\" should be XY, XZ or YZ".into()),
            };
            t.plane.origin = origin;
        }
        if let Some(value) = c.get("origin") { t.plane.origin = xyz(value).map_err(|e| format!("\"origin\": {e}"))? * s.doc.units.mm(); }
        (t.body, t.face, t.frame) = (None, None, None);
    } else {
        if c.get("plane").is_some() || c.get("origin").is_some() { return Err("attached text uses its face; omit plane/origin".into()); }
        if t.body.is_none() || c.get("body").is_some() || c.get("face").is_some() {
            let id = c["body"].as_u64().and_then(|n| Id::try_from(n).ok()).ok_or("attached text needs a \"body\" id")?;
            let body = s.built.body(id).ok_or(format!("there is no body {id}"))?;
            if !body.is_exact() { return Err("attached text needs an exact body with a flat face".into()); }
            let point = xyz(&c["face"]).map_err(|e| format!("attached text needs \"face\": [x, y, z]: {e}"))? * s.doc.units.mm();
            if !point.is_finite() { return Err("the text face point must be finite".into()); }
            let face = Face::near(body, point).ok_or("the body has no faces")?;
            let mut plane = face.plane.ok_or("text can only attach to a flat face")?;
            let surface = Item::Surface(face.tris.iter().map(|i| body.mesh.tri(*i)).collect());
            if measure::between(&Item::Point(point), &surface).distance > 1e-5 { return Err("the text face point must lie on the body's flat face".into()); }
            plane.origin = point - plane.normal() * (point - plane.origin).dot(plane.normal());
            t.plane = body.plane_to_local(plane);
            t.body = Some(id);
            t.face = Some(body.to_local(plane.origin));
            t.frame = s.built.frame(id);
        }
    }
    Ok(())
}

/// Resolve an edited face against the feature's input body, never its own
/// raised letters or geometry produced farther down the timeline. The caller
/// keeps the real session/history untouched until the edit has been validated.
fn text_face_edit_context(s: &Session, feature: Id, c: &J) -> R<Option<Session>> {
    if c.get("body").is_none() && c.get("face").is_none() { return Ok(None); }
    let body = c["body"].as_u64().and_then(|n| Id::try_from(n).ok()).ok_or("attached text needs a \"body\" id")?;
    let index = s.doc.features.iter().position(|f| f.id == feature).ok_or("the text feature no longer exists")?;
    let mut doc = s.doc.clone();
    doc.roll_to(index + 1);
    let mut context = Session::new(doc);
    if s.doc.active() != index && s.doc.active() != index + 1 {
        // The text itself may have changed the body, which is expected. Compare
        // just after this text with the currently displayed result to detect
        // later changes. Ignore kernel face IDs, which change on each rebuild.
        let same = context.built.body(body).zip(s.built.body(body)).is_some_and(|(before, now)| {
            before.mesh.len() == now.mesh.len()
                && before.mesh.tris().flatten().zip(now.mesh.tris().flatten()).all(|(a, b)| a.distance_squared(b) <= 1e-14)
        });
        if !same {
            return Err(format!("face coordinates for text feature {feature} are ambiguous because other timeline features changed the selected body; rollback to immediately before or after this text feature, select its base face again, then restore the timeline to end"));
        }
    }
    context.doc.roll_to(index);
    context.rebuild();
    if context.built.body(body).is_none() { return Err("the selected text body must exist before the text feature".into()); }
    Ok(Some(context))
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
    execute_validated(s, c, cam).map(|out| preview_provenance(s, out))
}

fn preview_provenance(s: &Session, mut out: J) -> J {
    if s.read_only && out.is_object() {
        out["geometry_trust"] = json!(s.geometry_trust());
        out["geometry_warning"] = json!("Unverified cached preview: this newer timeline cannot be rebuilt by this Ferrender.");
    }
    out
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
            if id==0 || matches!(s.doc.feature(id).map(|f|&f.kind),Some(FeatureKind::Component(_))) {return Ok(components_api::node(s,id));}
            let mut o = if s.doc.feature(id).is_some() {
                feature_info(s, id)?
            } else if let Some(body) = s.built.body(id) {
                json!({"id":id,"name":body.name,"type":"body","component":body.component})
            } else { return Err(format!("nothing has id {id}")); };
            if s.doc.sketch(id).is_some() {
                o["sketch"] = sketch_info(s, id);
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
                out.push(execute_validated(s, cmd, cam).map(|out| preview_provenance(s, out)).map_err(|e| format!("command {i} ({}) failed: {e}. The {i} before it were applied.", cmd["op"].as_str().unwrap_or("?")))?);
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
            if let Some(cache) = c["cache"].as_bool() {
                s.cache_policy = if cache { crate::doc::CachePolicy::Always } else { crate::doc::CachePolicy::Never };
            }
            let saved = s.save(&path)?;
            let mut out = json!({"saved": path.display().to_string(), "container": saved.container, "cached_bodies": saved.cached_bodies});
            if let Some(backup) = saved.backup {
                out["backup"] = json!(backup.display().to_string());
            }
            if let Some(reason) = saved.cache_skipped {
                out["cache_skipped"] = json!(reason);
            }
            Ok(out)
        }
        "export_stl" => {
            let path = c["path"].as_str().ok_or("export_stl needs a \"path\"")?;
            let unit = unit_of(c, Unit::Mm)?;
            let only = match &c["bodies"] {
                J::Null => None,
                _ => Some(ids_of(c, "bodies")?),
            };
            let component=components_api::export_component(s,c)?;
            let picked: Vec<&crate::doc::Body> = s.visible_bodies().filter(|b| only.as_ref().is_none_or(|o| o.contains(&b.id)) && component.is_none_or(|id|s.doc.component_contains(id,b.component))).collect();
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
                mesh.extend(picked.iter().flat_map(|b| b.threads.iter().flat_map(|t| t.tris())));
                mesh.face_ids.clear();
                let merged = crate::doc::Body { id: 0, name: "union".into(), component:0, placement:glam::DAffine3::IDENTITY, local_bounds:None, mesh, solids: Vec::new(), tags: Vec::new(), edges: Vec::new(), threads: Vec::new(), plain: 0 };
                let n = io::write_stl_with_provenance([&merged], unit, path.as_ref(), s.read_only)?;
                return Ok(json!({"path": path, "triangles": n, "units": unit.name(), "shells": all.len(), "open_edges": merged.mesh.open_edges(), "geometry_trust": s.geometry_trust()}));
            }
            let n = io::write_stl_with_provenance(picked, unit, path.as_ref(), s.read_only)?;
            Ok(json!({"path": path, "triangles": n, "units": unit.name(), "geometry_trust": s.geometry_trust()}))
        }
        "export_step" => {
            let path = c["path"].as_str().ok_or("export_step needs a \"path\"")?;
            let only = match &c["bodies"] {
                J::Null => None,
                _ => Some(ids_of(c, "bodies")?),
            };
            let component=components_api::export_component(s,c)?;
            let picked: Vec<&crate::doc::Body> = s.visible_bodies().filter(|b| only.as_ref().is_none_or(|o| o.contains(&b.id)) && component.is_none_or(|id|s.doc.component_contains(id,b.component))).collect();
            let skipped: Vec<Id> = picked.iter().filter(|b| !b.is_exact()).map(|b| b.id).collect();
            let solids: Vec<_> = picked.iter().flat_map(|b| &b.solids).collect();
            if solids.is_empty() {
                return Err("there are no exact bodies to write; STEP cannot hold meshes".into());
            }
            let bytes = io::step_with_provenance(solids.iter().copied(), s.read_only)?;
            std::fs::write(path, &bytes).map_err(|e| format!("could not write {path}: {e}"))?;
            Ok(json!({"path": path, "solids": solids.len(), "bytes": bytes.len(), "skipped_mesh_bodies": skipped, "geometry_trust": s.geometry_trust()}))
        }
        "get_reference" => Ok(json!(REFERENCE)),
        "text" => {
            let content = c["text"].as_str().ok_or("text needs a \"text\" string")?.to_owned();
            let mut text = Text {
                text: content, plane: Plane::XY,
                height: s.doc.value("6 mm", Kind::Length)?, depth: s.doc.value("1 mm", Kind::Length)?,
                spacing: zero(&s.doc, Kind::Length), angle: zero(&s.doc, Kind::Angle), x: zero(&s.doc, Kind::Length), y: zero(&s.doc, Kind::Length),
                align: crate::text::Align::Left, op: Op::New, body: None, face: None, frame: None, tag: None,
            };
            text_fields(s, c, &mut text)?;
            let id = s.edit_feature(|doc| {
                let id = doc.add_feature(FeatureKind::Text(text));
                if let Some(name) = c["name"].as_str() { doc.feature_mut(id).unwrap().name = name.to_owned(); }
                Ok((id, id))
            })?;
            let mut out = changed(s, &before);
            out["feature"] = json!(id);
            Ok(out)
        }
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
            let b=s.built.body(body).ok_or(format!("there is no body {body}"))?;
            let (edges, tags) = match &c["edges"] {
                J::String(all) if all == "all" => {
                    let all = exact::edges_tagged(&b.solids, &b.tags);
                    (all.iter().map(|e| e.mid).collect::<Vec<_>>(), all.into_iter().map(|e| e.tag).collect::<Vec<_>>())
                }
                _ => edge_picks(s, body, &c["edges"], "edges")?,
            };
            let edges=edges.into_iter().map(|p|b.to_local(p)).collect();
            let frame = s.built.frame(body);
            let id = s.edit_feature(|d| {
                let id = d.add_feature(FeatureKind::Blend(Blend { body, edges, size, chamfer, frame, tags }));
                Ok((id, id))
            })?;
            let mut out = changed(s, &before);
            out["feature"] = json!(id);
            Ok(out)
        }
        "shell" => {
            let body = id_of(c, "body")?;
            let thickness = s.doc.value(&text_of(&c["thickness"]).map_err(|_| "shell needs a \"thickness\"")?, Kind::Length)?;
            let b=s.built.body(body).ok_or("the body does not exist")?;
            let (faces, tags) = face_picks(s, body, &c["open_faces"], "open_faces")?;
            let faces = faces.into_iter().map(|p|b.to_local(p)).collect();
            let frame = s.built.frame(body);
            let id = s.edit_feature(|d| {
                let id = d.add_feature(FeatureKind::Shell(Shell { body, faces, thickness, frame, tags }));
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
                Ok(Item::Surface(face.tris.iter().map(|t| b.mesh.tri(*t)).collect()))
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
            let b=s.built.body(body).ok_or("the body does not exist")?;
            let at=at.into_iter().map(|p|b.to_local(p)).collect();
            let dir=b.placement.inverse().transform_vector3(dir);
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
            let t = Thread { body, face:b.to_local(face), frame: s.built.frame(body), tag: None, thread: thread.clone(), offset: opt("offset")?, length: opt("length")?, left: c["left_hand"].as_bool().unwrap_or(false), extra: opt("allowance")? };
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
        "create_component" => {
            let parent=if c["parent"].is_null() {s.doc.active_component} else {id_of(c,"parent")?};
            let activate=c["activate"].as_bool().unwrap_or(true);
            let name=c["name"].as_str().map(str::to_owned);
            let id=s.edit(|d|d.create_component(name,parent,activate))?;
            Ok(json!({"component":id,"active_component":s.doc.active_component}))
        }
        "activate_component" => {
            let id=if c["id"].is_null() {0} else {id_of(c,"id")?};
            s.edit_without_rebuild(|d|d.activate_component(id))?;
            Ok(json!({"active_component":id}))
        }
        "move_component" => {
            let id=id_of(c,"id")?;
            let placement=components_api::move_values(&s.doc,c,id)?;
            s.edit_feature(|d|{d.move_component(id,placement)?;crate::validation::document(d)?;Ok((id,()))})?;
            Ok(components_api::node(s,id))
        }
        "create_plane" => {
            let (kind,params)=planes_api::update(s,c,None)?;
            let id=s.edit_feature(|d| {
                d.params=params;
                let id=d.add_feature(FeatureKind::Plane(crate::planes::ConstructionPlane::new(kind)));
                if let Some(name)=c["name"].as_str() {d.feature_mut(id).unwrap().name=name.into();}
                crate::validation::document(d)?;
                Ok((id,id))
            })?;
            let mut out=feature_info(s,id)?; out["feature"]=json!(id); Ok(out)
        }
        "create_sketch" => {
            let on=if c["plane"].is_object() && !c["plane"]["id"].is_null() {Some(id_of(&c["plane"],"id")?)} else {None};
            if on.is_some() && !c["offset"].is_null() {return Err("offset a construction plane with create_plane before attaching a sketch".into());}
            let plane = match &c["plane"] {
                J::Null if !c["face"].is_null() => face_of(s, &c["face"])?.plane.ok_or("sketches need a flat face")?.transformed(s.built.component_placement(s.doc.active_component).inverse()),
                J::Null => Plane::XY,
                J::String(p) => match p.to_ascii_uppercase().as_str() {
                    "XY" | "TOP" => Plane::XY,
                    "XZ" | "FRONT" => Plane::XZ,
                    "YZ" | "RIGHT" => Plane::YZ,
                    _ => return Err(format!("unknown plane '{p}'; use XY, XZ or YZ")),
                },
                _ if on.is_some() => s.built.planes.get(&on.unwrap()).ok_or("the construction plane is not available")?.plane.transformed(s.built.component_placement(s.doc.active_component).inverse()),
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
            let world_plane = plane.transformed(s.built.component_placement(s.doc.active_component));
            s.edit(|d| {
                let mut sketch=Sketch::new(plane); sketch.on=on;
                let id = d.add_feature(FeatureKind::Sketch(sketch));
                if let Some(n) = name {
                    d.feature_mut(id).unwrap().name = n;
                }
                let u = d.units.mm();
                let tidy = |v: DVec3| v.to_array().map(|c| (c * 1e9).round() / 1e9 + 0.0);
                Ok(json!({"sketch": id, "origin": tidy(world_plane.origin / u), "x": tidy(world_plane.x), "y": tidy(world_plane.y), "normal": tidy(world_plane.normal())}))
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
        "point_coordinates" => {
            let sid = sketch_id(&s.doc,c)?;
            let point = if c["point"].is_null() { None } else { Some(id_of(c,"point")?) };
            let (x,y) = (text_of(&c["x"])?,text_of(&c["y"])?);
            s.edit(|d| {
                let (x,y) = (d.enter(&x,Kind::Length)?,d.enter(&y,Kind::Length)?);
                let sk = sk_mut(d,sid);
                let point = point.unwrap_or_else(|| sk.add_point(DVec2::new(x.v,y.v)));
                sk.set_point_coordinates(point,x,y)?;
                let report = settle(d,sid)?;
                sk_mut(d,sid).validate()?;
                Ok(json!({"sketch":sid,"point":point,"degrees_of_freedom":report.dof}))
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
                let constraint = &d.sketch(sid).unwrap().constraints[&cid];
                d.sketch(sid).unwrap().validate_dimension(constraint.kind, &constraint.refs, v.v)?;
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
                f => {
                    let mut face=face_of(s,f)?;
                    if let Some(body)=s.built.body(face.body) {face.prepare_exact(body);}
                    Some(face)
                },
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
            let owner=face.as_ref().and_then(|f|s.built.body(f.body)).map_or(s.doc.active_component,|b|b.component);
            let face=face.map(|f|f.transformed(s.built.component_placement(owner).inverse()));
            let c = c.clone();
            let has_bodies = s.built.bodies.iter().any(|b|b.component==owner);
            let id = s.edit_feature(|d| {
                let (sid, profs) = match &face {
                    Some(face) => {
                        let (sk, profs) = face.sketch()?;
                        let sid = d.add_feature_to(owner,FeatureKind::Sketch(sk))?;
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
                let id = d.add_feature_to(owner,kind)?;
                sk_mut(d, sid).visible = false;
                Ok((id, id))
            })?;
            {
                let mut out = changed(s, &before);
                out["feature"] = json!(id);
                Ok(out)
            }
        }
        "sweep" => {
            let owner = s.doc.active_component;
            let has_bodies = s.built.bodies.iter().any(|b| b.component == owner);
            let c = c.clone();
            let id = s.edit_feature(|d| {
                let path_sketch = match &c["path_sketch"] {
                    J::Null => return Err("sweep needs a \"path_sketch\": the sketch that holds the path".into()),
                    _ => id_of(&c, "path_sketch")?,
                };
                let along = d.sketch(path_sketch).ok_or(format!("feature {path_sketch} is not a sketch"))?;
                // The profile is the newest sketch that is not the path, unless one is named.
                let sid = match &c["sketch"] {
                    J::Null => d.sketches().filter(|(f, _)| f.id != path_sketch).last().map(|(f, _)| f.id).ok_or("sweep needs a second sketch holding the profile")?,
                    _ => sketch_id(d, &c)?,
                };
                if sid == path_sketch { return Err("the profile and the path must be in different sketches".into()); }
                let path = match &c["path"] {
                    J::Null => Vec::new(),
                    _ => ids_of(&c, "path")?,
                };
                // Report a path that is not one run now, before the feature exists.
                crate::profile::chain(along, &path)?;
                let orient = match &c["orientation"] {
                    J::Null => SweepOrient::Follow,
                    J::String(name) => SweepOrient::parse(name).ok_or(format!("unknown orientation '{name}'; use follow or fixed"))?,
                    _ => return Err("\"orientation\" should be follow or fixed".into()),
                };
                let profiles = pick_profiles(d, sid, &c)?;
                let spans = sweep_spans_of(&c)?.unwrap_or_default();
                let kind = FeatureKind::Sweep(Sweep { sketch: sid, profiles, path_sketch, path, spans, orient, op: op_of(&c, if has_bodies { Op::Join } else { Op::New })? });
                let id = d.add_feature_to(owner, kind)?;
                sk_mut(d, sid).visible = false;
                sk_mut(d, path_sketch).visible = false;
                Ok((id, id))
            })?;
            let mut out = changed(s, &before);
            out["feature"] = json!(id);
            Ok(out)
        }
        "loft" => {
            let owner = s.doc.active_component;
            let has_bodies = s.built.bodies.iter().any(|b| b.component == owner);
            let c = c.clone();
            let id = s.edit_feature(|d| {
                let sections = loft_sections_of(d, &c)?;
                let ruled = match &c["ruled"] {
                    J::Null => false,
                    J::Bool(v) => *v,
                    _ => return Err("\"ruled\" should be true or false".into()),
                };
                let sketches: Vec<Id> = sections.iter().map(|section| section.sketch).collect();
                let kind = FeatureKind::Loft(Loft { sections, ruled, op: op_of(&c, if has_bodies { Op::Join } else { Op::New })? });
                let id = d.add_feature_to(owner, kind)?;
                for sid in sketches { sk_mut(d, sid).visible = false; }
                Ok((id, id))
            })?;
            let mut out = changed(s, &before);
            out["feature"] = json!(id);
            Ok(out)
        }
        "remove_body" | "split_body" => {
            let id=body_ops_api::create(s,c)?;
            let mut out=changed(s,&before);
            out.as_object_mut().unwrap().extend(feature_info(s,id)?.as_object().unwrap().clone());
            out["feature"]=json!(id);
            Ok(out)
        }
        "primitive" => {
            let primitive = primitives_api::create(&s.doc,c)?;
            let id = s.edit_feature(|d| {
                let id = d.add_feature(FeatureKind::Primitive(primitive));
                crate::validation::document(d)?;
                Ok((id,id))
            })?;
            let mut out = changed(s,&before);
            out.as_object_mut().unwrap().extend(feature_info(s,id)?.as_object().unwrap().clone());
            out["feature"] = json!(id);
            Ok(out)
        }
        "pattern" => {
            let source = id_of(c, "feature")?;
            let axis = |key: &str, default: usize| match c[key].as_str().map(str::to_ascii_lowercase).as_deref() {
                None if c[key].is_null() => Ok(default),
                None => Err(format!("{key} must be x, y or z")),
                Some("x") => Ok(0),
                Some("y") => Ok(1),
                Some("z") => Ok(2),
                Some(o) => Err(format!("unknown axis '{o}'; use x, y or z")),
            };
            let count = |key: &str| c[key].as_u64().and_then(|n|u32::try_from(n).ok()).ok_or(format!("{key} must be a positive whole number"));
            let has_second = ["axis2", "count2", "spacing2"].iter().any(|key| !c[*key].is_null());
            if has_second && !matches!(c["type"].as_str(), Some("linear" | "rectangular")) {
                return Err("only linear patterns support a second direction".into());
            }
            let kind = match c["type"].as_str() {
                Some("circular") => PatternKind::Circular {
                    axis: axis("axis", 2)?,
                    count: count("count")?,
                    angle: match &c["angle"] {
                        J::Null => s.doc.value("360", Kind::Angle)?,
                        v => s.doc.value(&text_of(v)?, Kind::Angle)?,
                    },
                },
                Some("linear" | "rectangular") => {
                    let second = if has_second {
                        if ["axis2", "count2", "spacing2"].iter().any(|key| c[*key].is_null()) {
                            return Err("a second pattern direction needs axis2, count2 and spacing2 together".into());
                        }
                        Some(LinearDirection { axis: axis("axis2", 1)?, count: count("count2")?, spacing: s.doc.value(&text_of(&c["spacing2"])?, Kind::Length)? })
                    } else { None };
                    PatternKind::Linear { axis: axis("axis", 0)?, count: count("count")?, spacing: s.doc.value(&text_of(&c["spacing"]).map_err(|_| "a linear pattern needs a \"spacing\"")?, Kind::Length)?, second }
                },
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
            let face = if op == "project" { Some(face_of(s, c)?.transformed(s.built.component_placement(s.doc.feature(sid).unwrap().owner).inverse())) } else { None };
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
        // A new name alone changes no geometry, so it does not rebuild.
        "edit_feature" if renames_only(c) => {
            let id = id_of(c, "feature")?;
            let name = c["name"].as_str().unwrap_or_default().to_owned();
            s.edit_without_rebuild(|d| {
                d.feature_mut(id).ok_or(format!("there is no feature {id}"))?.name = name;
                Ok(())
            })?;
            feature_info(s, id)
        }
        "edit_feature" => {
            if !c["owner"].is_null() {return Err("moving features between components is not supported; activate a component before creating features".into());}
            let id = id_of(c, "feature")?;
            let c = c.clone();
            let body_op_update=body_ops_api::update(s,id,&c)?;
            let plane_update=match s.doc.feature(id).map(|f|&f.kind) {
                Some(FeatureKind::Plane(p)) if ["kind","base","distance","faces","flip","points"].iter().any(|key| !c[key].is_null()) => {
                    let mut prefix=s.doc.clone();
                    prefix.roll_to(prefix.features.iter().position(|f|f.id==id).unwrap());
                    let update=planes_api::update(&Session::new(prefix),&c,Some(&p.kind))?;
                    let mut trial=s.doc.clone(); trial.params=update.1.clone();
                    let feature=trial.feature_mut(id).unwrap();
                    let FeatureKind::Plane(p)=&mut feature.kind else {unreachable!()};
                    p.kind=update.0.clone(); feature.suppressed=false;
                    let index=trial.features.iter().position(|f|f.id==id).unwrap();
                    trial.roll_to(index+1);
                    crate::validation::document(&trial)?;
                    let built=trial.rebuild();
                    if let Some(e)=built.errors.get(&id) {return Err(e.clone());}
                    if !built.planes.contains_key(&id) {return Err("activate or unsuppress the plane's component before editing it".into());}
                    Some(update)
                }
                _=>None,
            };
            let text_update = match s.doc.feature(id).map(|f| &f.kind) {
                Some(FeatureKind::Text(t)) => {
                    let mut t = t.clone();
                    let context = text_face_edit_context(s, id, &c)?;
                    text_fields(context.as_ref().unwrap_or(s), &c, &mut t)?;
                    if let Some(body)=t.body && s.doc.body_owner(body)!=s.doc.feature(id).map(|f|f.owner) {
                        return Err("text cannot be moved between components; create new text on the target body instead".into());
                    }
                    let index = s.doc.features.iter().position(|f| f.id == id).unwrap();
                    if s.doc.active() <= index {
                        // A rolled-back feature is not built by edit_feature.
                        // Validate it in a temporary prefix before accepting the
                        // change, keeping the user's actual timeline untouched.
                        let mut preview = s.doc.clone();
                        let feature = preview.feature_mut(id).unwrap();
                        feature.kind = FeatureKind::Text(t.clone());
                        feature.suppressed = false;
                        preview.roll_to(index + 1);
                        if let Some(error) = preview.rebuild().errors.get(&id) { return Err(error.clone()); }
                    }
                    Some(t)
                }
                _ => None,
            };
            s.edit_feature(|d| {
                if let Some((_,params))=&plane_update {d.params=params.clone();}
                let probe = d.clone();
                let f = d.feature_mut(id).ok_or(format!("there is no feature {id}"))?;
                if let Some(n) = c["name"].as_str() {
                    f.name = n.to_owned();
                }
                if let Some(v) = c["suppressed"].as_bool() {
                    f.suppressed = v;
                }
                if let Some(kind)=body_op_update {f.kind=kind;}
                match &mut f.kind {
                    FeatureKind::Plane(p) => {if let Some((kind,_))=plane_update {p.kind=kind;}}
                    FeatureKind::Text(t) => { *t = text_update.expect("text feature update was prepared"); }
                    FeatureKind::Primitive(p) => primitives_api::update(&probe,&c,p)?,
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
                    FeatureKind::Sweep(w) => {
                        if !c["path"].is_null() {
                            let path = ids_of(&c, "path")?;
                            crate::profile::chain(probe.sketch(w.path_sketch).ok_or("its path sketch was deleted")?, &path)?;
                            w.path = path;
                        }
                        if let Some(spans) = sweep_spans_of(&c)? { w.spans = spans; }
                        if let Some(name) = c["orientation"].as_str() {
                            w.orient = SweepOrient::parse(name).ok_or(format!("unknown orientation '{name}'; use follow or fixed"))?;
                        }
                        w.op = op_of(&c, w.op)?;
                    }
                    FeatureKind::Loft(l) => {
                        if !c["sections"].is_null() {
                            l.sections = loft_sections_of(&probe, &c)?;
                        }
                        match &c["ruled"] {
                            J::Null => {}
                            J::Bool(v) => l.ruled = *v,
                            _ => return Err("\"ruled\" should be true or false".into()),
                        }
                        l.op = op_of(&c, l.op)?;
                    }
                    _ => {}
                }
                crate::validation::document(d)?;
                Ok((id, ()))
            })?;
            feature_info(s, id)
        }
        "delete_feature" => {
            let id=id_of(c,"feature")?;
            let removed=s.edit(|d|d.delete_feature(id))?;
            let mut out=changed(s,&before);out["removed_features"]=json!(removed);Ok(out)
        }
        "import_stl" | "import_mesh" => {
            let path = c["path"].as_str().ok_or("import_mesh needs a \"path\"")?;
            let (mesh, report) = io::import_mesh(path.as_ref(), unit_of(c, Unit::Mm)?)?;
            let name = std::path::Path::new(path).file_stem().map(|n| n.to_string_lossy().into_owned());
            let id = s.edit(|d| {
                let id = d.add_feature(FeatureKind::Import(mesh));
                if let Some(n) = name {
                    d.feature_mut(id).unwrap().name = n;
                }
                Ok(id)
            })?;
            Ok(json!({"feature": id, "body": body_info(s, id), "report": report, "summary": report.summary()}))
        }
        "script_meta" => {
            let source = match (c["source"].as_str(), c["path"].as_str()) {
                (Some(s), _) => s.to_owned(),
                (None, Some(p)) => crate::script::read_source(std::path::Path::new(p))?,
                _ => return Err("script_meta needs \"source\" or \"path\"".into()),
            };
            Ok(serde_json::to_value(crate::script::meta(&source)?).unwrap_or(J::Null))
        }
        "run_script" => {
            let (source, dir) = match (c["source"].as_str(), c["path"].as_str()) {
                (Some(s), _) => (s.to_owned(), None),
                (None, Some(p)) => (crate::script::read_source(std::path::Path::new(p))?, std::path::Path::new(p).parent().map(std::path::Path::to_path_buf)),
                _ => return Err("run_script needs \"source\" or \"path\"".into()),
            };
            let mut req = crate::script::Request::new(source);
            req.script_dir = dir.clone();
            req.inputs = if c["inputs"].is_object() { c["inputs"].clone() } else { json!({}) };
            req.sandbox.yes = c["yes"].as_bool().unwrap_or(true);
            req.time_limit = match &c["timeout"] {
                J::Null => None,
                v => Some(crate::script::timeout_duration(v.as_f64().ok_or("\"timeout\" is a positive number of seconds")?)?),
            };
            req.sandbox.allowed = c["allow"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).map(std::path::PathBuf::from).collect()).unwrap_or_default();
            if let Some(d) = dir { req.sandbox.allowed.push(d); }
            if let Some(d) = s.path.as_ref().and_then(|p| p.parent()) { req.sandbox.allowed.push(d.to_path_buf()); }
            // Run on a worker-style copy. Commands such as open/new replace the
            // entire session; a document-only rollback could otherwise leave the
            // caller pointing at another file, or discard its undo/redo history.
            let mut working = s.fork();
            let outcome = crate::script::run(&mut working, &req)?;
            if !outcome.cancelled && working.doc != s.doc {
                // Match the GUI: script edits apply to this design as one undo
                // step. Files opened/exported inside the script do not rename it.
                s.edit(|doc| { *doc = working.doc; Ok(()) })?;
            }
            let mut out = serde_json::to_value(&outcome).unwrap_or(J::Null);
            out["errors"] = json!(s.built.errors.iter().map(|(id, e)| json!({"feature": id, "error": e})).collect::<Vec<_>>());
            Ok(out)
        }
        "add_feature" => {
            // A feature as the file stores it, for scripts that replay an exported timeline.
            // Large embedded data may come from a file beside the script instead:
            // "mesh_path" for an import's mesh, "image_path" for a relief's or sketch's image.
            let mut value = c["feature"].clone();
            if let Some(p) = c["image_path"].as_str() {
                let inner = value.get_mut("kind").and_then(J::as_object_mut).and_then(|k| k.values_mut().next()).and_then(J::as_object_mut)
                    .ok_or("\"image_path\" applies to a relief or a sketch with a reference image")?;
                let key = if inner.contains_key("image") { "image" } else { "reference" };
                let slot = inner.get_mut(key).ok_or("\"image_path\" applies to a relief or a sketch with a reference image")?;
                let placement: crate::reference::ReferenceImage = serde_json::from_value(slot.clone()).map_err(|e| format!("add_feature: the image placement is malformed: {e}"))?;
                let image = placement.with_pixels_from(std::path::Path::new(p))?;
                *slot = serde_json::to_value(&image).map_err(|e| e.to_string())?;
            }
            let mut feature: crate::Feature = serde_json::from_value(value).map_err(|e| format!("add_feature needs a \"feature\" as the file format writes one: {e}"))?;
            if let Some(p) = c["mesh_path"].as_str() {
                let FeatureKind::Import(mesh) = &mut feature.kind else { return Err("\"mesh_path\" applies to an import feature".into()) };
                *mesh = io::import_mesh(std::path::Path::new(p), unit_of(c, Unit::Mm)?)?.0;
            }
            let id = s.edit_feature(|d| {
                if feature.id == 0 || d.feature(feature.id).is_some() { feature.id = d.next_id; }
                d.next_id = d.next_id.max(feature.id + 1);
                if !d.features.iter().any(|f| f.id == feature.owner) && feature.owner != 0 { feature.owner = d.active_component; }
                let id = feature.id;
                d.features.push(feature);
                Ok((id, id))
            })?;
            let mut out = changed(s, &before);
            out["feature"] = json!(id);
            Ok(out)
        }
        "mesh_measure" => {
            let body = id_of(c, "body")?;
            let b = s.built.body(body).ok_or(format!("there is no body {body}"))?;
            let u = s.doc.units.mm();
            let at = if c["at"].is_null() { None } else { Some(b.to_local(xyz(&c["at"])? * u)) };
            let mut m = b.bare();
            if !m.is_welded() { m.weld_exact(); }
            let mut measure = crate::meshops::measure(&m, at);
            measure.volume /= u.powi(3);
            measure.area /= u.powi(2);
            measure.min = measure.min.map(|v| v / u);
            measure.max = measure.max.map(|v| v / u);
            measure.thickness_at = measure.thickness_at.map(|t| t / u);
            let mut out = serde_json::to_value(measure).unwrap_or(J::Null);
            out["body"] = json!(body);
            out["kind"] = json!(if b.is_exact() { "exact" } else { "mesh" });
            Ok(out)
        }
        "mesh_repair" | "mesh_decimate" | "mesh_smooth" | "mesh_subdivide" | "mesh_cut" | "mesh_mirror" | "mesh_offset" | "mesh_extrude_region" | "mesh_sculpt" => {
            use crate::doc::MeshOpKind;
            use crate::meshops::{Brush, DecimateMethod, Keep, Scheme};
            let body = id_of(c, "body")?;
            let u = s.doc.units.mm();
            let num = |key: &str, default: f64| -> R<f64> { if c[key].is_null() { Ok(default) } else { c[key].as_f64().ok_or_else(|| format!("\"{key}\" must be a number")) } };
            let int = |key: &str, default: u64| -> R<u32> {
                let value = if c[key].is_null() { default } else { c[key].as_u64().ok_or_else(|| format!("\"{key}\" must be a whole number"))? };
                u32::try_from(value).map_err(|_| format!("\"{key}\" exceeds the supported whole-number range"))
            };
            let direction = if c["direction"].is_null() { None } else { Some(xyz(&c["direction"])?) };
            let distance = || s.doc.value(&text_of(&c["distance"]).map_err(|_| format!("{op} needs a \"distance\""))?, Kind::Length);
            let kind = match op {
                "mesh_repair" => MeshOpKind::Repair { fill_holes: int("fill_holes", 0)? },
                "mesh_decimate" => MeshOpKind::Decimate {
                    target: int("target", 0).and_then(|t| if t >= 4 { Ok(t) } else { Err("mesh_decimate needs a \"target\" of at least 4 triangles".into()) })?,
                    method: match c["method"].as_str().unwrap_or("quadric") { "quadric" => DecimateMethod::Quadric, "cluster" => DecimateMethod::Cluster, other => return Err(format!("unknown decimation method {other}; use quadric or cluster")) },
                    preserve_boundary: c["preserve_boundary"].as_bool().unwrap_or(true),
                },
                "mesh_smooth" => MeshOpKind::Smooth { iterations: int("iterations", 10)?, strength: num("strength", 0.5)? },
                "mesh_subdivide" => MeshOpKind::Subdivide { levels: int("levels", 1)?, scheme: match c["scheme"].as_str().unwrap_or("loop") { "loop" => Scheme::Loop, "midpoint" => Scheme::Midpoint, other => return Err(format!("unknown subdivision scheme {other}; use loop or midpoint")) } },
                "mesh_cut" => MeshOpKind::Cut { plane: planes_api::base(s, &c["plane"])?, keep: match c["keep"].as_str().unwrap_or("negative") { "negative" => Keep::Negative, "positive" => Keep::Positive, "both" => Keep::Both, other => return Err(format!("unknown side {other}; keep negative, positive or both")) }, cap: c["cap"].as_bool().unwrap_or(true) },
                "mesh_mirror" => MeshOpKind::Mirror { plane: planes_api::base(s, &c["plane"])?, weld: c["weld"].as_bool().unwrap_or(true) },
                "mesh_offset" => MeshOpKind::Offset { distance: distance()?, direction },
                "mesh_sculpt" => {
                    let b = s.built.body(body).ok_or(format!("there is no body {body}"))?;
                    let brush = match c["brush"].as_str().unwrap_or("pull") { "push" => Brush::Push, "pull" => Brush::Pull, "inflate" => Brush::Inflate, "smooth" => Brush::Smooth, "flatten" => Brush::Flatten, other => return Err(format!("unknown brush {other}; use push, pull, inflate, smooth or flatten")) };
                    let strength_kind = if matches!(brush, Brush::Smooth | Brush::Flatten) { Kind::Scalar } else { Kind::Length };
                    MeshOpKind::Sculpt {
                        brush,
                        at: b.to_local(xyz(&c["at"]).map_err(|_| "mesh_sculpt needs \"at\": [x,y,z] on the body")? * u),
                        radius: s.doc.value(&text_of(&c["radius"]).map_err(|_| "mesh_sculpt needs a \"radius\"")?, Kind::Length)?,
                        strength: s.doc.value(&text_of(&c["strength"]).map_err(|_| "mesh_sculpt needs a \"strength\" (a length for push, pull and inflate; 0 to 1 for smooth and flatten)")?, strength_kind)?,
                    }
                }
                _ => MeshOpKind::ExtrudeRegion { distance: distance()?, direction },
            };
            let region = if c["region"].is_null() { None } else { Some(region_of(s, &c["region"], u)?) };
            if op == "mesh_extrude_region" && region.is_none() { return Err("mesh_extrude_region needs a \"region\"".into()); }
            let id = s.edit_feature(|d| {
                let id = d.add_feature(FeatureKind::MeshOp(crate::doc::MeshOp { body, op: kind, region }));
                Ok((id, id))
            })?;
            let mut out = changed(s, &before);
            out["feature"] = json!(id);
            if let Some(b) = body_info(s, body) { out["body"] = b; }
            Ok(out)
        }
        "mesh_from_image" => {
            let path = c["path"].as_str().ok_or("mesh_from_image needs a \"path\" to a PNG or JPEG")?;
            let u = s.doc.units.mm();
            let image = crate::reference::ReferenceImage::from_file(std::path::Path::new(path), 100.0)?;
            let value = |key: &str, default: &str| -> R<Value> { s.doc.value(&if c[key].is_null() { default.to_owned() } else { text_of(&c[key])? }, Kind::Length) };
            let plane = match &c["plane"] {
                J::Null => Plane::XY,
                J::String(p) => match p.as_str() { "XY" => Plane::XY, "XZ" => Plane::XZ, "YZ" => Plane::YZ, other => return Err(format!("unknown plane {other}; use XY, XZ or YZ")) },
                v => Plane::from_normal(xyz(&v["origin"]).unwrap_or(DVec3::ZERO) * u, xyz(&v["normal"])?),
            };
            let plane = if c["origin"].is_null() { plane } else { Plane { origin: xyz(&c["origin"])? * u, ..plane } };
            let integer = |key: &str, default: u32| -> R<u32> {
                if c[key].is_null() { return Ok(default); }
                let value = c[key].as_u64().ok_or_else(|| format!("{key} must be a whole number"))?;
                u32::try_from(value).map_err(|_| format!("{key} exceeds the supported whole-number range"))
            };
            let relief = crate::doc::Relief {
                image, plane,
                width: value("width", "100 mm")?, depth: value("depth", "4 mm")?, base: value("base", "2 mm")?,
                resolution: integer("resolution", 300)?,
                invert: c["invert"].as_bool().unwrap_or(false),
                blur: integer("blur", 1)?,
                gamma: c["gamma"].as_f64().unwrap_or(1.0),
                op: op_of(c, Op::New)?,
            };
            let name = std::path::Path::new(path).file_stem().map(|n| n.to_string_lossy().into_owned());
            let id = s.edit_feature(|d| {
                let id = d.add_feature(FeatureKind::Relief(relief));
                if let Some(n) = name { d.feature_mut(id).unwrap().name = n; }
                Ok((id, id))
            })?;
            let mut out = changed(s, &before);
            out["feature"] = json!(id);
            Ok(out)
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
            if s.doc.sketch(id).is_none() && s.built.body(id).is_none() && !matches!(s.doc.feature(id).map(|f|&f.kind),Some(FeatureKind::Plane(_) | FeatureKind::Component(_))) {
                return Err(format!("{id} is not a sketch, plane, component, or body"));
            }
            s.edit_without_rebuild(|d| {
                if let Some(sk) = d.sketch_mut(id) {
                    sk.visible = visible;
                } else if let Some(FeatureKind::Plane(p))=d.feature_mut(id).map(|f|&mut f.kind) {
                    p.visible=visible; p.visibility_pinned=true;
                } else if let Some(FeatureKind::Component(component))=d.feature_mut(id).map(|f|&mut f.kind) {
                    component.visible=visible;
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
