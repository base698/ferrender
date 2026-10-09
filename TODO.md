# TODO

Checked items were addressed on the `cadrum-kernel` branch (2026-10-06); notes on
how are at the end. Unchecked items are still open.

What an AI agent driving Ferrender over MCP needed and didn't have. Collected while
building a screw, a panel with four screws, and a blocky crocodile figure (24 bodies).
Ordered roughly by how much each one cost.

## MCP responses

- [x] **Return only what changed.** Every `extrude` / `revolve` / `pattern` returns the
      full list of bodies, so output grows with the square of the part count. One batch
      of 10 extrudes on a 24-body document returned 99,749 characters and was rejected
      by the client for size. Return the new or modified bodies only, or add a
      `"quiet": true` option; `get_scene_info` already covers the full listing.
- [x] **Keep the command reference inside the client's limit.** The tool description
      reached the agent cut off mid-way through the sketch items, so the extrude,
      revolve, pattern and transform syntax had to be read out of
      `crates/fr-core/src/api.rs`. Split it across tools or serve it from a
      `get_reference` call.
- [x] **Let a batch refer to ids it just created.** Sketch and feature ids have to be
      guessed ahead of time (they happen to be sequential) or fetched with a round trip.
      Something like `"sketch": "$last"` or user-assigned names would remove the guessing.
- [x] **Return entity ids in a form that says which is which.** Revolving about a line
      needs that line's id, which means a separate call before the revolve. Allow
      `"axis": {"from": [x, y], "to": [x, y]}` as well.
- [x] **Return the plane's axes from `create_sketch`.** On an arbitrary
      `{"origin", "normal"}` plane the sketch x and y directions are unknown until
      `get_object_info` is called, so only circles and regular polygons are safe to draw.
      Also accept an explicit `"x"` direction.

## Booleans

- [x] **Joining small bodies explodes the triangle count.** Joining 14 bodies with
      about 5,000 triangles between them failed with "the meshes have 69534 triangles;
      booleans are limited to 60000".
- [x] **Joins leave open edges.** Joining 8 of those bodies succeeded but produced
      30,178 triangles and 106 open edges, so the result was undone. The figure is still
      24 overlapping bodies and cannot be exported as one clean shell.
- [x] **Export a union.** An `export_stl` option that merges overlapping bodies would
      cover the common case even if interactive booleans stay limited.

## Editing existing geometry

- [x] **Move sketch geometry.** There is no way to translate entities in a sketch.
      Moving the screw off the origin meant deleting its circles and redrawing them.
- [x] **Extrudes should survive that.** After the redraw every dependent extrude failed
      with "a profile it used is no longer closed", and `edit_feature` cannot repoint
      profiles, so all three extrudes were deleted and recreated.
- [x] **Parametric point positions.** 0.2 adds signed X/Y position dimensions and direct
      point-coordinate entry. Use those dimensions when a position should follow a
      parameter; one-time coordinates in `add_geometry` are still just initial positions.
- [ ] **Point-to-point horizontal/vertical distance dimensions.** Extend the signed
      point-to-origin dimensions to two independently moving points.

## Copying and placing bodies

- [ ] **Copy or pattern a whole body.** `pattern` repeats one feature, so a part made of
      three features needs three patterns that have to agree.
- [ ] **Pattern and rotate about any axis or point.** Patterns and `transform` rotations
      only work about world axes through the origin.
- [ ] **Mirror a body** across an arbitrary plane.
- [ ] **Non-uniform scale**, which would turn a sphere into an ellipsoid.

## Missing shapes and features

- [x] **Threads.** `thread` on a rod or hole, and `hole` with `"modeled": true`. A general
      helix (springs, coils) is still not there.
- [x] **Sweep** a profile along a path in another sketch: tangent runs exactly, sharp corners
      mitred, closed paths, Follow or Fixed orientation. Still missing: paths that leave one
      plane (helix, 3D splines), twist and scale along the path, a guide rail, and rounded
      instead of mitred corners.
- [x] **Loft** between profiles, for limbs, snouts and tails that change section. Sections
      need equal edge counts and no holes; a section that is a single point, guide rails and
      a closed loop of sections are not built.
- [x] **Spline sketch items.** 0.2 adds editable curves through four fit points.
- [ ] **Sphere and ellipse** sketch items or primitives. Joints, eyes and beads
      were faked with flat-ended cylinders.
- [ ] **More spline controls.** Arbitrary fit-point counts, periodic splines, and tangent
      constraints for spline endpoints.
- [x] **Edge fillet and chamfer on solids.** Sketch corners can be rounded; body edges
      cannot.
- [ ] **Shell** and **draft** would also help; taper only applies to a whole extrude.

## Documentation gaps found by trial

- [x] `"offset"` on the `XZ` plane moves toward -Y (offset `-15` gave `y = +15`).
- [x] `"symmetric": true` treats `distance` as the total thickness, not per side.
- [x] Nested regions become holes only when drawn in the same sketch; say so next to
      the `extrude` entry.

## Screenshots

- [x] **Zoom to fit.** The `iso` view left an 80 mm model filling about a quarter of
      the frame.
- [x] **Custom camera** (position and target, or azimuth and elevation) to inspect one
      area such as the jaw or a hand.

