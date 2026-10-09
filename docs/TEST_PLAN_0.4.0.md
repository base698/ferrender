# Ferrender 0.4.0 manual test plan

This is the **Ferrender 0.4.0 regression and manual acceptance plan**. Report **Pass**, **Fail**, or **Unsure** by number. Allow about 90 minutes; you can stop between sections. The [review report](REVIEW_0.4.0.md) records automated coverage and its limits; this plan is for the things only a person at the screen can judge.

Save open work and open Ferrender 0.4.0. Use separate test files and keep your existing designs unchanged. Check **Help → About Ferrender → Copy build info**: it should say **0.4.0** with the commit matching the release bundle.

## Files and the container

1. **Old files open unchanged.** Open a design saved by 0.3.0 (the gallery designs from westcot.io, for example the sword or the coffee holder). Expect it to open with no errors and no "unsaved changes" state. Save it under a new name: a design with no images or meshes stays a plain JSON file (open it in a text editor to confirm `"format": "ferrender"` at the top). Open the saved copy in 0.3.0 if you have it: it must still open, unless you added a fillet, shell, thread, text on a face or face plane in 0.4 (those gain tags and the file becomes format 10, which 0.3.0 refuses with a clear message).
2. **A container with a thumbnail and a backup.** Open a 0.3 design that has a reference image or an imported STL, or add one, and save over it. Expect a toast naming the kept backup `name (0.3 backup).ferr`, the saved file to start with `PK` (it is a ZIP), and **File → Open Recent** to show its thumbnail if the list shows previews. Unzip the file: `manifest.json`, `design.json`, `images/` or `meshes/`, `cache/` and `thumbnail.png` should be there and `design.json` should be readable JSON. Save again: no second backup appears.
3. **Opens from the cache.** Make a design that takes a moment to rebuild (a block with a dozen holes and a fillet on every edge is enough), save it, and reopen it. Expect a toast "Opened from the saved geometry: N bodies without a rebuild" and an open that is visibly quicker than the rebuild when a valid cache was saved. Caches larger than 32 MiB and bodies with modeled threads are not stored; old algorithm revisions deliberately rebuild once. Edit any dimension: the design rebuilds as usual and the next save writes a fresh cache.
4. **A newer file is shown read-only.** Take any container and, in a text editor, raise `"version"` in its `design.json` to 99 (re-zip it, or use the API: this is what a file from a future Ferrender looks like). Open it. Expect the bodies to appear, a status-bar note "Read-only: written by a newer Ferrender", every edit refused with an explanation, and Export STL to work.

## Large meshes

Use the scans in `~/Documents/ferr-tests/large-stl` or any STL over a million triangles.

5. **Import and orbit.** Import the 3.5 million triangle Beethoven scan. Expect the import toast within a few seconds, naming triangles, vertices, shells and whether it is watertight. Orbit, pan and zoom: the view should stay smooth; a mesh over two million triangles draws a slightly coarser copy while the mouse is down and the full one when it stops. Click the scan: the pick lands where you clicked without delay and selection identifies the correct body. Individual triangle/face highlights are currently disabled above 200,000 triangles; whole-body selection is available.
6. **Mesh menu on a scan.** With the scan selected, use **Mesh → Decimate** to 300 000 triangles (quadric). Expect a result that still looks like the scan, a new `mesh_decimate` chip, and the status bar's triangle count to drop. Undo and redo it. Try **Decimate** again with Cluster (fast) to 50 000: coarse but quick. **Mesh → Cut Mesh** on the XY plane with an offset through the middle, keep one side, cap on: expect a flat capped half. **Mesh → Offset / Thicken** on an open surface (cut with the cap off first) with 2 mm straight down: expect a closed solid; `Measure` or the status bar should no longer report open edges.
7. **Save and reopen a scan.** Save the design with the decimated scan and reopen it. Expect the container to open from its cache without re-running the decimation, and the file size to be far below the STL's.
8. **Sculpt.** Import a smaller mesh (or subdivide a primitive twice with **Mesh → Subdivide**), open **Mesh → Sculpt**, and click the surface several times with Pull, then Smooth. Expect each click to raise or soften the surface under the pointer and to add a `mesh_sculpt` chip; undo removes one stroke at a time.

## Reliefs

9. **Relief from an image.** Choose **Mesh → Relief from Image**, pick any photo, keep the defaults (100 mm wide, 4 mm high, 2 mm base) and OK. Expect a slab whose surface rises where the photo is bright, as a closed body. Repeat with **Invert** on, 2.5 mm high and 0.6 mm base for a lithophane. Save: the file is a container with the image inside; reopen it and expect the relief unchanged.
10. **The face relief over MCP.** Follow `docs/tutorials/FACE_RELIEF_0.4.md` with your own photo and an MCP client, or run the `face-relief.rhai` sample from **Scripts → Samples** with a depth map. Expect a watertight solid mounted on a plaque, an exported STL, and a top view in which the face is recognisable.

