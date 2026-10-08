# Construction planes in Ferrender 0.3

Status: **implemented in the 0.3.0-dev test candidate; local automated verification passed; manual acceptance pending. Not released.** Target: 0.3.0, alongside [components](COMPONENTS_0.3.md). The specification below defines the intended behavior; it is not a record of passed tests. Use the consolidated [0.3.0 manual test plan](TEST_PLAN_0.3.0.md) to report acceptance results. A public release waits for user verification.

In 0.2.2 a sketch could only start on an origin plane, on a flat face, or on a one-shot offset of either (the **Offset** field in the New Sketch dialog copies the plane and forgets where it came from). A construction plane is a plane that is a feature in its own right: it is visible in the viewport, listed in the browser, built from references that it keeps, and re-evaluated on every rebuild. A sketch drawn on it follows it when the referenced face moves, a parameter changes, or the three points it passes through are edited.

The 0.3.0 candidate includes three kinds:

| Kind | References | Result |
|---|---|---|
| **Offset** | a flat face, an origin plane or another construction plane, plus a distance | the same plane shifted along its normal; positive is out of the face, negative is into the body, set the way Extrude sets its distance |
| **Midplane** | two flat faces | the plane halfway between parallel faces, or the bisecting plane of faces that meet at an angle |
| **Three points** | three points: body vertices, sketch points or typed coordinates | the plane through them |

## What the user sees

### Creating a plane

**Model → Construction Plane** (toolbar button beside New Sketch; shortcut unassigned) opens one dialog with a Kind selector: Offset, Midplane, Three Points. The dialog's prompt line in the viewport changes with the kind, like the Extrude dialog does.

**Offset.** Click a flat face, an origin plane in the browser, or an existing construction plane. The dialog shows the chosen base and a **Distance** field that accepts units, expressions and `name = expr` like every other value field. An arrow stands on the base, exactly as the Extrude arrow stands on a profile: drag it to set the distance, it lands on round numbers, and pulling it back through the base makes the distance negative and puts the plane on the other side. **To face** lets you click another parallel flat face and measures the signed distance for you. Antiparallel face normals are accepted; a nonparallel target leaves the current distance unchanged and explains that a parallel face is needed. The preview plane moves live. `Flip` is unnecessary; the sign does the work.

**Midplane.** Click two flat faces. The preview appears after the second click. For parallel faces the plane sits halfway between them with the first face's orientation. For faces that meet at an angle, the plane bisects them through their line of intersection; a **Flip** checkbox picks the other bisector. Picking the same face twice is refused.

**Three points.** Click three points: a vertex of a body (ends of the exact edges the viewport already draws), a point of a visible sketch, or type coordinates into three X/Y/Z rows (any mix). Collinear or coincident points are refused with a message, not a degenerate plane.

OK adds a `PlaneN` feature to the timeline. Enter confirms, Escape cancels, as in the other dialogs.

### Using a plane

- **New Sketch** (the `PickPlane` dialog) says "Choose a plane, click a flat face, or click a construction plane." Clicking a plane in the viewport or in the browser's `Construction` folder starts the sketch on it. The existing Offset field stays as a quick, non-parametric shortcut for origin planes and faces. When selecting a construction plane it must be blank or zero, so the new sketch always retains its attachment. A nonzero value is refused with an instruction to clear it or create another Offset construction plane.
- The plane is drawn as a translucent rectangle with an outline and a small name label. It is sized to its references (an offset of a face covers that face's outline with a margin; a midplane covers both faces; a three-point plane covers its triangle with a margin; a plane based on an origin plane is sized to the model's bounds, or 50 mm when the model is empty). The side its normal faces is shaded a little lighter, so Offset's sign and Midplane's Flip are legible.
- The browser gets a `Construction` folder with an eye per plane. Planes are hidden automatically when a sketch is drawn on them and the sketch is finished, the way a sketch is hidden after it is extruded, but only if the user has not toggled the eye by hand.
- Double-click the chip to edit any of the inputs; right-click for rename, suppress and delete like other features.
- Edges of a construction plane are not pickable for dimensions; a plane is not geometry.

### What follows a plane

