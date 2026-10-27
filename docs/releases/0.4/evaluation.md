# Evaluating Ferrender 0.4.0 through a substantial modeling project

**Original run and follow-up recorded 9 October 2026.** The original evaluation describes a private residential reconstruction exercise using Ferrender **0.4.0 at `d546ca0dbb75e6df54f880d83ca994e76be1a686`**, on macOS ARM64 with OpenCascade 8.0.1 rev2. It is a workflow evaluation of that build, not a claim that every later commit has been tested. The original write-up was made at `1c77a50`. The follow-up below tests the performance fixes at `f70e64cd0610a0b1a008a5cb1566a6b91bf147a0`.

Ferrender successfully built, revised, saved and reopened a substantial editable assembly. Its existing solid-modeling tools were sufficient for the requested shapes. The original main weakness was the cost of working on a long history. The follow-up confirms that the reported display-only rebuild problem is fixed for the exercised operations and that full rebuilding is substantially faster. This model still exceeds the geometry-cache budget, but saving now explains why. Other assembly and verification limits remain documented below.

This report supplements [the independent review](review.md), [the acceptance run](acceptance.md) and [the complete test plan](test-plan.md). It does not supersede their security findings or mark their remaining checks as passed.

## Performance-fix follow-up — `f70e64c`

The same saved 878-feature, 638-body assembly was retested using a newly compiled release executable at `f70e64cd0610a0b1a008a5cb1566a6b91bf147a0`, with the verified pinned OpenCascade dependency. The executable reports local changes because the review Markdown was still uncommitted; the checkout had no product-code modifications beyond that commit. A separate native app, isolated local settings and a copy of the model protected the original.

**Result:** the slow visibility/activation/rename behavior is resolved in the exercised native-command workflows. A genuine geometry edit still rebuilds, now in about five seconds for this assembly. The cache explanation is also fixed; the 32 MiB cache limit itself remains.

| Operation | Original observation | Follow-up observation |
|---|---|---|
| Independent rebuild of the final assembly | 21.475 s | 4.945 s, approximately 4.3× faster |
| Native open of the final assembly | 23.48 s API call; 21.804 s geometry rebuild | 6.556 s API call; 4.973 s geometry rebuild |
| Native reopen after saving the test copy | Previously rebuilt without a cache | 6.505 s API call; 4.895 s geometry rebuild |
| Show furniture | 82.679 s on the earlier 681-feature assembly | 0.061 s on the final assembly; hiding took 0.123 s |
| Activate already-active Root | 83.800 s on the earlier assembly | Three calls: 0.026, 0.129 and 0.124 s |
| Rename a primitive | Not separately timed in the original native run | 0.042 s |
| Metadata Undo/Redo | Not separately timed in the original native run | 0.029–0.182 s in the exercised steps |
| Small component move and Undo | Related final-model headless moves took about 21–23 s | Native move 4.973 s; Undo 4.970 s |
| Show/hide upper-floor component | Not separately timed in the original native run | 0.040 / 0.036 s |
| Requested cache on save | No cache, without an explicit skip reason | No cache, with `cache_skipped: "the geometry is over the 32 MiB cache limit"` |

Times are individual end-to-end local command observations, not frame-rate measurements or statistical guarantees. The older visibility/activation numbers came from an earlier assembly and session; only the final-assembly rebuild/open rows compare the same model. Native command timings include bridge and application work.

Validation completed:

- The focused `rebuild_journal` suite passed **5 tests, zero failures**. It covers failed-feature rollback, rebuild scaling, metadata edits and Undo/Redo retaining built bodies, shared-tag edge fillets and cache-skip reporting.
- The live application passed furniture hide/show, activation, metadata Undo/Redo, rename/restore, nested parent/child visibility and upper-floor visibility checks. Showing a parent did not incorrectly reveal its individually hidden child.
- Moving one component changed only that component's bounds; its volume and all other bodies were retained. Undo restored the original assembly. Save/reopen retained the expected geometry and Root activation.
- The new build's body IDs, ownership, bounds, volumes and open-edge counts matched the original recorded model within the comparison tolerances stated below. All 638 bodies remained exact, with no feature errors. The source model's SHA-256 remained unchanged.
- An independent rebuild of the saved test copy passed with 878 features and 638 bodies, exit 0.
- In the actual GPU window, clicking Top switched to the plan view and clicking the table selected its intended face. An attempted continuous drag produced no observable view change, so continuous orbit performance remains unverified. A later browser-eye click was blocked by the automation tool's user-activity guard; it is not counted as a UI pass. Automated controls then yielded to the user, with the model left open in the latest app.

