# Build a 30 mm bishop in Ferrender

This tutorial recreates the photo-based bishop using Ferrender's desktop tools: draw a closed side profile, revolve it, and cut the diagonal slot. Start from a blank document; no scripts, API commands, imported solids, or AI assistant are required.

The silhouette matches the supplied bishop model closely. Its proportions were estimated from one photograph; only the 30 mm total height was specified. The red annotation on the photograph is not part of the piece.

Written for the interface in **Ferrender 0.1.0, commit `71293b5`**. Check **Help → About Ferrender** if a control differs. The instructions and labels were checked against that source version. The numerical recipe was rebuilt and checked with Ferrender's modeling engine; the complete mouse-and-keyboard walkthrough has not been independently performed by a human.

![Profile and finished silhouette with dimensions](docs/images/bishop-profile-guide.png)

## What you will make

| Detail | Size |
|---|---|
| Overall height | 30 mm |
| Maximum base diameter | 13.8 mm |
| Largest collar diameter | About 10.6 mm |
| Diagonal slot width | 0.9 mm |
| Slot direction | 28° left of vertical, when viewed from the front |
| Slot cutting depth | 16 mm total, symmetric about the front plane |

The outline is the detailed part of this exercise. Work on the base, stem, collars, and head in separate sessions if helpful. Save between sections.

## 1. Start a millimeter document

1. Choose **File → New**. Save an existing design before replacing it.
2. In the bottom status bar, set **Units** to **mm**.
3. Choose **File → Save As…** and name the document `bishop-tutorial.ferr`.
4. Choose **Edit → Parameters…**. In the new row, enter `total_height` as the name and `30 mm` as the expression, then click **Add**. Close the Parameters window.

This parameter will control a final scale feature. The outline itself is drawn at 30 mm. Do not also use `total_height` to drive the outline's height: that would apply the size change twice when the scale feature is added.

## 2. Start the side-profile sketch

1. Choose **Sketch → New Sketch**.
2. Leave **Offset** at zero and click **XZ (front)**.
3. Use **Look At** in the Sketch Palette if you need to face the sketch squarely.
4. Keep **Construction** off for now. Turn **Snap to Grid** off for the precise small details; snapping to existing points still works.

On this sketch, horizontal is world **X** and vertical is world **Z**. In the tables below, `(X, Z)` means **radius from the centerline, height above the bottom**. A radius of 6.9 mm makes a diameter of 13.8 mm after revolving.

### Put in the two straight starting edges

1. Choose **Line** (`L`). Click the origin, move straight upward until vertical inference appears, enter **30 mm** in the Length box, and press **Enter**. Press **Escape** to end the line chain.
2. This line runs from **P0 = (0, 0)** to **P24 = (0, 30)**. Keep it as ordinary geometry: it closes the profile along the axis. Do not make it a construction line.
3. Start another **Line** at the origin, move horizontally right, type **6.2 mm**, and press **Enter**. End the chain with **Escape**. The new endpoint is **P1 = (6.2, 0)**.
4. Use **View → Fit** (`F`) and zoom as needed.

**Checkpoint:** you have an upright 30 mm line and a 6.2 mm bottom line joined at the origin. You are drawing only the right side of the bishop, not both sides.

## 3. Place accurate outline points with construction rectangles

The current GUI does not have an absolute X/Y coordinate box for points. This workaround gives you points at known coordinates using the existing Rectangle tool. The construction rectangles stay in the sketch as guides and are ignored by Revolve.

For example, to place **P2 = (6.9, 0.7)**:

1. Choose **Select** (`S`) to finish any drawing gesture.
2. In the Sketch Palette, tick **Construction**.
3. Choose **Rectangle** (`R`) and click the origin for the first corner.
4. Move the pointer up and right. Type **6.9 mm** in **Width**, press **Tab**, type **0.7 mm** in **Height**, and press **Enter**.
5. The opposite corner is P2. Its two dimensions and the origin hold its position.

Use the same method for **P3 through P23**, taking their coordinates from the **End point** column in the next section. All of those points lie above and to the right of the origin. P0, P1, and P24 already exist. The labels P0–P24 are names used in this tutorial; Ferrender does not automatically put those labels on the sketch.

If a guide corner is very close to another point, zoom in before clicking it later. Look for the square point-snap indicator. Click the actual corner rather than nearby empty space. Hide **Show Dimensions** temporarily if the guide dimensions obscure the outline; that does not remove their constraints.

Leave the guides in place. Deleting them can remove the dimensions holding your outline points.

