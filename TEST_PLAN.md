# Ferrender manual test plan

The automated tests cover the
engine and that the screens draw; they do not cover real mouse, trackpad or keyboard input,
the live Claude API, Linux, or opening our files in other programs. This plan is ordered so
the things no test has ever exercised come first.

Run `cargo run --release -p ferrender`. Note the step number and what you saw for anything
that fails; a screenshot plus the saved `.ferr` is enough to reproduce most things.

## 1. Never checked by a person (highest value)

- [ ] **Trackpad navigation**: two-finger scroll zooms, drag orbits, Shift-drag pans, Space-drag pans. Zoom should go toward the cursor, not drift.
- [ ] **Mouse navigation**: left-drag orbit, right/middle-drag, wheel zoom. Orbit should not flip upside down past the poles.
- [ ] **Enter confirms a dialog** (Extrude, Move, Fillet, Shell); **Escape cancels** and leaves no body and no undo step behind.
- [ ] **Typing in a value box does not trigger shortcuts** (type `l`, `c`, `d` in a dimension box; no tool should start).
- [ ] **Depth clipping fix**: shell a small box, pan it to a screen corner, orbit. No holes should appear.
- [ ] **STEP export opens elsewhere**: export a filleted, shelled part, open in Fusion or FreeCAD. Check size (measure one edge) and that it is one solid body.
- [ ] **STL in a slicer**: export the same part in mm, cm and in. In the slicer the mm file should be the right size; the in file should be 25.4x smaller when read as mm. Slicer reports no errors or repairs.
- [ ] **Assistant with a real API key**: put a key in `~/.config/ferrender/config.toml`, ask for "a 20 mm cube with a 5 mm hole through the top". This path has never been run against the live API.
- [ ] **MCP from Claude Code**: `claude mcp add ferrender -- <path>/target/release/ferrender mcp`, with the app open, ask for the small screw. Watch the app update live.
- [ ] **Linux build**: `cargo build --release` on a Linux machine (needs a C++ compiler; first build downloads OpenCascade). Viewport renders, file dialogs open.

## 2. Sketching

- [ ] Start a sketch on XY, XZ, YZ, and on an offset plane. Grid and axes look right on each.
- [ ] Line, rectangle, circle, arc, polygon: each draws, snaps to existing points, and chains correctly (line tool continues from the last point; Escape ends it).
- [ ] Trim a line crossing a circle; mirror about a line; offset a closed rectangle inward and outward; round a corner. (Known gap: offset of arcs.)
- [ ] Project an edge of an existing body into a sketch; it should appear fixed and not drag.
- [ ] Select with click, Shift-click, box-drag. Delete removes entities and their constraints.
- [ ] **Copy/paste**: copy one point, one line, a rectangle with its dimensions. Paste lands near the cursor, keeps internal constraints, and does not stay tied to the original.
- [ ] Drag an unconstrained point: neighbours follow. Drag a fully constrained sketch: nothing moves and no undo step is added.

## 3. Constraints and dimensions

- [ ] Each constraint on a fresh pair: coincident, horizontal, vertical, parallel, perpendicular, tangent (line-circle, line-arc, arc-arc), equal (two lines, two circles), concentric, midpoint, symmetric, fix.
- [ ] Conflicts: make two lines parallel then perpendicular. Expect a clear refusal, the sketch unchanged, no collapsed line.
- [ ] Over-constrain with a redundant dimension. Expect a message, not a silently distorted sketch.
- [ ] Degrees-of-freedom readout goes to 0 on a fully dimensioned rectangle tied to the origin.
- [ ] Dimensions: length, distance point-line, radius, diameter, angle. Edit one by double-clicking; the sketch resizes without flipping.
- [ ] **Units**: enter `10`, `10mm`, `1cm`, `0.5in`, `1in + 2mm`, `25.4mm / 2`. Each box shows what was typed and resolves correctly.
- [ ] **Variables**: type `d = 10mm` in a box, then `$d`, `$d * 2`, `$d / 2 + 1mm` elsewhere. Change `d` in the Parameters panel; every dependent dimension and feature follows.
- [ ] Bad input: `$nope`, `10 mm mm`, `5 +`, an angle where a length is wanted. Expect an error next to the box and the old value kept.
- [ ] Rename or delete a variable that is in use. Expect a refusal or a clear error, not a broken model.

## 4. Solids

- [ ] Extrude a profile: distance, symmetric, through-all, tapered. Negative distance goes the other way.
- [ ] Extrude a profile with a hole (circle inside rectangle); pick inner and outer regions separately.
- [ ] Operations: new body, join, cut, intersect. Cut where the tool misses the body gives a clear message.
- [ ] **Extrude arrow**: drag it out and back through zero; the value box follows; typing a value moves the arrow.
- [ ] **Sketch on a face** of a body, then extrude outward (join) and inward (cut).
- [ ] Revolve a profile 360 and 90 degrees about a sketch line; about an axis touching the profile; about an axis crossing the profile (should refuse).
- [ ] Edit an early sketch dimension; everything after it rebuilds. Edit an extrude's distance from the timeline.
- [ ] Break a later feature by editing an earlier one (shrink a body so a cut misses). The failing feature is marked and the rest still shows.