## Faces and edges

Use a fresh document.

11. **A fillet follows its edge.** Create parameter `w = 20 mm`, sketch a rectangle 20 × 10 with its length dimensioned to `$w`, extrude 5 mm, and fillet the top edge at the far (+X) end with radius 1. Change `w` to 60 mm. Expect the fillet to stay on the far end (not jump to an edge near x = 20), and **get_object_info** on the fillet (or the timeline tooltip) to say it was resolved by tag.
12. **A removed face fails honestly.** Roll back before the fillet and cut the far end of the block away with a primitive box, then roll forward. Expect the fillet to show an error saying its edge is no longer there, rather than relocating to the new end.
13. **Shell and text survive a split face.** Shell a box leaving the top open; then, earlier in the timeline, cut a slot through the top. Expect the shell still to open a piece of the original top face. Put raised text on a face, then cut a slot through part of that face earlier in the timeline: the text stays on its face.

## Scripts

14. **Run a sample.** **Scripts → Samples → Spur gear** opens an inputs dialog (teeth, module, thickness, bore). Run with 18 teeth. Expect a progress window, then a gear body, a `Spur gear` chip at the start of the run and the sketch and extrude it made listed after it. One undo removes the whole run.
15. **Re-run, detach, delete.** Right-click the chip: **Edit Inputs and Re-run** with 30 teeth replaces the gear; the dialog remembered 18. **Re-run** repeats with the same inputs. **Suppress** removes everything the run made; unsuppress restores it. **Detach** leaves the sketch and extrude as ordinary features. Run the sample again and **Delete** the chip: the run and its features go together.
16. **Your own script.** **Scripts → New Script** creates `script-1.rhai` in your scripts folder and opens it in your editor. Change the box size, **Scripts → Reload**, and run it from the menu. Expect the box, and your inputs remembered on the second run. Make it write a file outside its folder (`write_text("/tmp/x.txt", "hi")`): the run must fail with a message about the allowed folders.
17. **Export a timeline.** Open a parametric design and choose **Scripts → Export Timeline as Script**. Run the exported script in a new document with a changed parameter: expect the design rebuilt with that value.
18. **Headless.** In a terminal: `ferrender run <samples>/spur-gear.rhai --input teeth=24 --save gear.ferr` (copy the sample from the menu to your scripts folder first) and then `ferrender check gear.ferr --rebuild`. Expect the gear saved, the check to report features and bodies with no errors, and exit code 0. Break the design (a fillet that is too large) and expect `check` to list the error and exit 1.

## Review regressions to verify before shipping

19. **Classic king.** Open `king-classic-75mm.ferr` from the review test pack. Inspect the turned base, curved stem, twelve crown flutes and rounded cross. Change `king_height` from `75 mm` to `90 mm`, then Undo. Expect one intact body at each size, no feature errors, and no detached jewel. Export STL and reopen the saved design.
20. **Organic turtle.** Open `sea-turtle-organic.ferr`. Orbit around the flippers, neck and shell; smooth faces should not have the former black stippling. The final mesh has 150,000 triangles, one shell, zero open/non-manifold edges. Roll back before the final Decimate and expect 767,968 triangles. Restore the timeline and wait for rebuilding to finish before dragging again. Try one Pull and one Smooth stroke, then Undo both. Save a copy and reopen it.
21. **No stale large mesh.** Open Beethoven, then a different large scan or the turtle without quitting. The new silhouette must replace the old one. Move it, hide/show it, and create a new document; old geometry must not linger. Orbit a >2 M mesh to exercise the coarse moving display. Try a face selection and whole-body selection, noting the limit in test 5.
22. **Safe Boolean refusal.** On a copy of Beethoven, try an intersecting mesh tool. A successful result must be a closed body. If the operation cannot make a reliable seam, expect an explicit error, no added broken feature, and both original bodies preserved. A silent success with missing volume or open edges is a failure.
23. **Script history and later edits.** Run Spur gear, add a Move afterward, then re-run the script with another tooth count. The Move should still address the gear. Suppress/unsuppress the ScriptRun, save/reopen and repeat. If you edit the document while a long script runs, the finished script must not replace your intervening work.
24. **Read-only cache safety.** Open the prepared future-version fixture. Try Move, Primitive, Mesh/Sculpt and Scripts. Expect a read-only explanation before any editing preview, with geometry still visible. Orbit and Export STL should work. Headless `check --rebuild` must explicitly refuse to verify a format it cannot rebuild.
25. **Non-mm sculpting.** On a copy of a small mesh in an inch document, Smooth and Flatten strength `0.25` are fractions, while Pull/Push depth and brush radius have length units. Switching brushes should choose an appropriate default and should not silently multiply smoothing by 25.4.

These are acceptance checks, not claims that every item has been hand-tested. See the review report for completed checks and any screen-access interruption.

## Observations

Record the build's commit, which checks passed, and anything unexpected. Known limits are listed in the [release notes](releases/0.4.0.md).
