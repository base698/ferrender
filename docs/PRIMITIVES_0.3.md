# Solid primitives in Ferrender 0.3

Primitives are part of the **Ferrender 0.3.0 release**. They create an editable solid directly, without drawing a sketch first. Human acceptance checks are in [tests 18–25 of the manual test plan](TEST_PLAN_0.3.0.md#solid-primitives).

## Create a solid

Choose **Model → Primitives → Box, Cylinder, Sphere, Cone, or Torus**. The **Primitive** toolbar button opens Box; the dialog's **Shape** list switches to another shape.

Enter the dimensions and Position X/Y/Z. The preview updates before you press **OK**. Open **Rotation** for Rotate X/Y/Z. Dimensions and positions accept length units such as `20 mm` or `1 in`; rotations accept angles such as `90 deg`. Use `$width` to reference a parameter or `width = 20 mm` to define one in a field.

| Shape | Dimensions | What Position identifies | Default size |
| --- | --- | --- | --- |
| Box | Width, Depth, Height | The minimum corner, before rotation; the box extends along positive X, Y and Z. | 20 × 20 × 20 mm |
| Cylinder | Diameter, Height | Base center; the cylinder extends along positive Z. | Diameter 20 mm, height 20 mm |
| Sphere | Diameter | Sphere center; it extends equally in every direction. | Diameter 20 mm |
| Cone | Bottom diameter, Top diameter, Height | Bottom center; the cone extends along positive Z. A zero top diameter makes a point. | Bottom 20 mm, top 0 mm, height 20 mm |
| Torus | Major radius, Tube radius | Ring center; the ring lies in XY around the Z axis. | Major radius 15 mm, tube radius 5 mm |

The torus's major radius runs from its center to the center of the tube. Its outside diameter is `2 × (major radius + tube radius)`, and its hole diameter is `2 × (major radius − tube radius)`. The default torus therefore has an outside diameter of 40 mm, a hole diameter of 20 mm, and a height of 10 mm.

Position and rotation use the **active component's coordinates**. Rotation happens about the shape's origin, first X, then Y, then Z, followed by Position. Moving the whole component moves the primitive with it. Use **Place in view** to click a position on XY, XZ, YZ, a flat face, or a construction plane. **Align** points the shape's local +Z along the chosen plane's normal; turn it off to keep the current rotation. After placement, drag the colored X/Y/Z arrows or rotation rings to refine it. Face/plane placement captures numeric values once: it does not attach the primitive to that reference. See the [direct modeling guide](DIRECT_MODELING_0.3.md) for handle directions, fallback controls, and transaction behavior.

## Choose the operation

**New Body** is the default and creates a separate body, even when it overlaps another body. **Join**, **Cut**, and **Intersect** operate on bodies in the active component. To combine bodies from different components, use the separate Combine tool.

For a simple cut, first create a 20 mm box. Add a cylinder with diameter `6 mm`, height `30 mm`, Position `(10 mm, 10 mm, -5 mm)`, and Operation **Cut**. It passes through the box from below and makes a straight hole. Cancel should leave the box unchanged.

Dimensions must be between **0.001 and 10,000 mm**, except that one cone end may have diameter zero. Both cone ends cannot be zero; exactly equal diameters produce a cylinder. The torus's major radius must exceed its tube radius by at least **0.001 mm**. Each Position coordinate must stay within **−1,000,000 to +1,000,000 mm**. Invalid expressions or dimensions should show an error and leave the saved model unchanged until corrected.

## Edit and reuse

Each primitive adds one timeline feature. Double-click its chip to reopen its dimensions, placement and operation. Accepting an edit updates that same feature. Its shape type stays fixed; create a new primitive to use a different shape. Undo and Redo should restore the whole change, including any parameter defined while making it.

Primitives can be used as Pattern sources, including a two-direction Linear grid. Their exact solid faces can also be used for sketching and subsequent modeling operations. Save and reopen to preserve expressions, ownership and placement; STL and STEP exports contain the resulting placed solids. Files using primitives require Ferrender 0.3.0 or later, and older builds should refuse them clearly rather than omit the new shapes.

For testing, report the primitive shape, dimensions, operation, active component, and copied build information. Keep a small failing `.ferr` and a screenshot if the preview or placement differs from the accepted model.
