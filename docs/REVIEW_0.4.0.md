# Ferrender 0.4 independent review

Review baseline: `f38e1f7` on `0.4-dev`, inspected on 8–9 October 2026. The latest code fixes are in `5c0eb8c`; the delivered clean release build is `7a9fd6eb19d5cf43c356b429184e253d2ea063c6`, whose additional changes are documentation. The review used an isolated clone, three scoped review agents and a separate native app/configuration. This review does not publish or approve a release by itself.

The baseline had significant correctness and security defects despite its passing feature tests. Reproductions included successful script filesystem escapes, document identity loss after failed scripts, wrong Boolean volumes, decimation holes, stale GPU geometry and permanently suppressed script results. The reviewed candidate fixes these cases and adds explicit refusal where a reliable result is unavailable. No finite suite proves the absence of other bugs.

## Current assessment

Updated against the completed native UI evidence and repository state on **9 October 2026, 10:25 UTC**. No additional source changes were present after `7a9fd6e`. This update reconciles recorded results; it does not claim another full test or security-audit run.

The reproduced script, recovery, geometry, cache and rendering defects listed as fixed now have passing targeted checks. The final local suite passed **417 tests with zero failures**. Actual native workflows include king resizing/export, turtle sculpt/Undo and timeline rollback, large-scan selection/movement/save, script reruns with downstream edits, future-version refusal and large-mesh recovery after a forced stop.

| Status | Result or remaining work |
|---|---|
| Verified locally | Clean Apple Silicon release build; About shows `7a9fd6e`. Four native-saved designs rebuild successfully. Both native-exported STLs reimport with zero open edges. |
| Hosted verification pending | The exact delivered build's [macOS/Linux CI](https://github.com/base698/ferrender/actions/runs/37917098628) and the [code-fix CI](https://github.com/base698/ferrender/actions/runs/37916523681) are still running. Earlier CI success does not establish a pass for these commits. |
| Remaining human check | Continuous orbit/drag feel and the coarse moving display; 5 M/12 M interactive frame-rate targets remain unmeasured. Discrete view controls, Fit and numeric movement passed. |
| Known functional limitation | The private face-relief example still fails safely at its final mesh Combine: 173 open edges and 5 non-manifold edges in the attempted result. Do not mark that tutorial as an end-to-end pass. |
| Release decision | Keep CI and the remaining manual check open. Consider the documented mesh-Boolean and script-isolation limits before shipping. Main has not been merged and no release was published by this review. |

## Evidence and coverage