## 5. Selection, move, combine, patterns

- [ ] Click a face: only that face highlights. Click again or double-click for the body, per the hint shown. Hover highlight matches what a click will pick.
- [ ] Pick edges for fillet on straight edges, circular edges, and edges on the far side after orbiting.
- [ ] Move: by typed distance on each axis, by drag, with a face selected (moves its body). Rotate if offered.
- [ ] Combine: join two overlapping bodies, cut one from another, intersect. Join two bodies that only touch on a face.
- [ ] Circular pattern of a hole (6 copies), linear pattern (3 x 2), mirror. Dots preview where copies land.
- [ ] Pattern where some copies miss the body: those are skipped; if all miss you get an error.

## 6. Fillet, chamfer, shell (exact kernel)

- [ ] Fillet one edge, a chain of connected edges, all edges of a box. Radius too large gives a refusal and leaves the body intact.
- [ ] Chamfer the same sets.
- [ ] Shell with one open face, two open faces, no open face (hollow, check with section view). Thickness larger than half the part should refuse.
- [ ] Fillet, then resize the base sketch: the fillet stays on the same edge.
- [ ] Fillet after a join of several bodies. (Known: filleting every edge of a large join at once fails.)
- [ ] An imported STL body: fillet and shell should refuse with a message saying it is a mesh.

## 7. Timeline and undo

- [ ] Drag the marker back one feature at a time; the model shows each earlier state. Drag forward; it returns.
- [ ] With the marker rolled back, add a feature: it is inserted there and later features rebuild on top.
- [ ] Undo/redo across 20 mixed steps, including across a roll-back. Ends at exactly the starting state.
- [ ] Delete a sketch that an extrude uses: refused or cascades, clearly said either way.

## 8. Viewing

- [ ] View presets (top, front, right, iso) and fit-to-view on an empty scene, one small part, and a part far from the origin.
- [ ] Section analysis on each axis, slide through a shelled part: cap is hatched, wall thickness looks uniform. (Known: picking and screenshots ignore the section.)
- [ ] Edge lines are clean on fillets (no speckle) and present on mesh imports.
- [ ] Resize the window very small and very large; move it between a Retina and non-Retina display.

## 9. Files

- [ ] Save, quit, reopen: same bodies, same parameters, same timeline, marker position sensible.
- [ ] Save As to a new name; title bar and later Save go to the new file.
- [ ] Open a hand-damaged `.ferr` (delete a brace): a readable error, no crash.
- [ ] Import an STL (binary and ASCII), in mm and in inches. Move it, cut it with a sketch-made body, export again.
- [ ] Import a large STL (1M triangles): time it, check the viewport stays usable.
- [ ] Quit with unsaved changes: you are asked.
- [ ] Crash recovery: make changes without saving, then `kill -9` the app (or pull the plug). Start it again: "Recover unsaved work" lists the design; Recover brings it back unsaved, with its old file name if it had one.
- [ ] Same, but choose Later: nothing is lost, and File › Recover Unsaved… shows it again. Delete removes it for good.
- [ ] Quit normally with unsaved changes and choose Discard: the next start offers nothing.
- [ ] Two windows open at once: neither offers the other's work while both run.

## 10. AI and MCP

- [ ] `get_scene_info` and `get_object_info` match what is on screen (body count, sizes in mm).
- [ ] `get_viewport_screenshot` matches the app's current view.
- [ ] A bad command in a batch: execution stops at the first error, names the command and leaves earlier commands applied. A request exceeding command/depth limits is rejected before anything is applied.
- [ ] Automation refuses new/open when the current design is unsaved, including after an earlier batch command changed it. Explicit `discard_unsaved: true` permits replacement.
- [ ] Restart or close the GUI after an MCP client connects: the client reports the lost/replaced backend instead of silently switching to an empty document.
- [ ] Headless: `ferrender mcp --headless` builds the screw with no window and writes an STL.
- [ ] Ask for a part using a variable ("make the wall thickness a parameter t"); then change `t` in the app.
- [ ] Two clients at once (app Assistant plus Claude Code): no corrupted state.

## 10b. Typed sizes and Measure

