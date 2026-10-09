# Scripts in 0.5: what 0.4 left open

Status: **plan for 0.5, proposed; nothing here is committed.** Written 2026-10-09 against `0.4-dev`. It collects everything about scripts that 0.4.0 defers, limits or leaves awkward, so 0.5 planning starts from one list. Each item says what is true in 0.4.0 and what closing it would take. Nothing here is committed.

Sources: [the 0.4 review](../0.4/review.md) findings 3, 6, 7 and 9, [the 0.4 plan](../0.4/plan.md) and its "Not in 0.4" list, and a survey of the app for work that belongs in scripts.

## 1. Isolation: the one that changes the architecture

In 0.4.0 a script runs inside the app's own process, on a worker thread.

- A modeling call that has started cannot be interrupted. Cancel and the time limit act at the script's next operation. The progress window offers Stop waiting, which abandons the result but leaves the call running to its end.
- There is no memory budget per script. The limits that exist are Rhai's (50 M operations, 1 MB strings, 1 M arrays) and the mesh work budgets.
- A script that crashes the kernel crashes the app.

**What closes it:** run each script in a child `ferrender` process that owns a copy of the design and talks to the app over the existing command protocol. Cancel becomes a process kill, a memory cap becomes an OS limit, and a kernel crash costs one run. The pieces exist already: `Session::fork`, the headless `ferrender run`, and the staged file publication that keeps a killed run from leaving half-written output. The cost is copying the design across the boundary, which matters for multi-million-triangle meshes, and a second code path for the GUI's progress and question dialogs.

This is the largest script item and the others in this section depend on it. Decide it first.

## 2. Files

- **Native readers take a path, not an open file.** `import_mesh`, `open` and `mesh_from_image` check the directory entry immediately before reading. Another program running as the same user can swap the entry in between. Closing it means passing an open file handle into those readers. The script's own writes do not have this gap.
- **Several output files are not one transaction.** Each file is staged and renamed, and an ordinary failure restores the earlier files. A crash or a full disk in the middle of publication can leave some new and some old, with named recovery files beside them. A journal would close it; whether that is worth it depends on how often scripts write sets of files that must agree.
- **Undo does not remove files a finished run wrote.** This is deliberate, but the app does not say so at the moment of Undo. A line in the toast would do.
- **Allowed folders are edited in `config.toml`.** The plan's "one-click Allow this folder, remembered" from the refusal message was not built.

## 3. Authoring

- **`META.shortcut` is read and ignored.** No key binding, and no `[scripts] shortcuts` in the settings.
- **Repeated modeling calls need a unique `output_key`.** A loop that makes ten boxes fails with guidance unless each call names its output. That is correct, because it is what keeps later features attached across re-runs, but it is the first wall a new script author hits. Options: derive a key from the loop's call site plus an iteration counter when the script is not being re-run, or lint for it in `script_meta` so the inputs dialog can warn before the run.
- **`inputs.width` and `inputs.expr.width`.** A length input arrives twice, as a number in millimetres and as the typed expression. Passing the number loses the link to the parameter. The plan said to revisit this after the samples were written; the samples all had to know the difference.
- **`selection()` has no faces.** It reports selected bodies and sketches. A script cannot start from "the face I clicked". The inputs dialog can prefill a declared `face` input, but an undeclared selection is invisible.
- **No `import` between scripts**, so shared helpers are copied into each file.
- **No editor in the app.** New Script opens the system editor. Errors are reported with a line number in a toast and the script log.
- **The script log is not kept** between sessions.
- **The GUI has no time limit setting.** `run_script` and `ferrender run` take one; a run from the Scripts menu does not.

## 4. Export Timeline as Script

- A script over 1 MB after large meshes and images have gone to sidecar files is refused. Very long names or many large sketches can reach that.
- Integers beyond Rhai's signed 64-bit range are refused.
- The export is a replay of stored features through `add_feature`, not readable modeling code. It rebuilds the design and exposes parameters as inputs, but nobody would edit it by hand. A second mode that emits `extrude(...)`, `fillet_edges(...)` and the rest would be the honest "record a macro".

## 5. The Assistant

The built-in Assistant has one tool, which sends a batch of commands. It cannot call `script_meta` or `run_script`, so it cannot write a generator, save it and run it, although an outside MCP client can. Giving it those two tools is small and makes "make this parametric" a thing it can do.

## 6. Work that should move into scripts

Found by reading the app for workflows written in Rust that are really sequences of commands.

- **Gallery models as generators.** The sword, the coffee holders, the toothbrush holder and the bearings exist as static `.ferr` files. Each is a script with three or four inputs. Export Timeline as Script gets most of the way.
- **`export_stl` with `"union": true`.** About forty lines in `api.rs` that combine every exact body and write one file. A script can do the same and also what the option refuses: meshes, a chosen subset, STEP as well. Ship `export-union.rhai`, keep the option one more release, then retire it.
- **"Make printable" for a scan.** Repair, fill holes, thicken or hollow, measure, export is four dialogs today and the same order every time. A sample with a wall thickness and a hole size would cover it.
- **The bishop tutorial.** Its recipe is 45 positioned points in prose. A `bishop.rhai` with a height input is the checkable form of it.
- **A sweep sample.** Sweep and its `spans` arrived after the samples were written. A bent handle or a pipe run with a clip at each end would show both.

Not candidates: anything that resolves a pick by tag, the sketch solver, the Boolean and mesh kernels, the file format and cache. Scripts compose those and cannot replace them.

## 7. Dependencies

Rhai 1.26.1 still requires `smartstring`, which carries an informational unmaintained notice (RUSTSEC-2026-0249) recorded as an exception in `deny.toml`. Nothing to do until Rhai migrates; check at the start of 0.5.

## 8. Explicitly not proposed

From the plan's "Not in 0.4" list, still not proposed: a live macro recorder, scripts that run while the user models, custom feature types that re-evaluate on rebuild, and network access from scripts. Each widens what a script can do to a design without the user starting it, and section 1 should be settled before any of them is reopened.

## Suggested order

1. Decide section 1. If scripts move out of process, do it first, because sections 2 and 3 are easier on the far side of it.
2. `META.shortcut`, faces in `selection()`, the Allow this folder button and the Assistant tools. Each is small and independent.
3. The samples in section 6, starting with the sweep sample and "make printable".
4. Readable timeline export.