## 4. Connect the profile with lines and arcs

Turn **Construction off** in the Sketch Palette before drawing the real outline.

The Arc tool in this version is **center → start → end**. It is not a three-point arc through the silhouette. Each row gives an approximate center to aim at, the two endpoints to snap to, and a radius to dimension afterward. Some centers are well outside the bishop; that is normal for shallow curves.

For every **Arc** row:

1. Choose **Arc** (`A`). Click roughly at the listed center. Avoid snapping that center to an unrelated guide point or guide edge; the center needs to be free to settle as the radius is set.
2. Click the row's **From** point, then its **To** point, using the prepared guide corners. Ferrender draws the shorter arc between them.
3. Choose **Dimension** (`D`), click the new arc, enter its listed **Radius** followed by `mm`, and confirm with **Enter**. The endpoints are held by the guides; the center can move slightly to satisfy the radius.
4. Compare the bulge with the diagram. If the arc bulges the wrong way, undo it and place its center on the other side of the chord.

For a **Line** row, choose **Line**, snap to the two listed endpoints, then end the chain. Do not add a length dimension to a line already located by the guide points unless you intend to change the construction scheme.

**Edges 1 and 25 already exist from step 2. Do not draw them again.** Start at edge 2. Work through the remaining rows in order. Centers are locating hints; the radius and the fixed endpoints determine the curve. The six-decimal radii provide a close numerical recreation, not a requirement to position the mouse to six decimal places.

| Edge | From → to | End point (X, Z), mm | Tool | Approximate arc center (X, Z), mm | Radius, mm |
|---|---|---|---|---|---|
| 1 | P0 → P1 | (6.2, 0) | Line | — | — |
| 2 | P1 → P2 | (6.9, 0.7) | Arc | (6.2, 0.7) | 0.7 |
| 3 | P2 → P3 | (6.2, 1.4) | Arc | (6.2, 0.7) | 0.7 |
| 4 | P3 → P4 | (6.5, 2) | Arc | (5.89, 1.93) | 0.614003 |
| 5 | P4 → P5 | (5.25, 4.55) | Arc | (4.05431, 2.382505) | 2.475421 |
| 6 | P5 → P6 | (4.05, 6.3) | Arc | (6.721839, 6.84569) | 2.726995 |
| 7 | P6 → P7 | (4.45, 6.95) | Arc | (3.90059, 6.840022) | 0.56031 |
| 8 | P7 → P8 | (3.45, 8.2) | Arc | (3.336628, 7.084302) | 1.121443 |
| 9 | P8 → P9 | (2.15, 15.2) | Arc | (18.499275, 14.61558) | 16.359717 |
| 10 | P9 → P10 | (2.45, 15.6) | Arc | (2.499216, 15.250588) | 0.352861 |
| 11 | P10 → P11 | (4.45, 15.6) | Line | — | — |
| 12 | P11 → P12 | (5.3, 16.35) | Arc | (4.630505, 16.252094) | 0.676616 |
| 13 | P12 → P13 | (4.2, 16.95) | Arc | (4.402561, 16.013028) | 0.958618 |
| 14 | P13 → P14 | (3.45, 17.15) | Arc | (3.987069, 17.657759) | 0.739095 |
| 15 | P14 → P15 | (4, 17.8) | Arc | (3.480225, 17.682117) | 0.532975 |
| 16 | P15 → P16 | (3.05, 18.45) | Arc | (3.11408, 17.524425) | 0.92779 |
| 17 | P16 → P17 | (3.7, 19.02) | Arc | (3.200348, 18.934164) | 0.506971 |
| 18 | P17 → P18 | (2.55, 19.75) | Arc | (2.825567, 18.91329) | 0.88092 |
| 19 | P18 → P19 | (2.55, 20.15) | Line | — | — |
| 20 | P19 → P20 | (4, 23.35) | Arc | (1.973258, 22.339852) | 2.264527 |
| 21 | P20 → P21 | (1.55, 27.65) | Arc | (-29.031129, 7.377903) | 36.690099 |
| 22 | P21 → P22 | (1.1, 28) | Arc | (2.2, 28.95) | 1.453444 |
| 23 | P22 → P23 | (1.65, 28.9) | Arc | (0.994903, 28.682282) | 0.690329 |
| 24 | P23 → P24 | (0, 30) | Arc | (0, 28.2125) | 1.7875 |
| 25 | P24 → P0 | (0, 0) | Line | — | — |

### Recognize the sections as you go