The earlier findings below are retained as historical evidence. The expensive metadata-rebuild finding is **resolved for the tested cases**, and the silent cache fallback/API wording is **resolved**. Large-cache support, body naming, reparenting and the other listed workflow limits are not marked fixed. A full standard E2E/security suite, hosted CI, cross-platform packaging, peak memory and sustained frame-rate testing were not rerun in this follow-up. No release is approved or published by these results.

Private follow-up evidence includes `results.json`, `baseline-native.json`, `final-native.json`, `BUILD-INFO.txt`, `build-and-focused-tests.log`, `fresh-rebuild.txt` and the native command log in the house collection's `performance-retest/` directory. Runtime settings, sockets and cache keys remain local.

## What the exercise asked of Ferrender

The task was to reconstruct a home from an older handmade reference model, photographs and a few owner-measured dimensions. The result needed real editable components for furniture and appliances, architectural openings, stairs, cabinetry, a bar, crown molding and baseboards. Further photographs introduced substantial layout corrections and part of an upper floor.

The source STL was not used. An existing STEP file was read to extract reference wall coordinates, but none of its bodies were imported into the new design. All new geometry was created through Ferrender's commands: native sketches, primitives, extrusions, sweeps, fillets, cuts and component placements. This exercised actual feature construction and rebuilding, rather than merely displaying an imported finished mesh.

The model, exact residential dimensions, photographs and detailed floor plan remain outside Git in the private archive. This document reports product behavior and aggregate measurements only.

## How the process went

### 1. Establishing the reference and building the first assembly

The older outline provided an initial footprint. Photographs supplied furniture, openings and trim details. Separate components made it practical to build the structure, appliances, furniture and decorative elements independently. Repeated cabinet and furniture parts were authored through small command-building helpers.

The initial construction ran through the MCP bridge into a separate native Ferrender application with isolated settings. The commands were sent directly to Ferrender; an external CAD kernel did not generate the result. Python orchestrated the calls and recorded their arguments, durations and errors. These helpers are not a test of Ferrender's Rhai scripting engine.

The first assembled checkpoint contained 681 features and 484 exact bodies. It saved and rebuilt successfully, but its spatial interpretation still contained errors. A valid CAD model is not necessarily a correct reconstruction of the reference.

### 2. Correcting the interpretation

Owner feedback clarified the room arrangement, an opening that had been mistaken for a door, the openness between two rooms and the stair location. Additional photographs showed the hall more clearly and supplied an upstairs landing and bedroom.

The revisions removed an invented divider and an incorrectly interpreted wall mass, moved the stair and its floor/ceiling openings, relocated fixtures and furniture, changed a table profile and rerouted trim. The upper-floor addition included sloping ceiling panels, a balustrade, storage and furniture as components.

Those corrections were authoring errors and incomplete reference interpretation, not evidence of a failed Boolean or corrupt file. Ferrender's component structure helped make the corrections without replacing the whole design. A better reconstruction process would confirm the floor plan and stair direction in a simple early layout before investing in detailed furniture and molding.

### 3. Working around rebuild cost and interrupted desktop access

As the history grew, repeated edits became expensive. Finished independent components were temporarily suppressed while another component was being built. Hiding them did not avoid their rebuild cost. Activating a child of a suppressed parent was correctly refused; the helper was changed to enable the full ancestor chain before activating it.

When the Mac was locked, work continued through Ferrender's headless MCP interface using saved checkpoints. Those operations used the same modeling implementation, but they were not mouse-driven UI tests. All temporary suppression was removed before delivery. Hidden ceilings and other inspection aids remain present in the design.

After desktop access returned, the revised file was opened in the isolated native application. Its geometry matched the headless result and the GPU viewport displayed the revised assembly. The UI tool then detected user interaction and rejected two attempted Top-view actions. Automation yielded control; those attempts are not counted as completed navigation tests.

### 4. Validation and delivery

Validation combined feature-error checks, reported solid volumes, tessellation edge checks, actual face-to-face measurements, an independent saved-file rebuild, native reopening and visual inspection. Previews were inspected during authoring; this caught a stray crossbar in an opening and prompted a furniture-placement correction.

The native design, previews, command log, measurements and notes were saved outside the repository. Original reference photographs were preserved, and archive copies were verified by SHA-256. No original reference model was overwritten. Product code was not changed during this modeling exercise.

## What worked well

