# Modeling and rebuild cost in 0.5: what the 0.4.0 evaluation leaves open

Status: **plan for 0.5, proposed; nothing here is committed.** Written 2026-10-09 against `main` at `1c77a50`, using the 0.4.0 build at `d546ca0`. It reads [the large-assembly evaluation](../0.4/evaluation.md) together with two small headless probes run afterwards, and turns them into a list for 0.5 planning, with the few items that could instead be a 0.4.x patch marked as such. Nothing here is committed. The companion for scripting is [0.5-scripts.md](plan-scripts.md).

Probe evidence (scripts, designs, screenshots, logs) is in the private archive under `ferrender/testing/0.4-review/evidence/rebuild-and-pumpkin-probes/`, indexed from that folder's README. The reference photograph used for the pumpkin is personal and is not in the archive or the repository.

## 1. Rebuild cost: the one that decides whether large assemblies are usable

### What the evaluation measured

On the 878-feature, 638-body house: a fresh rebuild of 21–22 s, and 21–83 s for edits that change no geometry (showing a component, activating the already-active root, deleting an unused parameter). The evaluation names `Session::edit` and repeated exact-solid copying in `Document::rebuild_with` as the suspects, without a measured share.

### What the probe adds

`boxes.rhai` makes N separate box primitives, one feature each, and nothing else. Making a box costs well under a millisecond, so any growth beyond linear is overhead.

| N boxes | one fresh rebuild (`ferrender check --rebuild`) | generating by script (`ferrender run`) |
|---|---|---|
| 100 | 0.23 s | 8.5 s |
| 200 | 0.97 s | 62 s |
| 400 | 3.64 s | 476 s |

Rebuild is quadratic (about 4× per doubling). Script authoring is cubic (about 8× per doubling), because every host call rebuilds the whole prefix. Extrapolating the quadratic term alone to the house's 878 features and 638 bodies gives on the order of 12 s of pure copying per rebuild, which is most of its 21 s.

### Where it is in the code

- [doc.rs](../../../crates/fr-core/src/doc.rs), `Document::rebuild_with`: before applying each feature, `let mut bodies = built.bodies.clone();` copies every body built so far so that a failed feature can leave the list untouched. `Body` owns its OpenCascade solids, display mesh, edges and tags; cadrum's `Solid::clone` is an OCCT deep copy of the topology. So a rebuild performs features × bodies deep copies.
- [doc.rs](../../../crates/fr-core/src/doc.rs), `Session::edit`: every edit snapshots the document for undo and then rebuilds everything. [api.rs](../../../crates/fr-core/src/api.rs) routes `set_visible`, `activate_component`, renaming and parameter deletion through it, so a visibility toggle costs a full rebuild.
- Scripts and the MCP batch go through the same path once per call.

### What closes it

**a. Stop copying every body per feature.** Patch-sized; a candidate for 0.4.x. `apply` and `merge` mutate a known, small set of bodies: a new-body feature (primitive, extrude/revolve/sweep with `new`, import, text, pattern) only pushes at the end and cannot fail after mutating; join/cut/intersect touch only the bodies of the same component whose bounds overlap the tool (`merge` computes exactly this list before doing anything); fillet, chamfer, shell, hole, thread, attached text, transform, mesh ops and relief touch one declared body; remove and split name theirs. Snapshot only those bodies and restore them on error. The alternative with wider benefit is copy-on-write bodies (`Arc` around solids, mesh, edges and tags, cloned on first write), which also makes undo snapshots, `Session::fork` and cache capture cheap. Test: `ferrender check --rebuild` on every archived design must still report "cache matches the rebuild"; the boxes probe should become close to linear; rerun the evaluation's follow-up check 1 on the house.

**b. Metadata edits without a rebuild.** Patch-sized. Visibility, activation, names, body labels and unused-parameter deletion should take the undo snapshot and update `built` directly (component visibility is a fold over the component tree and can be recomputed in microseconds). The evaluation's follow-up check 2 is the test: geometry identical before and after, undo/redo and save/reopen unchanged, and a genuine geometry edit still invalidating dependents.

**c. Incremental rebuild.** 0.5 work, the large item. Keep each feature's output keyed by the feature's serialized form plus the keys of the bodies it consumed; an edit at feature k reuses every stored output whose inputs are unchanged. Components make this natural, since most of the house is hundreds of independent small histories. Script re-runs and timeline rollback get it for free. This is what turns authoring from cubic to linear, and it should be designed together with (a), because (a) decides what a "body output" is.

