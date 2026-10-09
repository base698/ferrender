# Ferrender 0.4.0 release acceptance run

This is the short, ordered session that decides whether `0.4-dev` becomes 0.4.0. It takes about two hours at one Mac. [TEST_PLAN_0.4.0.md](TEST_PLAN_0.4.0.md) stays the full catalogue; the numbers in brackets below point at its checks. [REVIEW_0.4.0.md](REVIEW_0.4.0.md) records what automated tests already cover.

The run is ordered by what would stop the release. Part A is the gate: any failure there is a no-go. Parts B and C walk the five planned features and Sweep against the targets in [the plan](0.4-release.md). Part D is the handful of judgments no test can make. Stop at the first failure in Part A; elsewhere note it and carry on.

Mark each step **Pass**, **Fail** or **Unsure**. A step passes only if every "Expect" holds.

## Before you start

1. Quit every running Ferrender, including review builds.
2. Start the build under test with its own settings, so your real settings, recent files and cache key are untouched:

   ```sh
   export FERRENDER_CONFIG_DIR="$HOME/Documents/ferr-tests/acceptance-0.4.0/config"
   mkdir -p "$FERRENDER_CONFIG_DIR" "$HOME/Documents/ferr-tests/acceptance-0.4.0/work"
   open -n ~/Desktop/Ferrender.app
   ```

3. Save everything you make into `~/Documents/ferr-tests/acceptance-0.4.0/work`. Work on copies of fixtures, never the originals.
4. Fixtures: the scans in `~/Documents/ferr-tests/large-stl`, the review pack in `~/Documents/ferr-tests/ferrender-0.4-review`, and any 0.3.0 design in `~/Documents/ferr-tests/ferrender-0.3.0-release`.

## Part A: gate (25 minutes)

**A1. The build is the one being released.** Help → About Ferrender → Copy build info, and in a terminal `~/Desktop/Ferrender.app/Contents/MacOS/ferrender --version`.
Expect: `Ferrender 0.4.0`; the commit is the head of `0.4-dev`; `Source: Clean checkout`; `Build: release`; the `OpenCascade:` line starts with `verified occt-` and a sha256, not `unverified`. Both outputs agree.

**A2. A 0.3.0 design opens, is not marked changed, and round-trips.** [1] Open a 0.3.0 design with no image or mesh. Save As a copy without editing.
Expect: no errors, no unsaved marker on open; the copy is plain JSON starting with `{` and `"format": "ferrender"`; the body's volume in the status bar or Measure is the same before and after reopening the copy.

**A3. A converting save keeps the original.** [2] Open a 0.3.0 design that has a reference image or an imported STL (or add an image to a copy saved by 0.3.0). Save over it.
Expect: a toast naming `name (0.3 backup).ferr`; that backup is byte-for-byte the old file (`shasum` both if you kept a copy); the saved file starts with `PK`; a second save makes no second backup.

**A4. Nothing is lost on a crash.** [26] In the isolated instance import the Bearded Man scan, wait about fifteen seconds for the recovery copy (no recovery warning appears), then force-quit that instance from Activity Monitor. Start it again with the same `FERRENDER_CONFIG_DIR`.
Expect: an offer to recover; Recover returns the same mesh, unsaved, with the same triangle count; Save As works.

**A5. A failed operation leaves the design alone.** [22] On the recovered scan, add a box primitive that pokes through it and Combine → Cut the scan with it. Bearded Man is not a valid closed solid.
Expect: either a closed result, or a clear refusal that explains why. After a refusal both bodies are still there, the timeline has no new broken feature, and Undo has nothing unexpected to undo.

**A6. A script cannot write outside its folders.** [16, 32] Scripts → New Script; make `run` call `write_text("/tmp/ferrender-acceptance.txt", "x")`; Reload; run it.
Expect: the run fails naming the allowed folders; `/tmp/ferrender-acceptance.txt` does not exist; the design is unchanged.

**A7. A newer file cannot be edited or mistaken for verified.** [4, 24] Open `future-version-preview.ferr` from the review pack.
Expect: the bodies show; a persistent "Unverified preview" read-only notice; Move, Primitive, a Mesh operation and Scripts each refuse before any preview appears; Export STL works.

## Part B: the five planned features (55 minutes)

### 1. File container

**B1. Container contents.** [2] `unzip -l` the container from A3.
Expect: `manifest.json`, `design.json`, `images/` or `meshes/`, `thumbnail.png`, and `cache/` when the design was built. `design.json` is readable JSON. `manifest.json` names Ferrender 0.4.0 and the kernel.