- **The modeling vocabulary was sufficient.** Sketch/extrude handled the building footprint; primitives and fillets handled furnishings; swept profiles made baseboards and crown molding; component placement handled rearrangement. No new geometry operation was required to complete the modeled scope.
- **Components made revision practical.** Furniture, fixtures and upper-floor elements retained their own histories and placements. The final document remained editable instead of becoming one flattened imported body.
- **Native and headless execution agreed.** The final native reopen had the same body identifiers, bounds and volumes as the recorded headless result within the stated tolerances.
- **Errors were actionable in the exercised cases.** An excessive fillet, a numerically negligible sweep span, activation under a suppressed parent and a malformed delete command were refused. Correcting the request allowed work to continue.
- **Persistence held up for this assembly.** The final save reopened and independently rebuilt without feature errors. This is useful large-document evidence, though it is not a crash-recovery or power-loss test.

An editable history should not be confused with a fully constrained architectural model. Some geometry was placed with fixed coordinates. The API explicitly says expressions in `add_geometry` are evaluated at placement time; persistent size relationships need constraints or expression-bearing feature fields. Two unused provisional parameters were removed rather than implying that they controlled geometry they did not drive.

## Measured results

| Check | Recorded result |
|---|---|
| Final history | 878 features; no reported feature errors or suppressed features |
| Final geometry | 638 exact bodies; all report positive volume and zero open tessellation edges |
| Headless display tessellation | 936,144 triangles; this is a renderer representation, not a count of CAD faces |
| Owner-supplied dimensions | All three face-to-face measurements matched their supplied values |
| Stair/opening consistency | Upper-floor cut volume matched the relocated stairwell footprint; the stair reached the modeled upper-floor substrate level |
| Independent `ferrender check --rebuild` | Passed, exit 0; 21,475 ms |
| Native reopen | Passed; 21,804 ms reported geometry rebuild and 23.48 s for the open API call |
| Native/headless comparison | All body IDs matched; bounds within 0.001 document inches and volumes within `max(0.001 cubic inches, 1e-7 × absolute reference volume)` |
| Saved cache | `cached_bodies: 0`, `container: false`; native reopen reported `from_cache: false`, `geometry_trust: rebuilt` |
| Native visual inspection | Revised assembly visibly present in the GPU viewport after opening |

These checks do not prove collision-free assembly, architectural accuracy, universal topology stability or correct behavior for all inputs. Tessellation triangle counts can vary between rebuilds; comparison therefore used body identity, bounds and volume rather than demanding identical triangle counts.

## Problems exposed by the original workflow

### Originally high priority: inexpensive edits rebuild too much

**Follow-up status:** resolved for the metadata operations tested at `f70e64c`; genuine geometry edits still rebuild, with the faster times recorded above.

On the earlier 681-feature model, a native API call to show a furniture component took **82.68 seconds**. Activating the already-active Root took **83.80 seconds**. On the final model, six successful headless edits—including component moves, deleting one primitive and deleting unused parameters—each took roughly **21–23 seconds**.

These observations come from different operations and execution states. They are not controlled scaling benchmarks, percentile measurements or evidence that every human UI click takes the same time. The final native open was much faster than the earlier live edit calls; those timings must not be conflated.

Source inspection supports the diagnosis: `activate_component` and `set_visible` pass through `Session::edit`, which rebuilds the document. Native stack samples also showed repeated exact-solid copying in `Document::rebuild_with` and fillet tessellation. This identifies expensive work on the path, not a measured percentage attributable to each function. Relevant code is in [api.rs](../../../crates/fr-core/src/api.rs), [doc.rs](../../../crates/fr-core/src/doc.rs) and [components_ui.rs](../../../crates/ferrender/src/components_ui.rs).

**Recommended next work:** make selection/activation, visibility and naming avoid geometry recomputation where safe; skip actual no-ops; retain unaffected geometry across edits; investigate repeated solid copying before adding more modeling features. Preserve undo behavior, visibility, dependent-feature invalidation and topology references in the optimization tests. Suppressing parts of the design should not be the normal way to make routine authoring tolerable.

### Originally high priority: large exact assemblies silently lose cached opening

**Follow-up status:** the missing explanation and API wording are fixed at `f70e64c`. The cache size limit and resulting uncached reopen remain.

Saving requested `cache: true`, but no cache was written. The tested implementation's [cache capture](../../../crates/fr-core/src/cache.rs) has a **32 MiB total geometry-blob limit** and returns no cache when that limit is exceeded. The other explicit skip condition is modeled threads; this model has none. That supports the size-limit explanation, but the save response itself does not report a reason or the attempted cache byte count.