- [ ] **Rectangle**: click a corner, type `30`, Tab, type `12`, Enter. A 30 x 12 rectangle with both dimensions on it. Moving the pointer to each quadrant before Enter picks which way it goes.
- [ ] Type only the width, then click: the width holds and the click sets the height.
- [ ] **Circle**: click the centre, type `8`, Enter: an 8 mm circle with a diameter dimension.
- [ ] **Line**: click, type `25`, Enter: 25 long toward the pointer; the line carries on; Enter again ends it.
- [ ] `w = 20` in a box defines `w`; `$w / 2` in the next uses it; `0.5 in` works in a mm document.
- [ ] Typing words in a box shows a warning mark and Enter does not place the shape. Escape drops it.
- [ ] Clear a box you typed in: it goes back to following the pointer.
- [ ] Tab from the last box returns to the first. Tab never jumps to some other control. (Driven by simulated keys in the tests; not tried on a real keyboard.)
- [ ] The boxes do not get in the way of clicking where you want the second corner on a small rectangle.
- [ ] **Measure** (I): two parallel faces of a box give "Apart" equal to its size. Two faces that meet give 90 degrees.
- [ ] Two parallel edges; an edge and a face; two corners (check the X, Y, Z parts).
- [ ] Click inside a hole: it reads the diameter. Hole wall to an outside face gives the wall thickness.
- [ ] A sketch point to a body corner, with the sketch visible.
- [ ] Faces of two different bodies, including an imported STL.
- [ ] A third click starts a new measurement; Clear and Escape work.
- [ ] Known: edges and corners hidden behind the body can still be picked; a large curved face against another is slow on first click.

## 11. Holes and threads

- [ ] **M3 clearance hole**: Hole, click the top of a plate, leave it on "Clearance, normal" and M3. Measure it in the slicer or with section view: 3.4 mm.
- [ ] Close and loose fits give 3.2 and 3.6 mm. Change the thread to M5 and 1/4-20 and check against a drill chart.
- [ ] **Countersink**: M3 gives 6.3 mm at 90 degrees; #6-32 gives 82 degrees. A real flat head screw should sit flush in a print.
- [ ] **Counterbore**: M3 gives 6.5 mm by 3.4 mm deep; a socket cap screw head should drop in below the surface.
- [ ] "Custom size" on a counterbore and a countersink; a head narrower than the hole is refused with a message.
- [ ] Blind hole with and without "Drill point"; check the bottom in section view.
- [ ] Several holes in one go: click four spots, click one again to remove it, OK. One feature in the timeline.
- [ ] **Sketch-point placement**: draw a sketch with points where the holes go, leave it visible, start Hole and click near each point. Holes land exactly on them.
- [ ] Clicking a curved face says holes start on flat faces.
- [ ] **Tapped, not modeled**: M3 gives a 2.5 mm hole. Print it and drive an M3 screw in.
- [ ] **Tapped, modeled**: M6 and M8 in a 10 mm plate. Print and try a real bolt. Note which sizes print usably on your printer and what Allowance they need (start at 0.2 mm).
- [ ] Left-hand modeled thread looks mirrored next to a right-hand one.
- [ ] **Thread on a rod**: extrude a 6 mm circle 20 mm, Thread, click its side: it offers M6. Whole length; then a second rod threaded 10 mm from the tip.
- [ ] **Thread in a hole**: a 5 mm hole offers M6. Asking for M3 remakes the hole to the selected size.
- [ ] Print the M3 and M6 rods after the chamfer/thread correction and try the existing 0.2 mm-allowance plate. The earlier print accepted a metal M3 screw, but both printed screws failed; the new tip geometry still needs a physical test.
- [ ] Chamfer a screw tip before threading it: there is no full-diameter collar past the last thread. A free thread starts with a gradual lead; a partial thread leaves unrelated ends and the head alone.
- [ ] Thread dialog: the Allowance box starts at 0.2 mm; clearing it gives the exact size; a value larger than the pitch is refused. Hole dialog: ticking "Model it" fills Allowance in.
- [ ] A metal M3 screw in a printed, modeled M3 hole with 0.2 mm allowance. Compare with an unmodeled tapped M3 hole (2.5 mm).
- [ ] Fillet the plate's edges after adding a modeled thread; move the body; undo. The thread stays with it.
- [ ] The slicer shows the thread and reports no errors (it may mention multiple shells; that is expected).
- [ ] Save, reopen: holes and threads come back.
- [ ] Many threads: ten modeled M3 holes in one plate. Editing an earlier dimension should stay responsive.
- [ ] Through Claude: "add a countersunk M3 clearance hole at each corner, 5 mm in from the edges" and "make an M6 x 20 bolt with a 10 mm head".

## 12. Whole-part runs

Do these start to finish without notes; write down every point where you had to guess.

- [ ] **Bracket**: L-profile sketch, extrude 20 mm, two holes patterned, fillet the inside corner, export STL, slice.
- [ ] **Knob**: revolve a profile, circular-pattern six grip cuts, chamfer the top.
- [ ] **Enclosure**: box, fillet verticals, shell open top, add four bosses on the inside face, cut holes.
- [ ] **Parametric box**: `w`, `d`, `h`, `t` as variables; change each and confirm nothing breaks.