### 2. Geometry cache

**B2. Cached open, then an honest rebuild.** [3, 29] Build a block with a dozen holes and a fillet on every edge. Save, close, reopen.
Expect: the toast "Opened from the saved geometry"; a visibly quicker open. Change one dimension: it rebuilds normally. Then quit, start a second instance with a different `FERRENDER_CONFIG_DIR`, and open the same file.
Expect: no cached-open toast there, a rebuild, and the same volume. That second installation did not sign the cache, so it must not trust it.

### 3. Faces and edges by name

**B3. A fillet follows its edge.** [11] Parameter `w = 20 mm`; a 20 × 10 rectangle with its length dimensioned `$w`; extrude 5; fillet the top edge at the far +X end, radius 1. Set `w` to 60 mm.
Expect: the fillet is at x = 60, not stranded near x = 20.

**B4. A removed face is reported, not guessed.** [12] Roll back before the fillet, cut the far end off with a box, roll forward.
Expect: the fillet shows an error saying its edge is gone. It does not jump to the new end.

**B5. A generated face survives edits.** [27] 40 × 20 × 10 box; chamfer the far top edge 1 mm; put an offset construction plane on the chamfer face. Change the width to 60 and the chamfer to 2.
Expect: the plane is still on that chamfer face.

### 4. Meshes

**B6. Import and handle a multi-million-triangle scan.** [5, 21] Import `03-Beethoven.stl` (3.5 M triangles).
Expect: the toast within a few seconds, with triangles, shells and whether it is watertight. Click the scan: the pick is immediate and on the right body. Now open the turtle from the review pack without quitting.
Expect: the turtle replaces Beethoven completely; no stale silhouette.

**B7. Edit a scan.** [6, 7] On Beethoven: Mesh → Decimate to 300 000 (Quadric). Undo, Redo. Mesh → Cut Mesh through the middle on XY, keep one side, cap on. Save, reopen.
Expect: it still looks like the scan after decimation; the cut is a flat closed half with no open edges reported; the saved file is far smaller than the STL and reopens without redoing the decimation.

**B8. Sculpt.** [8, 25] On the turtle: Mesh → Sculpt, three Pull clicks, two Smooth clicks, then Undo five times.
Expect: each click changes the surface under the pointer and adds one chip; each Undo removes exactly one stroke.

**B9. Relief and lithophane.** [9] Mesh → Relief from Image with any photo at the defaults; again with Invert, 2.5 mm high, 0.6 mm base. Save and reopen.
Expect: closed bodies both times; bright is high in the first and low in the second; the reopened relief is unchanged.

**B10. A mesh Boolean gives a correct solid.** Two overlapping box primitives, each turned into a mesh by Mesh → Subdivide (1 level, Midpoint). Combine → Join, then on a copy Cut, then Intersect.
Expect: for 20 mm cubes offset by 10 mm on X, volumes of 12 000, 4 000 and 4 000 mm³, each one closed shell.

### 5. Scripts

**B11. Run, re-run, undo.** [14, 15, 23] Scripts → Samples → Spur gear. Run is enabled with the defaults. Run with 18 teeth. Add a Move to the gear. Right-click the chip → Edit Inputs and Re-run with 30 teeth.
Expect: a progress window, one chip that owns the sketch and extrude, the dialog remembering 18, the Move still applied to the 30-tooth gear, and one Undo removing a whole run.

**B12. Questions and Cancel.** [30] A script that calls `ask("How many?", "3")` then `confirm(...)` and makes that many boxes, each with its own `output_key`. Answer 2, Yes. Run again and press Cancel at the question.
Expect: two boxes and one chip the first time; nothing at all the second time.

**B13. Files appear only on success.** [32] A script that calls `write_text` and `export_stl` into its own folder and then `fail("stop")`. Run it. Remove the `fail` and run again.
Expect: first run, no new files and no `.ferrtmp` leftovers; second run, both files.

**B14. Export a timeline and replay it.** [17, 34] On the B3 block: Scripts → Export Timeline as Script. New document; run the exported script with `w` set to 45 mm.
Expect: the block rebuilt 45 mm long with its fillet. Repeat the export on the decimated scan from B7: an `.stl` appears beside the script and the script rebuilds from it.

**B15. Headless.** [18, 33] In a terminal, with the sample copied to your scripts folder:

```sh
F=~/Desktop/Ferrender.app/Contents/MacOS/ferrender
$F run spur-gear.rhai --input teeth=24 --save gear.ferr; echo "exit $?"
$F check gear.ferr --rebuild; echo "exit $?"
$F run slow.rhai --timeout 2 --save slow.ferr; echo "exit $?"
```

