# Sketching fixes in Ferrender 0.2.2

For 0.3.0, Select uses **V**; **S** opens command search. Other steps below retain their documented 0.2.2 behavior.

Version 0.2.2 addresses feedback from the 0.2 manual tests: choose an arc's endpoints before its bulge, set dimensions on existing geometry, keep deliberate angle locks, and move or scale an imported reference directly in the sketch.

Check **Help → About Ferrender → Copy build info** before testing. It should identify **0.2.2**; include its source commit with any report. The [full sketching guide](SKETCHING_0.2.md) still covers point coordinates, tangent arcs, splines, reference calibration, open-end highlighting, and expressions. The [bishop tutorial](../BISHOP_TUTORIAL.md) uses the revised arc click order.

## Three-point arcs: endpoints first

Choose **Sketch → 3-Point Arc**. Click the **start**, then the **end**, then move the pointer to choose the **bulge**. The first two points stay at the ends of the preview. A third point above the chord produces an upper arc; a point below produces a lower arc. The third point lies on the arc, not at its center. Its position controls the curvature and hence the circle's diameter.

For a semicircle from `(0, 0)` to `(10, 0)`, use `(5, 5)` as the third point. Use `(5, -5)` for the lower semicircle. A bulge beyond the semicircle can produce a sweep greater than 180°. Three collinear points cannot form an arc. After choosing the endpoints, the **Diameter** field holds an exact circle diameter. It cannot be smaller than the endpoint spacing. Move the pointer to choose the desired side and arc segment, then place it.

In 0.2.0 and 0.2.1 the UI used start → through → end. Existing documents keep their geometry, and the command API's named `start`, `through`, and `end` fields keep their meanings.

## Dimension an existing item

Select a line and press **D** or choose **Dimension** to set its length. You can also choose Dimension first, click the line, type a length, and press Enter. The editor opens immediately; a blank-space click is no longer required. The value becomes an editable dimension attached to the geometry, like the dimension shown in the manual test screenshot.

Circles default to diameter; arcs default to radius. An existing radius or diameter on a curve is reopened instead of adding another size constraint. Double-click any dimension label to edit it later.

To dimension **between** two items, use Select and Shift-click them both before pressing D. Two points give distance, a point plus a line gives perpendicular distance, parallel lines give spacing, and other pairs of lines give angle. Clicking a point and then another point or line in the Dimension tool still works. A first line click now opens length immediately, so two-line dimensions use preselection.

A dimension can only move geometry as its other constraints allow. A line with fully positioned endpoints may reject a conflicting new length. Correct those positions or remove an unwanted constraint rather than adding conflicting dimensions.

## Freeze a direction or an arc sweep

Ordinary snapping helps near horizontal, vertical, 45°, parallel, perpendicular, and tangent directions for lines, and near 45° sweep increments for tangent arcs. A snap stays engaged through small pointer movements; move farther away to leave it.

**Hold Shift** to freeze a line's current direction or a tangent arc's current sweep. Move the pointer to change length or size. Keep Shift held through the placement click to keep the angle fixed during later edits. Release it before placement to release the explicit lock. This does not change Shift's role in adding items to a Select selection.

Type **Angle** for an exact line direction from sketch +X, including `0 deg` or negative values. Type **Sweep** for a tangent arc's turn, strictly between 0° and 360°. These values, and an angle locked with Shift at placement, become dimensions. Double-click their labels to edit them; select and delete the dimension to free that angle again. Tangency remains a separate constraint that keeps the junction smooth.

Saved line-direction and arc-sweep dimensions require **0.2.2 or later** (native file format 4). Existing older files still open. Keep separate copies if you also use an older build.

## Move and scale a reference with Select

While editing the image's sketch, choose **Select** (`S`). Drag an empty part of the image to move it. Clicking the image selects it and shows four square corner handles. Drag a corner to scale uniformly around the opposite corner; the image keeps its proportions and rotation.

Sketch points, edges, and dimension labels take priority over the image body, so select or start a move away from those items. Once the image is selected, its corner handles take priority. A hidden image cannot be picked. The **Reference Image…** dialog still provides exact width, origin, rotation, opacity, visibility, and two-point calibration.

Dragging previews the change. Releasing commits one undoable image edit. Escape cancels the current drag; a second Escape clears the image selection. Moving or scaling the image does not move the sketch geometry. The placement is saved in the `.ferr` file along with the embedded image.

## Focused manual retest

Allow about 30–40 minutes. Use fresh XY sketches for the independent geometry checks, save test designs, and report results by number. These are tests to perform, not a claim that a person has already completed them.