- **Edges 2–8:** rounded foot and bell-shaped base.
- **Edges 9–10:** long concave stem and its transition into the collars.
- **Edges 11–18:** three turned collars. Edge 11 is a short horizontal ledge.
- **Edge 19:** short vertical neck under the head.
- **Edges 20–22:** pear-shaped bishop head.
- **Edges 23–24:** small rounded finial, ending exactly at the top of the 30 mm axis line.

The center of edge 9 is around X = 18.5 mm; the center of edge 21 is around X = −29 mm. Pan or zoom out to place them, then zoom back in to select the endpoints and arc. A future three-point or tangent-arc tool would make these two curves much easier to draw.

Do not add Tangent to every pair of arcs in this numeric recipe. The supplied recreation uses several independent circular arcs; additional tangent constraints can conflict with their already fixed endpoints and radii.

**Checkpoint:** the ordinary lines and arcs form one closed loop from the origin, around the right-hand outline, to the top, and back down the axis. All extra rectangles are construction geometry. Save the file.

## 5. Revolve the outline into a solid

1. Click **Finish Sketch**.
2. Choose **Model → Revolve**. If necessary, select the outline sketch in the timeline first.
3. Check **Profiles**. Select the single closed half-profile in the viewport if it is not already selected.
4. Set **Axis = Y** and **Angle = 360 deg**.
5. Set **Operation = New Body**, inspect the preview, and click **OK**.

**Why Y, when the finished bishop is vertical in Z?** The Revolve dialog's X and Y axes belong to the sketch. On an XZ sketch, the sketch's Y direction is world Z. Revolving about sketch X would turn the shape around the wrong axis.

Choose **View → Home** and **View → Fit**. Select the body in the Browser and inspect its status-bar dimensions. It should be approximately **13.8 × 13.8 × 30 mm**, and identified as an **exact** body. The head is still solid at this point.

Right-click the sketch's timeline chip and choose **Rename**: `Turned bishop outline`. Rename the revolve `Turned base stem collars and head`. Save again.

## 6. Draw the diagonal slot

Use a fresh origin plane, not a face on the curved head.

1. Choose **Sketch → New Sketch**, leave **Offset = 0**, and select **XZ (front)** again.
2. Face the sketch with **Look At**. If the solid gets in the way, temporarily hide the body with its Browser visibility control. Restore its visibility before previewing the cut.
3. Tick **Construction** and use origin-based rectangles to locate the four corners below. For a negative X, move the pointer **up and left** before entering the positive Width magnitude. Height is always the positive Z value. For example, A uses Width `0.297326 mm` and Height `21.238738 mm`, with the pointer left of the origin.
4. Turn **Construction off**, choose **Line**, and connect **A → B → C → D → A**, snapping to the guide corners. End the line chain.

| Corner | X, mm | Z, mm |
|---|---|---|
| A | -0.297326 | 21.238738 |
| B | 0.497326 | 21.661262 |
| C | -4.197389 | 30.490738 |
| D | -4.992042 | 30.068214 |

This is a narrow, tilted rectangle drawn with four lines. The standard Rectangle tool makes an axis-aligned rectangle, so use it only for the construction guides here.

The slot is **0.9 mm wide measured perpendicular to its long sides**, **10 mm long**, and tilted **28° left from vertical**. Its bottom-center is approximately **(0.10, 21.45)**. The top corners intentionally extend above and outside the head; this is what opens the cut to the outside instead of making an enclosed pocket.

Click **Finish Sketch** and rename this sketch `Diagonal mitre slot 0.9 mm`.

## 7. Cut through the head

1. Restore the body's visibility if you hid it.
2. With the slot sketch selected, choose **Model → Extrude**.
3. Select the narrow closed slot profile.
4. Set **Distance = 16 mm**, leave **Through all** unchecked, and leave **Taper** empty or at `0 deg`.
5. Tick **Symmetric** and set **Operation = Cut**.
6. Inspect the preview and click **OK**. Rename the feature `Cut diagonal slot through head`.

**Symmetric means 16 mm total: 8 mm on each side of the sketch plane.** This passes through the full head. You should still have one connected bishop, not a separate cutter body.

Use **View → Front** to check the diagonal opening. Then orbit slightly to see its depth. The stem, collars, and base should be untouched. Save.

## 8. Add the overall-height control

The complete shape is already 30 mm high. Add a final uniform scale feature so one parameter resizes the entire piece, including the slot.