The design saved successfully as plain JSON and rebuilt on every reopen. This is a performance and explanation problem, not observed data loss or an authentication failure. The API's wording that the flag “forces” caching does not describe this exception. No cache-versus-rebuild equivalence test passed here because there was no saved cache to compare.

**Recommended next work:** return a reason when cache capture is skipped and show the consequence to the user. Evaluate a bounded cache strategy that can accommodate large exact assemblies, with explicit memory/disk budgets and the existing authentication and integrity checks retained.

### Medium priority: organization becomes harder as the assembly grows

Feature names were descriptive, while many body entries retained generic `Body` labels. Component reparenting is not exposed by the documented API, so later reorganizing an appliance under a different parent was awkward. This made the up-front hierarchy more consequential than it needed to be.

**Recommended next work:** provide predictable body naming and safe component reparenting. Reparenting should preserve world placement, references, ownership and save/reopen behavior, with clear refusal for unsupported dependencies.

### Other limits and useful additions

| Observation | Effect and possible improvement |
|---|---|
| Box primitive dimensions stop at 10,000 mm | A building-scale floor exceeded that range. Sketch/extrude worked. Explain the limit and consider consistent model-scale limits across creation tools. |
| No material/color assignment in the documented modeling API | Geometry conveyed form, but gray surfaces made large assemblies harder to read and could not reproduce finishes. Per-component display colors would be useful even without photorealistic materials. |
| STEP export is flat | Retain `.ferr` for component hierarchy and history. This is an existing export limitation, not a new successful house-export test. |
| Many repeated furnishing parts were authored individually | Reusable component definitions or instances could reduce authoring effort and may help document size. Performance benefits need measurement rather than assumption. |
| Early spatial uncertainty caused substantial rework | Named inspection views, simple annotation and clearer measured-versus-estimated dimensions would help review a draft before detailing. Better authoring discipline is also necessary. |

## Original coverage boundaries

- The supplied photos did not define a complete measured building. Unseen upper rooms and an external roof were not invented. Estimated details remain subject to owner review; no structural or code-compliance conclusion is made.
- This was primarily an exact-solid and component workload. It did not rerun the mesh Boolean, sculpting, large-mesh import, Rhai sandbox or security suites.
- It did not rerun the full standard E2E suite, Linux/Intel jobs, packaging or hosted CI. Their previous results remain tied to the revisions recorded in the independent review and acceptance documents.
- Continuous orbit responsiveness, frame rate, peak memory, automated picking and the final upstairs visibility-toggle interaction were not measured or completed here.
- Successful save/open is not evidence of crash recovery, cancellation responsiveness or power-loss durability for this model.
- No new house STL/STEP export, physical print or geometry-cache equivalence result is claimed.

No new security defect was demonstrated by this workload. That is a coverage statement, not a security approval. Reference photos and residential details stayed outside Git; runtime configuration and cache keys stayed local instead of being copied into the shared archive.

## Follow-up acceptance checks

1. On a sanitized assembly with a comparable feature/body count, measure cold open, warm open, component hide/show, activating an already-active component, renaming and moving one component. Record the build, machine, timing distribution and peak memory. Confirm which operations actually invoke geometry rebuilding.
2. After performance changes, prove that metadata-only actions do not rebuild solids, while genuine geometry edits still invalidate dependent features correctly. Compare geometry before/after, Undo/Redo and save/reopen.
3. Exercise cache capture just below and above the budget. Verify a clear skip reason, bounded resources and successful rebuilding; if the cache strategy changes, retest local authentication, foreign-file fallback and corrupt-cache handling.
4. Let a human orbit, zoom, pick small furniture faces, switch floor visibility and edit one dimension in the large assembly. Record responsiveness and any drawing artifacts separately from geometry correctness.
5. Check body naming and any future reparenting with nested components, downstream references and export ownership.
6. Continue the existing release acceptance plan on the exact proposed release build. This project adds realistic assembly evidence; it does not independently approve shipping 0.4.0.

## Evidence record

The private archive's house end-to-end collection contains the revised `.ferr`, inspected previews and detailed notes. Key records are `commands.jsonl`, `final-photo-scene.json`, `final-photo-measurements.json`, `fresh-rebuild-revised.txt` and `native-photo-open-scene.json`; earlier native performance samples are retained with the first assembly's evidence. Archive indexes and SHA-256 manifests identify the copies. See [AGENTS.md](../../../AGENTS.md) for storage and privacy rules. The model and photographs must not be added to the public repository to make these links portable.

The initial write-up added documentation only. The subsequent performance retest used the separate `f70e64c` implementation and ran the focused tests and actual-model checks recorded above; this evaluation did not itself change product code.