1. **Arc bulge direction.** Choose 3-Point Arc. Click a left endpoint and a right endpoint, then move above and below the line joining them before clicking. Expect the preview and committed arc to follow that side, with the first two endpoints unchanged. Repeat using exactly `(0, 0)`, `(10, 0)`, then `(5, 5)` for the upper semicircle. Undo/Redo should remove/restore one complete arc.
2. **Arc diameter and closure.** Start a separate arc with endpoints 10 mm apart. Type a diameter of `14 mm` and choose its side. Expect a 7 mm circle radius and the selected side to match the preview. Try a diameter of `8 mm`: expect a visible refusal, with the previous sketch intact. Close a valid arc with a line between its endpoints; Finish Sketch and extrude 3 mm. Expect a closed solid. Escape during an unfinished arc must leave no partial geometry.
3. **Dimension an existing line.** Draw a free slanted line and end the line chain. Select the line, press D, type `10 mm`, and press Enter. Expect the existing line to become 10 mm long with a dimension label. Double-click the label and change it to `15 mm`; Undo/Redo should restore/reapply the edit. Repeat by choosing Dimension first and then clicking another line: the size box must open immediately. Do not use coordinate-fixed endpoints for this check.
4. **Curve and between-item dimensions.** Select a free circle and press D; set its diameter to `12 mm`. Repeat D on the circle to edit that same dimension. Check an arc's radius too. In another sketch, use Select and Shift-click two free nonparallel lines, then D: expect an angle value, not a line length. Repeat with two parallel lines for spacing and two points for distance. Confirming a valid value should change the intended geometry.
5. **Sticky line direction and lasting Shift lock.** Start a line and move near horizontal, vertical, 45°, and parallel to an existing line. Expect a stable snap with visible feedback. Move to a slanted angle, hold Shift, move farther and sideways, and place while still holding Shift. Expect the direction to stay fixed while length changes. End the chain, use Select to drag the free endpoint, and confirm its angle remains constrained. Edit the angle label, including `0 deg` and a negative angle. Save and reopen; the lock must remain. Delete only the angle dimension and confirm the endpoint can rotate again.
6. **Tangent arc sweep lock.** Draw a horizontal line and end the line chain. Start Tangent Arc at its right endpoint, make a bend, hold Shift, move the pointer, and place while holding Shift. Expect the sweep to stay fixed as size changes and the junction to remain smooth. Repeat with typed `90 deg` in Sweep, once above and once below the source line. Edit the saved sweep dimension, Undo/Redo, and reopen the file. The sweep and tangency must remain. In a separate attempt, release Shift before placement and verify no explicit Shift angle lock is left behind.
7. **Move a reference image.** Import a small PNG or JPEG and Apply. Draw a line over part of it. Choose Select and drag an empty part of the image; expect only the image to move. Undo once should restore its entire previous position; Redo should restore the new one. Drag the line instead and confirm sketch geometry takes priority. Start another image drag and press Escape: expect no committed movement or extra undo step.
8. **Scale a reference image.** Select the image and drag a square corner handle. Expect uniform scaling, no stretching or mirroring, with the opposite corner staying fixed in sketch coordinates. Undo/Redo should restore/reapply the whole gesture. Set a rotation in Reference Image, Apply, and repeat; its rotation and proportions must stay correct. Drag a handle across its opposite corner: the image must not be committed at an invalid or flipped size. Use the exact Width field and calibration controls to confirm those methods still work.
9. **Image persistence and visibility.** Save the moved/scaled image in a `.ferr` file, rename or move the original image file, and reopen the design. Expect the image and placement to survive. Hide it using Visible: it should stop intercepting Select. Show it again; the placement should be unchanged. Finish Sketch and export a solid: the reference should not appear in STL or STEP.
10. **Short regression checks.** In a fresh sketch draw three sides of a rectangle: expect two open-end rings, then zero when you close the fourth side. In a value box enter `1e1 mm`, then try `1 / 0`: expect 10 mm for the first and a visible error without geometry changes for the second. Check View → Appearance in Light and Dark: handles, angle feedback, labels, and geometry should remain readable. Reopen one existing bishop, rook, or pawn and confirm its shape and dimensions remain unchanged.

## Follow-up: macOS Open With

The source update after the original 0.2.2 release adds Finder document-open handling. Check the commit in About: the original `c46e848` build does not include this fix. These checks apply to the updated build.

1. Save a test design containing a reference image. Quit Ferrender, then use Finder **Open With → Ferrender** on that `.ferr` file. Expect the saved design and image to load without an error. If several Ferrender copies are listed, choose **Other…** and select the updated Desktop app.
2. Leave Ferrender running and use Finder to open a different saved design, then the image design again. Each should replace the current saved document. Try a filename with spaces and `%` too.
3. Make an unsaved change. Open another design from Finder and choose **Cancel** when asked to discard changes. The current work must remain. Repeat and choose **Discard** only on this throwaway test: the requested file should open.
4. Open a deliberately invalid `.ferr` file from Finder. Expect a persistent error; the current design must remain intact. Dismiss it and use **File → Open** on a valid design to confirm the normal path still works.

Finder requests to open several documents together are refused with a message to open one at a time. STL files use the existing import dialog.

## Reporting

Send results such as “1–3 pass; 4 fails after reopening.” Include copied build info, exact inputs, a screenshot for visual problems, and the smallest saved `.ferr` file that reproduces the issue. The already-passing open-end and invalid-input tests only need the short regression checks here; the rest of the original 0.2 test plan remains available when you are ready to continue.