## From the working session (discussed, not built)

Collected from the conversation that built Ferrender, 2026-10-06. These were raised,
proposed or noted as limits along the way and are still open.

### Set aside after the Fusion tutorial review ("leave for later")

- [x] **Loft** between profiles (the moka pot's spout).
- [x] **Threads** and a **Hole** feature with a thread catalog (clearance, tapped,
      counterbore, countersink). Still open on these:
  - [ ] Check the catalog's tap drill and clearance numbers against ISO 273 and the inch
        drill charts; they were entered from memory.
  - [ ] Thread classes (6g/6H). There is a plain allowance, 0.2 mm by default; tune it from real prints.
  - [ ] Lead-in chamfer on thread ends; taper threads (NPT); custom catalog entries.
  - [ ] Holes placed from a sketch by reference, so they follow it; holes on curved faces
        from the dialog (the API takes a "direction").
  - [ ] Pattern a hole or thread; edit one after the fact.
  - [ ] Threads follow later cuts (they are overlapping shells; see README limits).
- [ ] **T-splines / freeform surfaces** (the handle).
- [ ] **Components and joints**: assemblies, grounding, joint limits.
- [x] **Reference image (canvas)** with two-point scale calibration, placement, opacity,
      and a PNG/JPEG embedded in the native document (0.2).
- [ ] **Appearances**: per-body colour or material.

### Asked for or offered, not done

- [ ] **Pattern along a picked edge or axis.** Patterns only run along the model's X, Y, Z;
      the screenshot that failed needed a direction along the part.
- [ ] **Edit Pattern, Move, Combine, Fillet, Chamfer and Shell after the fact.** Only
      sketches, extrudes and revolves reopen; the rest are delete-and-redo.
- [ ] **Reorder features** by dragging in the timeline.
- [ ] **Mesh editing tools** for imported STL beyond move, scale, combine and cut. Asked
      for early ("handle mesh"); nothing specific was chosen. Candidates: repair, simplify,
      plane cut, convert a clean mesh to an exact body.
- [ ] **Blender-style interface**: "copy Blender's interfaces" was read as the Blender
      MCP's tool set. If Blender's own keys or layout were meant, that is not done.
- [ ] **Split crossing shapes into regions.** Two sketch shapes that merely cross are not
      split where they cross, so the overlap cannot be picked as a profile.
- [ ] **Offset of arcs** in sketches; variable-radius fillets; asymmetric chamfers.
- [ ] **Sketches and extruded faces that follow their face.** They record where the face
      was. Fillets and shells find edges by position, with the same weakness.
- [ ] **Section view**: picking ignores it, and it is not available in MCP screenshots.
- [ ] **Perspective camera** and a view cube; the camera is orthographic only.

### Never verified

- [ ] **The built-in Assistant against the live Claude API** (no key in the build
      environment).
- [ ] **Linux build and run.**
- [ ] **Trackpad gestures, paste between two windows, Enter confirming a dialog** by hand.
- [ ] **STEP files opened in another CAD program.**
- [ ] **Merge `cadrum-kernel` into `main`** once it has been tried.

## How the checked items were addressed

- **Threads:** the kernel's helix sweep was tried three ways and dropped: about 2 s per
  thread, erratic failures, and empty solids returned without an error on long threads.
  Cutting a generated thread mesh into the body with the BSP booleans left open edges.
  What shipped keeps the body exact (a plain hole, or a rod turned down to the thread's
  roots) and adds the thread as a generated closed shell that overlaps it. Every catalog
  size is built as a screw and as a tapped hole in the tests.

- **Responses:** body-changing commands return `changed_bodies`, `removed_bodies` and
  `body_count`. A 42-command batch building 14 bodies returned 14,088 characters.
- **Reference:** `get_reference` tool and op; the `execute` tool description is 659
  characters and lists the op names.
- **Ids in a batch:** `"$last_sketch"`, `"$last_feature"`, `"$last_body"`. User-assigned
  names are not done.
- **Revolve axis:** `"axis": {"from": [x, y], "to": [x, y]}`.
- **Sketch plane:** `create_sketch` returns `origin`, `x`, `y`, `normal`, and accepts `"x"`.
- **Booleans:** sketch-made bodies are exact OpenCascade solids. Joining the 14 bodies
  above gave one body, 2,552 triangles, 0 open edges, in under a second. Filleting
  every edge of that joined body at once still fails; pick edges instead.
- **Union export:** `export_stl` with `"union": true` (exact bodies only).
- **Moving geometry:** `{"op": "move", "sketch", "ids", "by"}` keeps entity ids, so
  extrudes built on them survive.
- **Parametric positions:** the centred `rect` item predates 0.2; 0.2 adds signed X/Y
  point-to-origin position dimensions and the Point Coordinates dialog. Point-to-point
  horizontal/vertical dimensions remain open.
- **Fillet, chamfer, shell:** `fillet_edges`, `chamfer_edges`, `shell`; edges and faces
  are listed under `topology` in `get_object_info`. Draft is still open.
- **Docs:** the three gaps are in the reference text.
- **Screenshots:** views fit the bodies; `azimuth`, `elevation`, `target`, `zoom`.