- **All-target build: passed.** The final clean release build also completed without warnings; its About dialog was checked in the native app.
- **Hosted CI:** earlier macOS/Ubuntu runs for [`4858b6a`](https://github.com/base698/ferrender/actions/runs/37876475897) and [`c7dd232`](https://github.com/base698/ferrender/actions/runs/37878285251) passed, including the Linux offscreen suite. The latest `5c0eb8c`/`7a9fd6e` runs remain pending as recorded above; archived earlier logs are in `evidence/hosted-ci.log`.
- **Latest full standard suite: 417 passed, zero failed, five ignored** after the unlocked-screen fixes, including native E2E/rendering tests. The ignored cases are three optional visual demonstrations and two fixture-driven stress tests; the turtle and 5 M/12 M opt-in tests were run separately and passed.
- **Dependency-verifier tests: four passed.** The real Apple Silicon archive also passed size/hash/extraction verification.
- **Doc-test command: passed; the crate currently contains no runnable doc tests.**
- **Clippy:** the initial review run completed without errors but reported style, complexity and dead-code warnings. The reported Cargo build warnings were subsequently addressed; the final release build is warning-free. A new full Clippy pass was not run for this documentation update.
- Logs and detailed model/compatibility measurements are supplied with the test pack. No physical print was made during this review.

Test machine: Apple M3 Max, 48 GiB RAM, macOS 26.2 (25C56), arm64. Native rendering tests use the local Metal adapter. The first unprivileged run could not create sockets or access a graphics adapter; those environmental failures were rerun with the required local access. A subsequent real timeline regression was fixed and its original assertions retained.

- **Direct native operation:** opened the isolated application, used Search, created a sketch/circle, finished the sketch, selected its profile, extruded it, fitted the view and saved through the macOS Save dialog. The classic king was also constructed through the running app's command bridge, not an external CAD kernel. The final models were rebuilt and verified through Ferrender's headless command interface after fixes.
- **Screen-access follow-up:** the initial review was interrupted by a locked Mac. The unlocked-screen checks below now cover native king, turtle, large-mesh, script and recovery workflows. Continuous drag/orbit performance remains unverified; discrete view controls and Fit were exercised.
- **Existing designs:** 76 saved test designs were opened and checked against both baseline and reviewed executables: 70 valid designs passed, six intentionally invalid constraint/file/plane fixtures failed as expected, and all file hashes remained unchanged. Private portrait assets were not copied into the repository or test pack.
- **Classic king:** a turned sketch profile with three-point arcs, revolve, twelve patterned crown cuts, symmetric cross extrusion, fillets, solid joins, a jewel and a height parameter. Ten modeling features form one exact body. Changing height from 75 to 90 mm and Undo both rebuild correctly; `.ferr`, STL and STEP exports are supplied. Exact volume is about 13,069.965661 mm³. The triangulated STL is about 13,064.1659 mm³; tessellation is an approximation, and its triangle count can vary slightly across exact-body rebuilds. Cache restoration, fresh exact rebuild and exported script replay agree on exact volume and bounds.
- **Organic turtle:** an original connected anatomical seed was generated outside Ferrender, then imported and refined using Ferrender's Repair, Loop subdivision, smoothing, Pull/Smooth sculpt strokes and quadric decimation. This is not a downloaded finished turtle or a claim of manual brush sculpting from an empty viewport. The seed has 191,992 triangles; subdivision produces 767,968; the delivered model has 150,000 triangles, 75,002 vertices, one shell, zero open/non-manifold edges and volume about 90,425.199726 mm³. Undo/Redo, saved-file reopen and exported-STL reimport pass.
- **Security:** harmless scratch-file exploits were reproduced before fixing. Tests cover real outside modules, leaf/dangling symlinks, failed script transactions, read-only caches, bounded source/output, malicious archives and amplified 3MF components. Tests use local temporary data, not personal files.

The test pack includes `king-classic-75mm.ferr/.stl/.step`, `king-classic-generator.rhai`, `sea-turtle-organic.ferr/.stl`, its seed, views, a read-only future-version fixture, compatibility and geometry evidence. The user copy belongs under `~/Documents/ferr-tests/ferrender-0.4-review/`. The repository contains code/tests/docs, not these large meshes or personal files.

## Fixed findings

| Area | Reproduced problem | Change and regression coverage |
|---|---|---|
| Script file access | Canonicalizing only a parent let an allowed leaf symlink escape; Rhai's default module loader imported outside files, even during META inspection. | Resolve/check leaf targets, refuse dangling escapes, disable import/eval and module resolution in both engines. |
| Script transactions | A failed API script could leave the original design pointing at another file; invalid typed inputs could empty the session. | Validate before ownership transfer; run on a session fork, commit success as one Undo, retain original file identity and history on failure. |
| Worker adoption | A finished worker could overwrite edits/open/Undo that happened while it ran. | Check source document, revision and path before adoption; decline stale results. |
| Script history | Suppressing a chip permanently altered its children; reruns broke downstream references; deletion orphaned children. | Derive suppression from ownership, reuse compatible output IDs, infer component ownership and share deletion cascade. |
| Script UI | Valid sample inputs showed “Nothing to apply” and disabled Run. | Exclude scripts from geometry previews, preserve Enter-to-run and test actual Run/Run again clicks. Native sample execution and rerun passed. |
| Large-design recovery | Mesh autosave expanded to legacy JSON, exceeded its size limit and left a stale warning after Save. | Store compact native payloads with bounded metadata, CRC, atomic replacement and legacy loading; 13 recovery tests and real native forced-stop/Recover passed. |
| Script exporter | Scalar/radian/reference parameters, keyword names and document state were lost; large embedded payloads produced unusable scripts. | Preserve expression inputs and supported state, bound serialization before large allocations, validate generated source, explicitly refuse unsupported states. |
| Resource bounds | Sources, logs and aggregate ZIP expansion had incomplete limits; small 3MF component graphs could expand exponentially. | Bounded regular-file reads/events, aggregate container/cache budgets, finite transforms and bounded component expansion. |
| 3MF coordinates | Inch-model vertices converted to millimetres but component/build translations did not. | Convert translations consistently; nested inch-model bounds regression. |
| Backups | A dangling conversion-backup symlink could redirect a write. | Exclusive creation and safe failure leave the original and unrelated target untouched. |
| Native dependency | The separately downloaded OCCT build archive was not authenticated by Cargo.lock. | Pin official asset sizes/SHA256, verify before safe extraction, use the verified root in CI/release/bundling; negative tests and a real arm64 archive check pass. |
| Cache | Geometry fixes did not invalidate older algorithm results; float parsing changed some serialized sketch coordinates by one ULP and rejected a valid king cache. | Add Ferrender geometry revision and round-trip float parsing, retaining strict CRC/volume/bounds checks. |
| Future-file viewing | Child-component cached bodies disappeared; scripts/edit previews/rebuild could discard read-only geometry. | Register cached owners, refuse edits before previews, preserve cache on direct rebuild, explicitly refuse CLI rebuild verification of unsupported future formats. |
| Topology tags | Primitive-face and pattern-copy families conflated distinct identities; removed text/plane faces could be guessed by position. | Preserve face/copy identity, give pattern copies distinct tags and report removed tagged faces honestly. |
| Mesh Boolean geometry | Identical, overlapping and contained boxes gave wrong union/cut/intersection volumes. | Correct coplanar ownership and containment; use full clipping context in bounded cases. Analytic regression grid covers 84 combinations. |
| Mesh Boolean failure | A watertight scan cut returned success with 77 open edges. | Reject non-manifold/open inputs or results and preserve the full original document on failure. Bound recursive splitting work with iterative traversal. |
| Mesh topology | Reduction created bowties/holes and f64-valid collapses became degenerate after f32 storage; capped cuts lost rim points and oblique intersections. | Link-condition and stored-coordinate checks, consistent shared intersections and cap stitching, translated-cut regressions. |
| Other mesh operations | Zero-strength smoothing moved vertices, mirror ignored no-weld, Loop used the wrong boundary neighbors, clustered decimation ignored boundary preservation, cavity parity failed. | Targeted fixes with topology, position, volume and option regressions. |
| Mesh performance/measure | Wide relief blur was quadratic in radius; thickness measured at the triangle centroid instead of the chosen point; empty BVH could hang. | Separable prefix-sum blur, point-specific thickness, safe empty traversal and nonnegative picking. |
| API units and limits | Smooth/Flatten strength was multiplied by document length units; large integer arguments wrapped to small values. | Dimensionless Smooth/Flatten, length-valued displacement brushes, appropriate UI defaults, checked integers for mesh/relief arguments. |
| Rendering | Another large document could retain the old GPU mesh; selection reuploaded unchanged buffers; smooth translated surfaces acquired black stippling. | Reset document caches, track indexed upload identity, release empty buffers and compare planes at a common screen sample. |
| Timeline | Empty-scene cleanup temporarily left the drag fence busy during review. | Keep immediate empty-scene completion while releasing buffers; nonempty models still wait for GPU completion. Original E2E assertions pass. |
| Local bridge | Long custom configuration paths exceeded the Unix socket pathname limit. | Use a distinct private short runtime directory when required; permissions and bind/connect isolation are tested. |

## Large-mesh measurements

Real public scans were imported, indexed, picked, saved and reopened. The safe-refusal checks also verify that a failed Boolean leaves original bodies and document unchanged.

| File | Approx. triangles | Import | Save | Reopen |
|---|---:|---:|---:|---:|
| Bearded Man | 1.0 M | 0.32 s | 0.40 s | 0.19 s |
| Cupid | 2.05 M | 0.66 s | 0.27 s | 0.19 s |
| Beethoven | 3.5 M | 1.44 s | 0.45 s | 0.32 s |

The combined real-scan stress process peaked at **3.54 GiB RSS**, including extra document copies used to verify rollback. Bearded Man and Cupid have invalid source topology for some solid operations; imported display success is not proof of a valid closed Boolean operand.

Separate synthetic bumpy-sphere tests ran sequentially in fresh processes, with an 18 GiB termination guard. Neither approached it and neither swapped.

| Triangles | Import | First BVH | Mean later pick | Save | Reopen | Peak RSS |
|---:|---:|---:|---:|---:|---:|---:|
| 5,000,000 | 2.122 s | 2.958 s | 0.0242 ms | 1.325 s | 0.315 s | 1.16 GiB |
| 12,000,000 | 7.241 s | 4.889 s | 0.0255 ms | 1.382 s | 0.701 s | 2.79 GiB |

These are import/storage/query measurements, not viewport frame-rate, sculpt, reduction or Boolean benchmarks at 12 M. First-pick BVH construction is separate from cached query latency. Synthetic files compress more predictably than arbitrary scans. A cache larger than 32 MiB is not saved, but the source mesh blob remains indexed and opens quickly.

## Remaining problems and release boundaries

1. **Complex mesh Booleans remain limited.** Closed-result checking catches known seam failures but is not a proof that every closed output has correct geometry. Some intersections now fail safely that the old implementation incorrectly claimed had succeeded. A robust-predicate solver remains future work.
2. **Topology tags are not universally stable.** Kernel-generated faces still partly use ordinals and surface matching. Radical changes to fillet networks, freeform Booleans or script output order may require selecting references again. Ambiguous pattern-copy tags written by early unreleased 0.4 builds may also require reselection.
3. **Scripts are not an OS sandbox.** Scripts run in the app's process: a native modeling call cannot be cancelled mid-call and there is no per-script memory budget or process isolation. *Narrowed after the review (branch `script-limits`):* Cancel and a new time limit (`run_script` `"timeout"`, `ferrender run --timeout`) take effect at the script's next operation, and the progress window offers to stop waiting for a modeling call and discard its result; files a script writes are staged beside their targets and moved into place only when the run succeeds, so cancellation and failure leave none of them (Undo still does not remove files a finished run wrote); the allowed folders are walked from their root without following links when a file is used, writes go through a fresh temporary and a rename, so a link or hard link planted on a target name is replaced rather than written through; `confirm`/`ask` are answered in the progress window. The sandbox bounds the script, not another program running as the same user, and native commands that read a file by path (`import_mesh`, `open`, `mesh_from_image`) check the entry immediately before the read rather than holding it open.
4. **Cache integrity is not authentication.** CRC and geometric checks detect accidental damage/staleness. They do not authenticate a deliberately forged cache and index. Use a fresh rebuild when independently validating a supported downloaded design.
5. **Large-mesh selection is incomplete.** Individual face highlighting is disabled above 200,000 triangles. Full-resolution picking and whole-body selection remain available. The plan's 5 M/12 M interactive frame-rate and reduction targets have not been established by these headless benchmarks.
6. **Script export has explicit limits.** *Narrowed after the review (branch `script-limits`):* a timeline marker anywhere is exported in full and restored by the script; failed features are exported suppressed with a note; hidden body ids that no longer exist are dropped; imported meshes and embedded images above 64 KiB are written as STL and PNG files beside the script, which `add_feature` reads back through `mesh_path`/`image_path`. Still refused: a script that would exceed 1 MB after that (very long names or many large sketches) and integers beyond Rhai's 64-bit range. An STL sidecar carries the triangles in 32-bit floats, which is also how the inline form stored them.
7. **Deferred features remain deferred.** No region subdivision, mesh-region projection, META shortcut bindings, or 16-bit relief input was added by this review. (Interactive script confirm/ask was added afterwards on branch `script-limits`.) Relief from brightness alone is not reconstructed facial geometry.
8. **Native build verification has boundaries.** Raw Cargo builds can bypass the new dependency verifier; packaging alone cannot attest how an existing binary was linked. The verified CI/bundle path should be used for distributed builds. Major-version GitHub Action tags could be hardened further with reviewed commit pins.
9. **Dependency maintenance:** the locked-version RustSec scan matched two informational unmaintained notices: [smartstring 1.0.1 via Rhai](https://rustsec.org/advisories/RUSTSEC-2026-0249.html) and [ttf-parser 0.25.1](https://rustsec.org/advisories/RUSTSEC-2026-0192.html). No matched known security vulnerability was found in that scan. This is a custom semver check of the official database, not a complete dependency certification; replacing the font parser and tracking Rhai's dependency migration remain follow-ups. Database snapshot: `550efd3d587a29b2e2c2b21b17a440da4fede999`.
10. **Platform/manual coverage:** unlocked native checks are recorded below. Physical prints, Linux desktop behavior, Intel macOS and continuous 5 M/12 M orbit frame rates were not inferred from this arm64 run. Earlier hosted macOS/Linux CI passed; the new recovery/script fix has a separate CI run. No release was made as part of this review.

## Reproduction

Use the standard verified build setup in [RELEASING.md](../RELEASING.md), then:

```sh
cargo build --locked --workspace --all-targets
cargo test --locked --workspace --all-targets
python3 scripts/test_prepare_occt.py
cargo clippy --locked --workspace --all-targets
```

The real-scan stress test reads `FERRENDER_STRESS_DIR`; absent that directory it uses a synthetic fixture. `tests/scalability.rs` is opt-in with `FERRENDER_SCALE_TRIANGLES=5000000` or `12000000`, plus `FERRENDER_SCALE_DIR`, and `--ignored`. The organic turtle pipeline regression is opt-in with `FERRENDER_REVIEW_TURTLE` pointing to the supplied seed STL. Keep large generated fixtures outside Git.

Use [the updated acceptance plan](TEST_PLAN_0.4.0.md), especially tests 19–26, for further acceptance testing. Check Help → About for the exact review commit before testing.


### Final fixture and running-app addendum

The existing private face-relief design was also checked read-only after the main report was committed. Its final `Combine1` is now safely refused: the attempted result has **173 open edges and 5 non-manifold edges**. The original file hash is unchanged; no portrait/image data was copied into this pack or the repository. This is a concrete remaining limitation of the mesh Boolean solver, and the original face-relief tutorial cannot currently be treated as passing end-to-end on this fixture. Evidence: `evidence/private-relief-check.json`.

The earlier signed app at commit `4858b6a` completed the live command-bridge sequence: turtle Smooth/Undo, cached king open, future-version editing refusal with both bodies retained, 3.5 M Beethoven import, and replacement by the turtle. The Mac was still locked, so no final mouse/keyboard or frame-rate claim is made. Native command timings in this state were slower than the isolated headless measurements (about 12.4 s for Beethoven import and 11 s for its scene query versus 2.1/1.6 s headless through MCP). These are historical locked-screen measurements. The unlocked follow-up below measured about 2.3 seconds from native Import click to the returned UI state; neither measurement establishes continuous viewport frame rate.

## Unlocked-screen follow-up — 9 October 2026

This follow-up used a separate **Ferrender UI Review** app and private settings. Native file dialogs, menus, numeric fields, viewport clicks and timeline menus were driven through macOS UI automation; the local command bridge was used read-only to record the resulting geometry. Original user designs were not overwritten. Evidence and test copies are under `~/Documents/ferr-tests/ferrender-0.4-review/evidence/unlocked/`, outside Git.

Initial checks used clean release `c7dd232`. Two defects found through the real UI were corrected in **`5c0eb8cac28c0681a324542f8e44355a3642a20c`**. The retest executable used that source plus only the in-progress review Markdown, so About reported local changes. A subsequent clean build at `7a9fd6e` was packaged and opened with the saved king. Its actual Help → About dialog showed the full commit above, “Source: Clean checkout” and “Build: release”, matching `BUILD-INFO.txt`. No model logic changed between this build and the `5c0eb8c` retest.

### New defects fixed

- **Script Run disabled:** Scripts → Samples → Spur gear showed “Nothing to apply” and a disabled Run button with valid defaults. Scripts had incorrectly entered the modeling-preview path. They now use their worker action without a geometry preview, while Enter still runs them. The regression now clicks the real **Run** and **Run again** controls, rather than calling the action directly, and also checks Enter. Actual native Run and Run again passed after rebuilding.
- **Large-mesh crash recovery failed:** Beethoven imported normally, but autosave expanded it to legacy base64 JSON and exceeded the old 64 MiB limit. Recovery now stores the compact native JSON/ZIP payload in a bounded metadata envelope with length and CRC checks. Legacy recovery still loads; discovery reads only the header; synced atomic replacement preserves the previous copy when a write fails. A successful save also clears a stale failed-recovery warning. Thirteen recovery tests pass, covering a 1.5 M-triangle mesh with an embedded image, compatibility, corruption, bounded discovery, failed replacement and temporary symlinks. The real Beethoven fixture round-tripped identically in 0.697 seconds in the focused storage test.

Recovery payloads are limited to 2 GiB and metadata to 64 KiB. Encoding remains in memory on the background writer, so a large write may finish later than the one-second scheduling interval. Older Ferrender versions cannot read the new recovery envelope. CRC is accidental-damage checking, not authentication, and no universal power-loss guarantee is claimed.

### Verified in the native app

| Workflow | Observed result |
|---|---|
| King — acceptance 19 | Opened the supplied 75 mm king, entered 90 mm in Parameters, verified one intact body, then Undo restored 75 mm. Native STL export and Save As/reopen passed. Recorded heights were 90.0018/75.0015 mm including the existing model tolerance. |
| Turtle — acceptance 20 | Opened the 150,000-triangle turtle; real Pull and Smooth surface clicks each added a stroke. Two Undos restored the original volume and zero open edges. Timeline **Roll Back to Here** restored 767,968 triangles; **Roll to End** restored 150,000. Save As/reopen used the saved geometry cache. |
| Non-mm sculpt — acceptance 25 | Switched to inches, selected Smooth, entered strength `0.25`, and clicked the shell. The stored scalar remained `0.25`, with radius `0.315 in` (8.001 mm). Undo restored the model and units. Pull displayed length-valued radius/depth. Flatten itself remains covered by automated tests rather than a new native click. |
| Beethoven — acceptances 5, 7, 21 | Real STL import showed 3,499,998 triangles and a watertight mesh in approximately 2.3 seconds from Import click to the returned UI state. Face/body selection, hide/show, a 500 mm Move and Undo, Save As and reopen passed. Replacing it with the turtle and then New cleared the old GPU geometry. Individual face highlighting remains intentionally limited for large meshes. |
| Script history — acceptance 23 | Ran Spur gear with 18 teeth, added a 20 mm X Move, then used **Edit Inputs and Re-run** for 30 teeth. The same body ID and downstream Move survived; bounds were centered at X=20 mm. Suppress removed the generated body (the dependent Move correctly reported its missing source); Unsuppress restored it without errors. Save/reopen retained the script, 30-tooth output and Move. |
| Future cache — acceptance 24 | Opened the future-version fixture with two cached bodies. Move, Box and Sculpt were refused before editing previews. Clicking the repaired script Run button gave a read-only explanation and retained both bodies. Native Export STL succeeded with 3,546 triangles. |
| Real crash recovery — acceptance 26 | Imported Beethoven again on the repaired build. A ~45.1 MB recovery copy had the expected payload length and CRC. Force-killed only the isolated test instance, restarted, and clicked **Recover** in the actual prompt. It restored an unsaved body with exactly 3,499,998 triangles, zero open edges and volume 2,697,706,227.55 mm³. Save As succeeded with no recovery warning. |

Geometry evidence includes `king-90mm.json`, `king-undo.json`, `turtle-undo.json`, `turtle-rollback.json`, `turtle-inch-smooth.json`, `beethoven-moved.json`, `gear-rerun.json`, `beethoven-recovery-envelope.json` and `beethoven-recovered.json`. Native exports and saved test copies are alongside them.

### Saved-file and delivery verification

The four copies saved through native dialogs—king, turtle, gear and recovered Beethoven—each passed a fresh `check --rebuild` using the final clean packaged executable. The native king STL export reimported with **108,946 triangles and zero open edges**; the future-cache STL export reimported with **3,546 triangles and zero open edges**. Evidence: `evidence/unlocked/final-file-validation.json` and its log.

The packaged app's ad-hoc signature verified in both the output pack and the Documents test pack. All **117 recorded file checksums** matched at delivery. The app bundle is excluded from that file manifest and verified by its signature instead. These checks confirm the delivered artifacts, not a physical print or every possible geometric property.

### Verification boundaries

The full workspace suite after these fixes passed **417 tests, zero failures, five intentionally ignored**. The release build completed without warnings. The fixes and report were pushed to `0.4-dev`. At the status check above, both the [code-fix CI](https://github.com/base698/ferrender/actions/runs/37916523681) and [delivered-build CI](https://github.com/base698/ferrender/actions/runs/37917098628) were still running; the delivered-build macOS and Ubuntu jobs were building all workspace targets. Earlier `c7dd232` [CI passed](https://github.com/base698/ferrender/actions/runs/37878285251). The local full-suite log is `evidence/unlocked/full-suite.log`.

Discrete Front/Top/isometric controls, Fit, selection and numeric movement were verified. The automation tool's continuous drag gestures did not produce a reliably observable orbit, so **coarse-LOD interaction, continuous orbit frame rate and drag feel remain human checks**. This is not evidence of a Ferrender orbit defect. One native unsaved-changes confirmation blocked accessibility automation until the user clicked Cancel; no app crash was inferred from that tool limitation. Subsequent tests saved their copies before opening another file.

Safe mesh-Boolean refusal, long-script stale-result rejection and script log/export retain their automated and bridge coverage; this follow-up does not claim an additional native mouse-driven run of every item. The private face-relief Boolean limitation above remains unresolved. Main was not merged and no release was published.