**d. Cache capture that explains itself and scales.** [cache.rs](../../../crates/fr-core/src/cache.rs) returns no cache above `MAX_CACHE_BYTES` (32 MiB) and the save response did not say so; it now does, and for the house it says "the geometry is over the 32 MiB cache limit", which confirms the evaluation's reading. Smallest fix: report the reason and the attempted size in the save result and the UI. Larger: a budget on disk rather than a fixed limit, keeping the existing CRC, authentication and foreign-file rules.

**Done in 0.4.0 (9 October, after this note was first written):** (a) as a journal in `rebuild_with` that copies a body only when a feature first changes or removes it: 400 boxes rebuild in 0.33 s instead of 3.64 s, generating them by script takes 31 s instead of 476 s, and `ferrender check --rebuild` on the house takes 5.3 s instead of 21.5 s on the same Mac; (b) for showing and hiding, activation, names, and undo/redo of those steps; and the reporting half of (d) as `cache_skipped` in the save result and the app's save message. The rebuild cost that remains is the genuine geometry work plus the per-call rebuild of the whole prefix, which is (c). (d)'s budget half belongs with (c).

## 2. Loft and the third dimension of sweep: the pumpkin

The probe modeled a real blown-glass pumpkin (ribbed body, curled tapering stem) using only 0.4.0 features: a revolved profile; a circle swept along an outward copy of the profile as the groove tool; a circular pattern of eight; a cut; and a circle swept along a planar arch for the stem. The result is a recognizable ribbed pumpkin with an arched stem. It took 180 s to author through ten script calls and 17 s to rebuild from the saved file, almost all of it the eight-tool cut.

What it could not do:

1. **The stem.** The real one curls in three dimensions, tapers to a point and twists. Sweep paths must lie in one sketch plane and the profile cannot change size or turn, so the stem is a planar, constant-diameter arch.
2. **The lobes.** A pumpkin's cross-section changes continuously from a point to an eight-lobed curve and back. Cutting grooves is a workaround; a loft between a few sections is the honest feature, one kernel call, and would also serve hulls, handles, bottles, ducts and most "organic but still parametric" shapes. The README already lists loft as absent.
3. **Splines take exactly four fit points.** The outline is two splines joined at the equator, which leaves a visible crease ring. An n-point spline (with the same four-point default in the UI) removes it.
4. **`fillet_edges` with `"edges": "all"` was refused** after the patterned cut, with "the topology reference is ambiguous after the body changed". Diagnosed and fixed in 0.4.0 the same day: the cut leaves a few edges sharing a tag (each groove face meets the body's face along two curves), and such an edge is told apart only by a point within 1e-5 mm of it, but the point the body reported came from the drawn polyline, not the true curve. Reported and picked points are now moved onto the edge or face they name, and a reference learns that moved point along with its tag. On the pumpkin itself the kernel then refuses the blend at any radius, because "all" includes the near-tangent seam ring where the two four-point splines meet and the stem junction; that is the n-point spline item again, not a reference problem.
5. **No colour or material**, see section 3.

### What is already there

cadrum 0.8.20 exposes `loft(sections, ruled)`, `helix(radius, pitch, height, axis, x_ref)`, a 3-D `bspline` edge, and `sweep(profile, spine, orient)` over any edges, not only planar ones. The kernel side of both features exists; the Ferrender side is the work:

- **Loft: built** (see "Loft as built" below). The plan was: `FeatureKind::Loft { sketches: Vec<Id>, profiles, ruled, closed, op }` taking two or more closed regions from sketches on different planes (construction planes already stack them), tags per section so later fillets and shells find faces, the usual join/cut/intersect, the dialog and the MCP command. Mismatched vertex counts between sections are the classic failure and should be refused with the count, not silently twisted.
- **3-D paths for Sweep.** Two curve sources that are not sketches: a helix (axis, radius, pitch, turns, taper) and a 3-D spline through points, each a timeline feature with a handle in the viewport. `Sweep` gains `path3d: Option<Id>` as an alternative to `path_sketch`, plus `twist` (degrees over the path) and `scale` at the end (a tapering stem is scale 0.2). The existing span logic should carry over unchanged.
- **Splines with n points** in `add_geometry` and the solver, keeping four as the drawn default.

With these, the pumpkin is three sections lofted, a helix-and-spline stem swept with taper and twist, and a fillet: about five features and no Boolean.

### Loft as built

`FeatureKind::Loft { sections: [{sketch, profile}], ruled, op }`, the `loft` command (also `edit_feature`, scripts and MCP), Model → Loft with numbered sections picked in the viewport, and file format 14. Kernel tests are in `crates/fr-core/tests/loft_kernel.rs`, feature tests in `tests/loft.rs`, app tests in `uitest/loft.rs`.

- Unequal edge counts are refused with both counts, as planned. That includes a circle to a square, which is a common wish; the way through is to draw the circle as four arcs, and a "split to match" helper would remove the chore.
- Each section after the first is turned and, if need be, reversed so its corners sit nearest the matching corners of the one before, compared about each section's own centre. Without this, two squares started at different corners loft into a twisted solid with no error.
- Faces: the ends are caps, and each side face carries the entity of the first section's edge it grew from, so a fillet on a loft resolves by tag.
- Not built: `closed` (the binding's `loft` always caps the ends, so a ring of sections needs a kernel change), a section that is a single point (the pumpkin's poles: the binding takes edges only), sections with holes, and guide rails. The pumpkin therefore still needs small end sections rather than true points.
- The result is checked for positive volume, a closed triangulation that agrees with the exact volume, and extents that contain every section (and, with straight walls, do not exceed them). There is no closed-form volume to compare against, unlike extrude, revolve and sweep.

## 3. Materials and appearance

Today the viewport draws every body in one grey with a half-Lambert light and a fixed 0.85 alpha ([gpu.rs](../../../crates/ferrender/src/gpu.rs)). The design has no colour field; [APPEARANCE.md](../../adrs/0003-appearance-follows-the-system.md) covers only the light/dark theme. The evaluation's medium item (large assemblies are hard to read in uniform grey) and the pumpkin's translucent mottled glass are the two ends of the same gap.

A staged version that keeps geometry untouched:

1. **Per-component appearance in the design** (format version bump): colour, opacity, roughness, metallic, optional per-body override, inherited down the component tree. `set_appearance` in the MCP, a colour swatch in the browser, and nothing exported until STEP/3MF colour is deliberately added. This alone answers the house readability problem.
2. **A shading model worth the field names.** Per-body uniforms; a GGX specular lobe with Fresnel; a small baked environment (a sky gradient and a floor) for reflections; back-to-front body sorting for transparency, since bodies are convex enough in practice and order-independent transparency is not worth its cost here. Presets: matte, plastic, metal, glass. Glass is low roughness, high opacity falloff by Fresnel, tinted transmission. That reproduces the photo's sharp highlights and the amber tint; the mottling is a texture and stays out of scope, or becomes a procedural noise tint later.
3. **Screenshots and the thumbnail** render with the same shader, so the `.ferr` preview and MCP screenshots show the materials.

cadrum's `color` feature (per-face colour map on a solid, on by default) is available if per-face colour is ever wanted for STEP export, but per-component display colour is the right first step.

## 4. Smaller items from the evaluation

- Box and other primitive dimensions stop at 10 000 mm; a building floor exceeds it. State the limit in the refusal and make the model-scale limits consistent across creation tools.
- Component reparenting that preserves world placement, references and ownership, with a clear refusal when a dependency crosses the move.
- Body naming that follows the feature name (`Body12` is what the house ended up with) and renames that survive rebuilds.
- Linked component instances for repeated furniture; measure before claiming a performance benefit.
- Named views and simple annotation for reviewing a draft before detailing.
- STEP export keeps the component hierarchy (assembly STEP), which also makes colour export meaningful.

## 5. Suggested order

1. Done before 0.4.0 shipped: section 1 (a), (b), the cache reason, and the `"edges": "all"` bug (2.4). The evaluation's follow-up checks 1 and 2 on the house itself remain to be rerun on the release build.
2. Decide incremental rebuild (1c) and cache budget (1d) with [the scripts isolation decision](plan-scripts.md#1-isolation-the-one-that-changes-the-architecture); they share the question of what a feature's output is.
3. Loft, then 3-D sweep paths with twist and scale, then n-point splines (section 2). The pumpkin script in the archive is the acceptance model: it should shrink to about five features and rebuild in well under a second.
4. Per-component appearance (3.1), then the shading model (3.2).
5. Section 4 as time allows, reparenting and body naming first.