where `slow.rhai` keeps modeling:

```rhai
const META = #{ name: "Slow", description: "Keeps modeling until the time limit stops it." };
fn run(inputs) {
    for k in 0..100000 {
        primitive(#{ type: "box", width: 1, depth: 1, height: 1, position: [k * 2, 0, 0], operation: "new", output_key: "box" + k });
    }
}
```

Expect: exits 0, 0 and 1. The check prints the feature and body counts. The third prints "the run exceeded its time limit of 2 s" after about two seconds and `slow.ferr` is not written. A script that only loops, with no modeling calls, stops sooner with "Too many operations": that is the separate operation limit, not a failure of the time limit.

## Part C: Sweep (15 minutes)

Sweep was added after the plan was written, so it has no plan target. These are its release checks.

**C1. A mitred bar.** [S1] Sketch on XY: a 40 mm line along X, then 30 mm up from its end. Sketch on YZ: a 6 mm square around the origin. Model → Sweep.
Expect: the dialog shows one profile, the first sketch as Path, "2 pieces, open", a highlighted path and a preview with a clean corner. OK gives one body of 2 520 mm³ (36 × 70), a Sweep chip, both sketches hidden. One Undo removes it.

**C2. A closed ring.** [S2] New document. A 30 mm radius circle on XY centred at the origin. A sketch on XZ with a 4 mm radius circle centred at x = 30. Model → Sweep.
Expect: "1 piece, closed"; a torus with no flat cap or visible seam face; volume about 9 475 mm³ (π × 16 × 2π × 30). This is the check the review could not finish because the screen locked.

**C3. A tangent bend with a fillet that follows.** [S2] Path: a 40 mm line then a tangent arc. Sweep the 6 mm square. Fillet one long edge 1 mm. Edit the profile sketch and widen the square to 8 mm.
Expect: a smooth bend; after the edit the fillet is still on the same edge and there are no errors.

**C4. Refusals read sensibly.** [S4] On the L path set Orientation to Fixed. Then try a tangent arc of 2 mm radius bending toward the profile. Then draw the profile on the path's own plane.
Expect: three different messages, each naming the cause (fixed orientation, bends more tightly, a plane that crosses the path); OK is refused; the design is unchanged each time.

**C5. STEP carries true surfaces.** [S5] Export STEP from C3 and open it in another CAD program if one is installed.
Expect: the bend is one smooth face, not facets.

## Part D: what only a person can judge (25 minutes)

The review leaves these open. Record what you see even where there is no pass line.

**D1. Orbit and drag on a large mesh.** With Beethoven loaded, orbit, pan and zoom continuously for thirty seconds.
Record: does it feel smooth; does the coarse stand-in while moving look acceptable; does the full mesh return promptly on release. The plan asked for 60 fps at 5 M triangles and 30 fps at 12 M. Nothing has measured that.

**D2. Selection on a large mesh.** Hover and click faces on Beethoven.
Record: whole-body selection works. Individual face highlighting is switched off above 200 000 triangles; decide whether that is acceptable to ship.

**D3. The face relief is recognisably the person.** [10] Follow `docs/tutorials/FACE_RELIEF_0.4.md` with your own photo and a depth map, or run the `face-relief.rhai` sample.
Expect: a watertight solid on a plaque and an exported STL. Record: is it recognisable from the top view. This is the plan's second acceptance test and only you can call it.

**D4. A print.** Slice any STL exported during this run.
Record: the slicer reports no errors and the size is right. A physical print has never been made from a 0.4 export.

**D5. Cancel during a long call.** [31] Run a script that subdivides a large mesh twice and press Cancel at once.
Expect: "Cancelling…", then after two seconds a note that the call cannot be interrupted and a Stop waiting button; pressing it returns the design as it was. Record: is that acceptable behaviour to ship.

**D6. Linux and Intel Mac.** If you have either machine, repeat A1, B6, B11 and C1 there.
Record: the result, or "not tested". Only automated builds and tests have run on those platforms.

## Record sheet

| Step | Result | Note |
|---|---|---|
| Build commit from A1 | | |
| A1 to A7 | | |
| B1 to B15 | | |
| C1 to C5 | | |
| D1 to D6 | | |

**Go** needs every Part A step to pass, no Fail in Parts B and C, and a decision written down for D1, D2 and D5. After that, follow [RELEASING.md](../RELEASING.md).