A sketch created on a plane remembers it (`Sketch.on`). On rebuild the sketch's plane is replaced by the plane's current value before anything uses it, so extrudes, revolves and text on the sketch move with it. Face references inside a plane use the same point-and-bounds re-finding as fillets, shells and text (`exact::candidates`), so the plane survives the body changing size and reports an error when the face is gone.

## Data model (`crates/fr-core`)

```rust
// doc.rs
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaneRef {
    /// XY, XZ or YZ of the owning component.
    Origin(OriginPlane),
    /// A flat face, named the way Blend/Shell/Text name faces.
    Face { body: Id, at: DVec3, frame: Option<[DVec3; 2]> },
    /// Another construction plane, which must come earlier in the timeline.
    Plane(Id),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointRef {
    /// Legacy name: a literal in the owning component's frame, stored in mm.
    World(DVec3),
    /// An end of an exact edge, re-found by position within the body's bounds.
    Vertex { body: Id, at: DVec3, frame: Option<[DVec3; 2]> },
    SketchPoint { sketch: Id, point: Id },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaneKind {
    Offset { base: PlaneRef, distance: Value },
    Midplane { a: PlaneRef, b: PlaneRef, flip: bool },
    ThreePoint { points: [PointRef; 3] },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConstructionPlane {
    pub kind: PlaneKind,
    #[serde(default = "yes")]
    pub visible: bool,
    /// Set when the user toggled the eye, so auto-hide leaves it alone.
    #[serde(default)]
    pub visibility_pinned: bool,
}

pub enum FeatureKind {
    // ...existing...
    Plane(ConstructionPlane),
}
```

`Sketch` gains `#[serde(default)] pub on: Option<Id>`. `Sketch.plane` stays as the cached, last-resolved value so the file is still self-describing and the solver, which is purely 2D, never needs the plane feature.

`Built` gains `planes: BTreeMap<Id, ResolvedPlane>` with the `Plane`, the display rectangle (four world corners) and the error state, so the viewport and API read results rather than re-deriving them.

`Feature::type_name` returns `"plane"`, so auto-names are `Plane1`, `Plane2`. Icon: a square outline (`icon::SQUARE` or `icon::FRAME_CORNERS`).

### Resolving each kind

All of this is in a new `crates/fr-core/src/planes.rs` with pure functions that take resolved inputs, plus `Document::resolve_plane(&self, f: &Feature, bodies: &[Body], planes: &BTreeMap<Id, Plane>) -> Result<Plane, String>` that looks the references up.

**Reference lookup.** `PlaneRef::Origin` returns the constant plane. `PlaneRef::Face` finds the body by id (error: "a body it used no longer exists"), re-finds the face with `exact::candidates(&body.solids, &[at], frame)` and `Face::near`, and requires `face.plane` (error: "the face is no longer flat, or is gone; edit the plane and pick it again"). Mesh bodies work too through `Face::near`'s mesh path. `PlaneRef::Plane` reads `planes` (error: "the plane it is offset from was deleted or comes later in the timeline"). `PointRef::Vertex` uses the same candidates mechanism against the ends of `exact::edges`; `PointRef::SketchPoint` maps the sketch point through its source component into the owning component's local frame. `PointRef::World` is a legacy enum name: its literal is already in the owning component's frame, in millimeters. Cross-component face and vertex references are converted from the source frame to the owner frame during resolution.

**Offset.** `base.offset(distance.v)` using the existing `Plane::offset`, which keeps the base's x and y axes so a sketch on the offset lines up with the face below it. A face base keeps the orientation `Face::pick` gives it (origin nearest the world origin, x horizontal).

**Midplane.** With unit normals `na`, `nb` and origins `oa`, `ob`:

- If `|na · nb| > 1 − 1e-6` the faces are parallel. Normal `n = na`; origin is the midpoint of `oa` and `ob` projected onto `n` (`oa + n * ((ob − oa) · n) / 2`), with the in-plane position at the centroid of both faces' outlines projected onto the result, so the rectangle lands between the faces. Axes come from `Plane::from_normal` on the first face's normal, then rotated to match face a's `x` so sketches line up with it. Opposite faces of a plate have antiparallel normals; the parallel test uses the absolute dot product so they count as parallel.
- Otherwise the faces meet along the line `L` where the two planes intersect. The two bisectors through `L` have normals `na − nb` and `na + nb`. The default is the one that bisects the angle on the material side: `normalize(na − nb)`, which is also the limit that agrees with the parallel case as the faces turn parallel; `flip` chooses `normalize(na + nb)`. Origin is the point on `L` nearest the midpoint of the two faces' centroids; `x` runs along `L`.
- Same face picked twice, or two references that resolve to the same plane, is an error.

