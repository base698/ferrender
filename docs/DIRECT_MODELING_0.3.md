# Direct modeling and placement in Ferrender 0.3

These controls belong to the **unreleased 0.3.0-dev test candidate**. Use the exact commit shown in About when reporting results. They are awaiting human acceptance; the numbered checks continue at 26 in the [manual test plan](TEST_PLAN_0.3.0.md).

## Remove, split, and join bodies

**Remove Body** records the selected bodies' removal at the current point in the timeline. Select bodies in the browser or viewport; click again to remove one from the selection. Review the preview and press OK. Earlier features remain in history, and copies made before the Remove step remain intact unless selected too. For example, mirror a cone, then Remove only the original cone: the mirrored cone remains. Undo restores the removed bodies. To inspect or revise an existing Remove step, double-click its timeline chip.

**Split Body** divides an exact solid using an infinite flat plane. Select the body, then choose a flat face, a construction plane, or XY/XZ/YZ. Origin planes use the body's component axes. The visible rectangle or selected face's boundary does not limit the cut. Each resulting piece becomes a separate body. A plane that does not divide the solid is refused; curved faces and imported mesh bodies are not supported as the split target.

**Join Bodies** opens the existing Combine workflow. Choose the target to keep, select the other bodies, and use Join. The result belongs to the target's component; Keep Tools controls whether the other bodies remain. It can rejoin pieces from a split. Changing or deleting the original modeling feature is a different history edit from recording a later Remove step.

## Move with arrows and rings

Open Move for a selected body or component. The preview displays **red X, green Y, and blue Z arrows**. Drag an arrow along its direction to change only that translation value. Hold Shift for finer movement. The arrow colors and labels identify coordinate axes even when the model is rotated.

Drag a matching rotation ring to turn around that axis. Shift snaps a ring gesture to 15° increments. When a ring is edge-on, a labelled rotation bar replaces it: drag the bar horizontally to rotate. An arrow pointing toward the eye has a labelled **depth** handle; drag along that handle to move along the same 3D axis. You can also orbit to get a clearer view. Handle size stays readable as you zoom.

Body handles use the owning component's axes. A body ring gesture turns around the current body's center and adjusts translation to keep that center still. Directly typed Transform rotations retain the existing rotation-about-component-origin convention. Component handles use the **parent** component's axes and rotate around that component's own origin. Primitive handles use the owning component's axes and rotate around the primitive's Position.

Dragging changes only the open dialog's preview. Releasing keeps the chosen values available for review. **OK** accepts one edit; **Cancel** discards it. Undo restores the accepted edit in one step. The camera should remain still while a handle is being dragged. Switching dialogs or changing the document must cancel any unfinished gesture.

## Place a primitive in the view

Open **Model → Primitives**, choose the shape and its dimensions, then use **Place in view**. Choose XY, XZ, or YZ, or pick a flat face or construction plane. Click the desired position. With **Align** enabled, the primitive's local +Z axis follows the chosen plane's normal. Turning Align off preserves the current rotation.

After placing it, use the arrows/rings or Position and Rotation fields to refine the result. The primitive's origin still follows the conventions in the [primitive guide](PRIMITIVES_0.3.md): a box uses its minimum corner, a cylinder or cone its base center, and a sphere or torus its center.

This placement captures **numeric position and orientation once**. Moving the selected face or construction plane later does not move the primitive with it. For a feature that follows a construction plane, create an attached sketch on that plane and model from the sketch.

## Set pattern spans graphically

A Linear pattern has a handle at its last copy along each enabled direction. Drag that handle to set the overall span; the dialog converts it to spacing between neighboring copies. For Count 4, a 30 mm span means 10 mm spacing. A rectangular grid has two independent handles, one for each direction.

The handle can capture a visible sketch point, line, or arc. It aligns the last copy's coordinate **along the chosen pattern axis**. The other coordinates and elevation remain unchanged, so a target off the pattern's row does not move the entire pattern onto it. This captures a numeric spacing; it does not create a persistent constraint to the sketch. Changing the target later requires another adjustment. Orbit if a pattern direction points directly at the eye and its handle has no usable screen direction.

Counts include the original body. Negative spans reverse a direction. Editing an existing pattern uses the same handles and updates that pattern feature when accepted; Cancel preserves its previous spacing.

## Sticky sketch placement

While placing geometry, a nearby line, circular arc, circle, or supported endpoint can capture the cursor. Capture starts within **10 screen pixels** and stays until the pointer is more than **18 pixels** away. This makes it easier to click the intended geometry without losing it to a tiny hand movement. An outward flick or holding **Alt** releases the capture.

Placing on supported geometry records the sketch relationship: shared endpoints stay coincident, and a point on a line or circular curve retains that relationship during later edits. Splines support their endpoints here; capturing the interior of a spline and general point-on-spline constraints are outside this feature. Watch the highlighted target before clicking, especially where several items overlap.

The [manual tests](TEST_PLAN_0.3.0.md) cover preview/Cancel/Undo behavior, moved coordinate frames, both themes, and the distinction between persistent sketch relationships and one-time primitive/pattern placement.