1. Select the bishop body.
2. Choose **Model → Move / Rotate / Scale**.
3. Leave **Move X/Y/Z** at zero and **Rotate X/Y/Z** at zero.
4. Enter **`$total_height / (30 mm)`** in **Scale** and click **OK**.
5. Rename the feature `Overall height from total_height`.

With `total_height = 30 mm`, the scale is 1. As a check, change the parameter to `36 mm`: the whole bishop should become 36 mm tall and the base 16.56 mm wide. Restore it to **30 mm**, then save. Scaling also changes slot width: it would become 1.08 mm at 36 mm height.

## 9. Check and export

Your timeline should contain these five features:

1. Turned bishop outline — sketch.
2. Turned base stem collars and head — revolve.
3. Diagonal mitre slot 0.9 mm — sketch.
4. Cut diagonal slot through head — extrude cut.
5. Overall height from total_height — scale.

Select the body and check that the final height is **30 mm**. Look for failed-feature messages in the status bar and error-marked timeline chips. Confirm the base is flat, the finial is round, the slot opens through the head, and the document contains one body.

Choose **File → Save** to preserve the editable `.ferr` design. Choose **File → Export STL…** for printing; export in millimeters and use 100% scale in the slicer. Choose **File → Export STEP…** if you also want the exact solid for another CAD program.

When experimenting with the timeline, hold and move the marker to choose a position, then release to rebuild. The model stays unchanged while the marker is held. Wait for **Updating model…** to finish before starting another drag; **Escape** cancels a drag.

![Completed photo-based bishop](docs/images/bishop-finished.png)

## Troubleshooting

| What you see | What to check |
|---|---|
| Revolve has no closed profile | Check every outline junction. Use the point-snap indicator; two nearby points are not necessarily the same point. Verify the 30 mm axis edge is ordinary geometry. |
| Many extra regions appear | Some construction rectangles may have been drawn as ordinary geometry. Select their edges and use Toggle Construction; do not convert the actual outline. |
| An arc cannot accept its radius | Confirm the endpoints and units, check that the listed radius exceeds half the chord length, and undo any unintended constraint fixing its center. |
| A shallow curve appears enormous or backwards | Check the center hint, especially edges 9 and 21. These have distant centers and short sweeps. |
| A finial or collar looks angular | Check whether you accidentally used Line in place of Arc. Some visible surface faceting is display tessellation; STEP preserves the exact curved surfaces. |
| Revolve makes a sideways shape | Use the sketch's Y axis on the XZ sketch. |
| The cut does nothing | Check Operation = Cut, that the slot is a closed ordinary profile, and that its lower end intersects the head. |
| Only one side of the head is cut | Tick Symmetric; the sketch is at the middle of the body, not on its outside face. |
| Two bodies appear | The slot extrusion was probably New Body. Edit that feature and change it to Cut. |
| Changing total_height does nothing | Check the final scale expression, and make sure the timeline is rolled to the end. |
| Size changes twice | Use a literal 30 mm for the source outline and the parameter only in the final scale feature. |
| Fit zooms much farther out while sketching | Fit includes construction geometry and arc-center points. Pan and zoom manually around the part. |

## What would make this easier in Ferrender?

No new solid-modeling feature was required for this bishop. Revolve, true circular arcs, a symmetric cut, and uniform scale were sufficient. The main friction is creating the profile accurately in the human interface.

| Priority | Addition | How it would help |
|---|---|---|
| Highest | Point-coordinate editing and horizontal/vertical position dimensions | Enter `(radius, height)` directly and remove the construction-rectangle workaround. |
| Highest | Three-point arcs and tangent arc chains | Draw a curve through silhouette points without locating distant circle centers. |
| High | Reference-image canvas with opacity and two-point scale calibration | Put the photo behind the sketch, calibrate its height to 30 mm, and compare the outline while drawing. Calibration would not remove perspective distortion from the original photo. |
| High | Editable splines with endpoint and tangent constraints | Adjust the stem and pear-shaped head with fewer curve segments. |
| Useful | Profile gap highlighting and a clear closed-region preview while sketching | Find the exact junction preventing a revolve. |
| Useful | Optional finer viewport tessellation or smoother display | Judge subtle curves more easily without relying only on the exported solid. |

For closer proportions, a second straight-on photograph and measurements would help too. Software cannot determine the hidden dimensions reliably from one perspective image.

One separate API rough edge surfaced while making the matching pawn: a calculated arc center very close to zero was rejected when its numeric value became scientific notation. Using the mathematically exact zero avoided the problem. That is input handling to improve, not a missing modeling operation; it is not needed for the manual workflow above.
