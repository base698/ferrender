# Ferrender 0.4 independent review

Reviewed the implementation at `f38e1f7` on `0.4-dev` on 8–9 October 2026. The review used an isolated clone, three scoped review agents and a separate native app/configuration. The fixes, regressions and documentation are intended for the same 0.4 development branch. This review does not publish or approve a release by itself.

The baseline had significant correctness and security defects despite its passing feature tests. Reproductions included successful script filesystem escapes, document identity loss after failed scripts, wrong Boolean volumes, decimation holes, stale GPU geometry and permanently suppressed script results. The reviewed candidate fixes these cases and adds explicit refusal where a reliable result is unavailable. No finite suite proves the absence of other bugs.

## Evidence and coverage

- **All-target build: passed.**
- **Full standard suite: 410 passed, zero failed, five ignored** — 172 app tests (including native E2E/rendering) and 238 core tests. The ignored cases are three optional visual demonstrations and two fixture-driven stress tests; the turtle and 5 M/12 M opt-in tests were run separately and passed.
- **Dependency-verifier tests: four passed.** The real Apple Silicon archive also passed size/hash/extraction verification.
- **Doc-test command: passed; the crate currently contains no runnable doc tests.**
- **Clippy: completed without errors**, with style, complexity and dead-code warnings still present. This review did not reformat the entire project to silence them.
- Logs and detailed model/compatibility measurements are supplied with the test pack. No physical print was made during this review.

Test machine: Apple M3 Max, 48 GiB RAM, macOS 26.2 (25C56), arm64. Native rendering tests use the local Metal adapter. The first unprivileged run could not create sockets or access a graphics adapter; those environmental failures were rerun with the required local access. A subsequent real timeline regression was fixed and its original assertions retained.

- **Direct native operation:** opened the isolated application, used Search, created a sketch/circle, finished the sketch, selected its profile, extruded it, fitted the view and saved through the macOS Save dialog. The classic king was also constructed through the running app's command bridge, not an external CAD kernel. The final models were rebuilt and verified through Ferrender's headless command interface after fixes.
- **Screen-access limit:** the Mac locked during the review. Further direct mouse/keyboard checks require the user to unlock it. Final offscreen native UI tests still ran, but they are not a substitute for judging final on-screen interaction. The updated manual plan calls out the remaining king/turtle, large-mesh orbit/selection and Scripts checks.
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
3. **Scripts are not an OS sandbox.** Native modeling calls cannot be cancelled mid-call, and there is no global per-script memory budget/process isolation. Static path checks do not defeat a separate same-user process racing filesystem entries or introducing hard links. Completed file exports are not rolled back by document Undo/cancellation. Confirm/ask still use automatic headless answers in the GUI.
4. **Cache integrity is not authentication.** CRC and geometric checks detect accidental damage/staleness. They do not authenticate a deliberately forged cache and index. Use a fresh rebuild when independently validating a supported downloaded design.
5. **Large-mesh selection is incomplete.** Individual face highlighting is disabled above 200,000 triangles. Full-resolution picking and whole-body selection remain available. The plan's 5 M/12 M interactive frame-rate and reduction targets have not been established by these headless benchmarks.
6. **Script export has explicit limits.** Move the rollback marker to the end and resolve/suppress errors first. Oversized embedded payloads and hidden bodies unavailable in the final build are refused. Use a `.ferr` file or a script that imports an external mesh for large designs.
7. **Deferred features remain deferred.** No region subdivision, mesh-region projection, META shortcut bindings, interactive script confirm/ask, or 16-bit relief input was added by this review. Relief from brightness alone is not reconstructed facial geometry.
8. **Native build verification has boundaries.** Raw Cargo builds can bypass the new dependency verifier; packaging alone cannot attest how an existing binary was linked. The verified CI/bundle path should be used for distributed builds. Major-version GitHub Action tags could be hardened further with reviewed commit pins.
9. **Dependency maintenance:** the locked-version RustSec scan matched two informational unmaintained notices: [smartstring 1.0.1 via Rhai](https://rustsec.org/advisories/RUSTSEC-2026-0249.html) and [ttf-parser 0.25.1](https://rustsec.org/advisories/RUSTSEC-2026-0192.html). No matched known security vulnerability was found in that scan. This is a custom semver check of the official database, not a complete dependency certification; replacing the font parser and tracking Rhai's dependency migration remain follow-ups. Database snapshot: `550efd3d587a29b2e2c2b21b17a440da4fede999`.
10. **Platform/manual coverage:** final native mouse/keyboard acceptance still needs an unlocked Mac. Physical prints, Linux desktop behavior and Intel macOS were not inferred from this arm64 run. Hosted CI remains a release gate: inspect the workflow results for the reviewed branch before merging or tagging. No release was made as part of this review.

## Reproduction

Use the standard verified build setup in [RELEASING.md](../RELEASING.md), then:

```sh
cargo build --locked --workspace --all-targets
cargo test --locked --workspace --all-targets
python3 scripts/test_prepare_occt.py
cargo clippy --locked --workspace --all-targets
```

The real-scan stress test reads `FERRENDER_STRESS_DIR`; absent that directory it uses a synthetic fixture. `tests/scalability.rs` is opt-in with `FERRENDER_SCALE_TRIANGLES=5000000` or `12000000`, plus `FERRENDER_SCALE_DIR`, and `--ignored`. The organic turtle pipeline regression is opt-in with `FERRENDER_REVIEW_TURTLE` pointing to the supplied seed STL. Keep large generated fixtures outside Git.

Use [the updated acceptance plan](TEST_PLAN_0.4.0.md), especially tests 19–25, when the screen is available. Check Help → About for the exact review commit before testing.