**Three points.** `p0, p1, p2`. Require `|(p1 − p0) × (p2 − p0)| > 1e-9 · max(1, |p1 − p0| · |p2 − p0|)` (error: "the three points are in a line; move one"). Origin `p0`, `x = normalize(p1 − p0)`, `y = n × x` with `n = normalize((p1 − p0) × (p2 − p0))`.

Every result goes through `Sketch::new(plane).validate()`'s plane check (finite, unit, perpendicular axes) before it is published.

### Display rectangle

Computed once per rebuild and stored in `Built.planes`:

- Offset of a face: the face's `loops` in the plane's coordinates, bounding box, 15 % margin, at least 10 mm.
- Offset of an origin plane or of another plane: that plane's rectangle (recursively), or the model's bounds projected onto the plane with a margin, or a 50 mm square when there are no bodies.
- Midplane: both faces' outlines projected onto the result, bounding box, margin.
- Three points: the triangle's bounding box in plane coordinates, margin, at least 10 mm.

## Rebuild changes

`Document::rebuild` currently evaluates values and solves every sketch in a first pass, then builds bodies in a second pass that skips sketches. Planes depend on bodies (a face) and sketches depend on planes, so plane resolution and sketch plane assignment have to happen **in timeline order inside the body pass**:

```text
for each active, unsuppressed feature in order:
    Plane     → resolve against current bodies and planes so far; store in built.planes (or built.errors)
    Sketch    → if sketch.on = Some(p): copy built.planes[p] into sketch.plane, or record an error on the sketch
    otherwise → apply as today
```

Two consequences:

1. The loop mutates `self.features` (the sketch's cached plane) while iterating, so it switches to indices rather than `self.features.iter()`. The sketch solve stays in the first pass; it never reads the plane.
2. A feature whose sketch has an error should fail rather than build on a stale plane. `tool()`'s `sk(id)` helper takes `&built.errors` and returns "its sketch could not be placed: {reason}". Today a sketch is never in `errors` except for unsatisfiable constraints, where features already fail at the kernel; making this explicit is cheap and keeps geometry honest.

Value evaluation in the first pass covers `PlaneKind::Offset.distance` (`Kind::Length`) and the `World` coordinates of three-point planes if they are made expressions; the plan keeps `World` points as plain `DVec3` in 0.3.0, entered along the owning component's axes in document units and converted to millimeters on entry. The dialog accepts expressions for entry but stores their evaluated literal coordinates; those coordinates do not retain a parameter link.

Rollback and suppression work without special cases: a plane rolled past is not in `built.planes`, so a sketch on it errors with "its plane is rolled back or suppressed", and features on that sketch fail.

With [components](COMPONENTS_0.3.md) in place, a plane is owned by a component like any feature, `PlaneRef::Origin` means the owning component's origin, and references into other components' bodies are allowed (the body is found by id). Resolution runs in local coordinates like every other feature.

## App changes (`crates/ferrender`)

- `Dialog::Plane(PlaneDlg { editing, kind, base, faces, points, text (distance), pick_to, flip, error })`.
- `Action::Plane`, menu item, toolbar button, `panels::dialogs` arm with the kind selector and per-kind rows; reuse `value_row` for Distance, `confirm` for OK/Cancel.
- **Arrow.** Generalize `view::extrude_arrow` and `pull_arrow` into `distance_arrow(app, plane: Plane, middle: DVec2, text: &str)` returning the same `Arrow`, used by both the Extrude dialog and the Offset plane dialog, with the drag target chosen by `app.dialog`. `Drag::Arrow` stays as is.
- **Picking in `model_mode`.** New arm for `Dialog::Plane`: Offset takes a face (`pick_face`) or a plane click; Midplane takes two faces; Three Points takes vertices (new `pick_vertex`: nearest end of `body.edges` within a pixel radius, drawn as a dot on hover) or sketch points (`hit` on visible sketches, already available through `Hit::Point`). The browser's `Origin` links and `Construction` entries feed the dialog when it is open instead of starting a sketch.
- **Drawing.** `view::draw_planes`: for each visible plane in `Built.planes`, the quad as a filled translucent polygon (two tones for front and back, using the camera to decide which faces the viewer), the outline, the label, and a highlight on hover when `PickPlane` or the plane dialog is open. Drawn after bodies, before sketches, with depth testing off like sketches so it reads through geometry; it should be faint enough not to hide the model.
- **Pick a plane** in `PickPlane`: ray-plane hit inside the rectangle; the nearest plane along the ray wins over a face only when the plane is in front of the hit face (compare ray parameters).
- Browser `Construction` folder, auto-hide on sketch finish (with the `visibility_pinned` rule), `feature_menu` for planes, chip icon.
- Status bar prompts for each dialog state, in `view::viewport`'s prompt table.
- Edit: `App::edit_feature` for `Plane` opens the dialog prefilled; `FeatureKind::Plane` added to the edit-feature match in `app.rs`.

## API and MCP (`api.rs`)

```
{"op":"create_plane","kind":"offset","base":"XY","distance":"10 mm"}
{"op":"create_plane","kind":"offset","base":{"face":{"body":ID,"point":[x,y,z]}},"distance":"-3 mm"}
{"op":"create_plane","kind":"offset","base":{"plane":ID},"distance":"$gap / 2"}
{"op":"create_plane","kind":"midplane","faces":[{"body":ID,"point":[x,y,z]},{"body":ID,"point":[x,y,z]}],"flip":false}
{"op":"create_plane","kind":"three_point","points":[[x,y,z],{"body":ID,"point":[x,y,z]},{"sketch":ID,"point":PID}]}
    → {"feature": ID, "origin": [...], "x": [...], "y": [...], "normal": [...]}
{"op":"create_sketch","plane":{"id":ID}}          a sketch on construction plane ID
{"op":"edit_feature","feature":ID, ...same fields as create_plane...}
```

- `get_object_info` on a plane returns its kind, inputs, resolved `origin`, `x`, `y`, `normal` and any error.
- `get_scene_info` lists planes among features as it does now (type `"plane"`); nothing new to add.
- `set_visible` accepts a plane id.
- Literal three-point coordinate arrays use the owning component's local frame in document units. `PointRef::World` stores those literals in millimeters despite its legacy name.
- Face and vertex pick points come in world coordinates in document units, like `fillet_edges` and `hole`, and are stored as the found candidate point in the body's local frame, like `Blend`.
- Sketch query `plane` reports the resolved world frame; `local_plane` explicitly reports the owner-local frame. Both origins use document units; axes and normals are unit vectors. A plane feature query likewise reports its resolved world frame.
- `REFERENCE` gains a short "Construction planes" block next to `create_sketch`.

## File format

`io::FORMAT_VERSION` becomes 5 (shared with components). `to_json` writes 5 when any feature is a `Plane` or any sketch has `on` set. Old files have no planes and load unchanged.

Validation (`validation.rs`): plane references must point at earlier features of the right type (face → any body-making feature id or pattern copy id; plane → an earlier `Plane`; sketch point → an earlier sketch and an existing point in it); `distance` expression length and finiteness; `at`/`frame` finiteness with valid bounds (reuse the text check); `Sketch.on` must name an earlier `Plane`. A dangling `on` is a loadable timeline error, not a load failure, consistent with other feature dependencies.

## Tests

Core (`crates/fr-core/tests/planes.rs`):

- Offset from each origin plane, positive and negative, keeps axes; offset from a face of a box lands at the expected height; offset of an offset composes.
- Sketch on an offset plane whose distance is `$h`; change `$h` with `set_parameter`; the extrude's bounding box moves by the difference.
- Offset from a face survives the base body growing (edit the base extrude's distance): the plane follows the face, and the dependent extrude still builds. Deleting the base body gives the plane an error and the dependent extrude fails with the sketch-placement message, not stale geometry.
- Midplane of opposite faces of a plate lies at half thickness with the first face's x; midplane of the two faces of a 90° wedge is at 45°; `flip` gives the other bisector; same face twice is refused.
- Three points: a plane through three vertices of a box matches the expected diagonal plane; collinear points refused; a sketch point reference follows when the sketch point is moved with `point_coordinates`.
- Rollback before the plane: the sketch on it and its extrude show errors; roll to end restores them.
- Serialization round trip with every kind; a file with planes is version 5; a plane whose face reference points forward in the timeline is rejected on load; a sketch with a dangling `on` loads with a timeline error.
- API: `create_plane` for each kind returns axes; `create_sketch` with a plane id uses them; bad input leaves the document unchanged (extend `api_rejects_bad_input_without_changing_the_document`).

App (`uitest.rs`):

- Offset plane from a face: open the dialog, click the top face, drag the arrow, check the distance text snaps and the preview moves; OK; New Sketch, click the plane in the viewport, draw a circle, extrude; the body's bottom sits at the plane height.
- To face in the Offset dialog sets the distance to the gap between two faces.
- Midplane by clicking two faces of a plate; sketch on it; a symmetric extrude is centred in the plate.
- Three points by clicking three vertices; the plane is drawn and a sketch can start on it.
- Eye in the `Construction` folder hides and shows the plane; finishing a sketch on a plane hides the plane unless its eye was toggled by hand.

## Manual acceptance (to perform, not claims)

1. Make a 40 × 20 × 10 mm box. Construction Plane → Offset, click the top face, type `5 mm`, OK. The plane floats 5 mm above the box and is listed under Construction. New Sketch, click the plane, draw a 6 mm circle, Extrude 3 mm Join. The post stands on the plane and reaches down to nothing: it is a separate body unless it touches. Change the extrude to −5 mm: it joins to the box.
2. Edit the box's extrude to 20 mm tall. The plane and the post move up with the top face.
3. Offset with a negative distance from the same face: the plane is inside the box, and the dialog's arrow points into it.
4. Midplane between the top and bottom faces. Sketch a slot on it and cut Symmetric 4 mm: the slot is centred in the plate.
5. Midplane between the top face and one side face: a 45° plane through their shared edge. Flip picks the other 45° plane.
6. Three points on three corners of the box: a slanted plane; sketch and cut through all to lop the corner off.
7. Delete the box. Every plane that used it goes red; the sketches and extrudes on them go red with a message that names the plane. Undo brings everything back.
8. Save, reopen: planes, visibility and the sketches' attachments are preserved. Open the file in 0.2.2: it refuses with the "newer version" message.
9. Over MCP: `create_plane` of each kind, then `create_sketch` on the result and an extrude, with a screenshot to confirm.

## Work breakdown

1. **Core model and resolution.** `PlaneRef`, `PointRef`, `PlaneKind`, `ConstructionPlane`, `planes.rs` with the three resolvers and rectangle sizing, `Sketch.on`, rebuild reordering, sketch-error propagation into `tool()`, validation, version 5. Core tests.
2. **API.** `create_plane`, `edit_feature`, `create_sketch` by plane id, object info, visibility, reference text. API tests.
3. **Viewport and browser.** Draw planes, pick them in `PickPlane`, `Construction` folder with eyes and auto-hide, chip icon, feature menu.
4. **Dialog.** `PlaneDlg` with the three kinds; shared distance arrow; face, vertex and sketch-point picking; To face; live preview. UI tests.
5. **Docs.** README (Reference → a `Construction planes` entry under Model, and the `Faces` paragraph), tutorial touch-up if a step benefits, `docs/releases/0.3.0.md`, this plan updated to match what shipped.

### Order relative to components

Both plans change `Document::rebuild` and the New Sketch picker. Do planes first: step 1 here is the smaller rework of the rebuild loop, and the components work then adds owners and scoping to a loop that already walks planes and sketches in order. Nothing in the plane model depends on components; `PlaneRef::Origin` simply means "the owner's origin" once owners exist.

## Follow-ups this enables (not 0.3.0)

- Extrude **To face** could accept a construction plane as the target.
- Pattern **Mirror** across a construction plane, which is the open TODO item "Mirror a body across an arbitrary plane".
- Construction axes and points (an axis through two points, the axis of a cylinder) for revolve and circular patterns about arbitrary axes, also an open TODO.
- Angled plane (a plane through an edge at an angle to a face), tangent plane on a cylinder, and a plane at a point on a path.
- Expressions for three-point coordinates and for a midplane ratio other than one half.
