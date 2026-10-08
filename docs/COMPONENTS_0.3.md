# Components in Ferrender 0.3

Status: **implemented in the 0.3.0-dev test candidate; local automated verification passed; manual acceptance pending. Not released.** Target: 0.3.0, alongside [construction planes](CONSTRUCTION_PLANES_0.3.md). The specification below defines the intended behavior; it is not a record of passed tests. Use the consolidated [0.3.0 manual test plan](TEST_PLAN_0.3.0.md) to report acceptance results. A public release waits for user verification.

A component is a named, nestable container for sketches, bodies, construction planes and the features that make them. A document starts with one root component and gains more with **New Component**. Features go into the *active* component. Operations such as Join and Cut only touch bodies in the component they run in, so parts built side by side stay separate until you combine them on purpose. A component can be moved as a whole, and hidden as a whole.

Fusion's model is the reference: one timeline for the whole document, a browser tree of components, each feature owned by exactly one component. Joints, component instances (one definition placed many times) and external (linked) components are **not** in 0.3.0; see [Not in 0.3.0](#not-in-030).

## What the user sees

### Browser

The browser becomes a tree. The document node at the top is the root component. Each component shows:

- a radio-style **activate** control (the active component is bold, and its name is shown in the status bar);
- an eye that hides the whole subtree;
- `Origin` (the three origin planes, in that component's own frame; see [Placement](#placement));
- `Construction` (planes, from the planes feature) and `Sketches`, `Bodies` as today, but listing only what the component owns;
- child components.

Right-click on a component: Activate, Rename, New Component (child), Move, Show/Hide, Delete. Right-click on the document node: New Component.

Clicking a body in the browser selects it as today. Bodies are named per component, so two components can each have a `Body1`; the status bar shows `Bracket › Body1`.

### Toolbar and menus

- **Model → New Component** (also a toolbar button next to New Sketch). Creates `ComponentN` as a child of the active component and activates it. Takes an optional name via the rename box.
- **Model → Activate Root** (and the browser control) to get back out.
- **Move** with a component selected (clicked in the browser) moves the component's placement rather than adding a Transform feature; see [Placement](#placement).

### Timeline

The timeline stays one row. A `New Component` chip appears where the component was created (icon: `icon::TREE_STRUCTURE` or similar). Hovering any chip adds `in Bracket` to its tooltip. A short coloured underline per component helps tell runs of features apart; the colours come from a small fixed palette cycled by component index and are not user-editable in 0.3.0.

Rolling back before a component's chip removes that component, its bodies and its children from the model. New features go in at the marker as today, owned by the active component; if the active component does not exist at the marker, the root becomes active and a toast says so.

### What changes about Join and Cut

Today an extrude set to Join merges into **every** body whose bounds it overlaps, document-wide (`Document::merge` in `crates/fr-core/src/doc.rs`). With components, Join, Cut and Intersect only consider bodies in the same component as the feature. Combine is the explicit way to reach across: its target and tools may be in different components, and the result lives in the target's component (tools not kept are removed from theirs). Pattern copies land in the source feature's component. Hole, Thread, Fillet, Chamfer, Shell and Text act on the body they name, wherever it is.

Through All measures its reach against bodies in the same component only.

## Data model

Everything below is in `crates/fr-core`.

### A component is a feature

```rust
// doc.rs
pub struct Component {
    /// The containing component; `None` only for the root, which is never stored as a feature.
    pub parent: Option<Id>,
    /// Rigid placement of everything in the component, applied after the build. Identity by default.
    #[serde(default)]
    pub placement: Placement,
    #[serde(default = "yes")]
    pub visible: bool,
}

pub struct Placement {
    pub translate: [Value; 3],
    /// Degrees about the parent's X, then Y, then Z, like `Transform`.
    pub rotate: [Value; 3],
}

pub enum FeatureKind {
    // ...existing...
    Component(Component),
}
```

Making the component a `FeatureKind` rather than a separate list means it gets an id from `next_id`, a timeline position, rename/suppress/delete from the existing `feature_menu`, undo through the existing snapshot of `Document`, and roll-back for free. Suppressing a component suppresses everything it owns (see rebuild).

The root is implicit: id `0`, never serialized, cannot be deleted or moved. `ORIGIN` is already id 0 for sketch points, which is a different id space; feature ids start at 1 (`validation::document` already enforces `f.id != 0`).

### Every feature has an owner

```rust
pub struct Feature {
    pub id: Id,
    pub name: String,
    pub suppressed: bool,
    /// The component this feature belongs to; 0 is the root.
    #[serde(default, skip_serializing_if = "is_root")]
    pub owner: Id,
    pub kind: FeatureKind,
}
```

`Document::add_feature` sets `owner = self.active_component`. A component feature's own `owner` is its parent (so `Component.parent` is redundant with `Feature.owner`; drop `parent` from the struct and use `owner`). Old files deserialize with every owner at 0 and behave exactly as before.

### Which component is active

```rust
pub struct Document {
    // ...
    #[serde(default, skip_serializing_if = "is_root")]
    pub active_component: Id,
}
```

It lives in the document rather than the app because the MCP server and the AI assistant drive `Session` without an `App`, and because reopening a file should land you where you were. It is therefore part of undo snapshots, which means undo can change the active component; that matches what a user expects after undoing "New Component".

### Bodies know their component

```rust
pub struct Body {
    // ...
    pub component: Id,
    /// World from local: the component's cumulative placement when it was built.
    pub placement: DAffine3,
}
```

Body naming in `merge` changes from one global counter to a counter per component (`BTreeMap<Id, usize>` in `rebuild`), so each component numbers its own `Body1, Body2…`.

### File format

`io::FORMAT_VERSION` goes to 5. `to_json` writes version 5 only when the document has a component feature or any feature with a non-root owner, following the existing pattern that keeps plain designs readable by older apps. Older apps already refuse newer versions with a clear message.

### Validation (`validation.rs`)

- `owner` is 0 or the id of a `Component` feature that appears **earlier** in `features` (a feature cannot belong to a component made after it).
- No cycles; depth at most 32 (a `Component` whose owner chain does not reach the root is rejected).
- `active_component` is 0 or an existing component.
- `Placement` values finite.

Broken owners should fail loading the file (they are structural), unlike broken feature dependencies which remain loadable and show as timeline errors.

## Rebuild

`Document::rebuild` keeps its shape: evaluate values, solve sketches, then one ordered pass over the active, unsuppressed features. Changes:

1. **Effective suppression.** A feature is skipped if it or any component in its owner chain is suppressed, or if its owner is not among the features built so far (which happens when the timeline is rolled back before the component chip). Compute `alive: BTreeSet<Id>` of components as the loop goes.
2. **Scoped merge.** `merge` takes the feature's owner and filters `hits` to bodies with the same `component`. New bodies are tagged with the owner.
3. **Through All** filters `bodies` to the owner's component before measuring reach.
4. **Placement last.** After the loop, walk components in timeline order, compose each one's placement with its owner's (`parent_world * local`), and for every body in that component call the existing `Body::place` with the equivalent `[Scale(1), Turn x, Turn y, Turn z, Shift]` steps so exact solids, meshes and modeled threads all move together. Record the `DAffine3` on the body.

Everything the loop does therefore happens in component-local coordinates, which coincide with document coordinates while placements are identity. That keeps every existing feature's stored geometry (sketch planes, `Blend.edges`, `Hole.at`, `Text.face`, frames) meaningful without rewriting it, and placing a component later never invalidates its history.

`Built` gains `components: Vec<(Id, DAffine3, visible)>` so the app and API can draw origin triads and report placements without re-deriving them.

## Placement

A component's placement is a rigid move (rotate, then translate) of the whole subtree, edited with **Move** while a component is selected. It is stored on the component feature, not as a timeline item, and edits to it go through `Session::edit` so they are undoable. This is deliberately simpler than Fusion's capture-position dance.

Consequences that need code:

- **Picking maps back to local.** The viewport picks against placed meshes and gets world points. Before anything is stored in a feature, the point (and any face plane) must be taken to the body's local frame with `body.placement.inverse()`. Call sites: `pick_face` for New Sketch and the Extrude dialog's face and To Face, `pick_edge` for Fillet/Chamfer, Shell faces, Hole positions and direction, Thread face, Text face, `face_of` in `api.rs` (which takes a world point from the caller), and `Face::sketch` (the hidden sketch an extruded face uses). One helper on `App` and one in `api.rs`, both built on `Body::to_local(&self, p: DVec3)` and `Body::plane_to_local(&self, Plane)`.
- **Body drag (`slide_body`) and Transform values** are local; the screen delta is rotated by the inverse of the component rotation before it becomes a translate.
- **Measure, Section, Fit, screenshots, STL and STEP** use placed bodies and need no change beyond reading `Built` as they do now.
- **Sketch display** for sketches in a placed component: `draw_sketch` and `sketch_pos` use `sk.plane`, which is local. Drawing a sketch in a placed component is uncommon but legal; apply the placement in `work_plane`/`on_screen` when the sketch's owner is placed (one transform of the plane, cached per frame).
- Placement with a non-identity rotation makes `Hole.dir`, which is stored as a world-looking vector, local; that is already how it is used during the build, so only the picker changes.

A reasonable staging is to ship components with placement hidden behind identity in the first PRs, and switch Move on for components once the picking paths are converted.

## Visibility

`Component.visible = false` hides the subtree. `Session::visible_bodies` checks `hidden_bodies` and the body's component chain. `set_visible` in the API accepts a component id. Hidden components are still built (unlike suppressed ones), so measurements and Combine still see their bodies.

## Deleting and moving things

- **Delete component**: removes the component feature and every feature it owns, recursively. `App::delete_feature` already confirms when dependents exist; extend that confirmation to list counts ("Delete Bracket and its 7 features and 2 bodies?"). Undoable.
- **Move a body to another component** (browser context menu): re-owns the feature that made the body and every later feature that references that body id or its sketches. Useful but easy to get wrong; schedule it last and ship only if the dependency walk is solid. Otherwise defer to 0.3.1.
- **Reparent a component** (drag in the browser): changes the component's `owner`; refused if the new parent comes later in the timeline. Low priority.

## API and MCP (`api.rs`)

New commands:

```
{"op":"create_component","name":"Bracket","parent":ID?,"activate":true?}
    → {"component": ID}
{"op":"activate_component","id":ID}            0 or omitted activates the root
{"op":"move_component","id":ID,"translate":[x,y,z]?,"rotate":[rx,ry,rz]?}
    values are expressions in document units, like "transform"
```

Existing commands change as little as possible:

- `get_scene_info` adds `"components"`: a tree `[{id, name, visible, placement, children:[…]}]`, and `"active_component"`. Every feature and body gains `"component": ID`.
- `get_object_info` on a component id returns the same node plus its bodies and feature ids.
- `set_visible` accepts a component id.
- `delete_feature` on a component deletes the subtree and reports `removed_features`.
- `edit_feature` accepts `"owner"` only for the move-body work above; otherwise owners are set by activation.
- `export_stl` and `export_step` take an optional `"component": ID` to write a subtree; bodies are written placed. STEP output stays a flat list of solids (no product structure) in 0.3.0.
- New sketches and untargeted features land in the active component. Body-targeted operations follow their target body's owner, and patterns follow their source. The `"face"` form of `create_sketch` and the `face_of` helper take world pick points in document units and convert them as described under Placement.
- Sketch/origin/typed plane inputs use the owning component's local frame. Literal three-point coordinates are stored in millimeters as `PointRef::World`; that legacy name does not mean world-space input. Face, edge, and vertex pick points are world-space inputs in document units.
- Sketch query `plane` reports the resolved world frame, while `local_plane` explicitly reports the owner-local frame. Both origins are in document units; axes and normals are unit vectors. Measurements and exported solids use world placement.

`REFERENCE` and the MCP tool descriptions get a short "Components" section, kept inside the client size limit that `get_reference` was split up for.

The AI assistant (`crates/ferrender/src/ai.rs`) needs no change beyond the reference text.

## App changes (`crates/ferrender`)

- `panels::browser` becomes recursive over components. Keep the flat per-component sections (`Origin`, `Construction`, `Sketches`, `Bodies`) so the root view looks like today's browser when there are no components.
- `App` gains `sel_component: Option<Id>` alongside `sel_body` and `sel_face`; `Action::NewComponent`, `Action::ActivateRoot`; Move (`Action::Transform`) checks `sel_component` first.
- `panels::timeline`: chip for `Component` features, owner in tooltip, per-component underline.
- `panels::status`: active component name.
- `view::draw_bodies`: origin triad for the active non-root component (small, at its placed origin) so moved components are legible.
- Picking conversions listed under Placement.
- `recovery.rs` and file dialogs: no change; they store the document JSON.
- `uitest.rs`: new tests below.

## Tests

Core (`crates/fr-core/tests/components.rs`):

- Two components, each with an overlapping extrude set to Join: two bodies remain, one per component. The same with the second component suppressed: one body. Rolled back before the second component's chip: one body.
- Cut in component B does not touch a body in A; Combine with target in A and tool in B produces one body in A and removes B's body unless `keep_tools`.
- Pattern of a feature in B puts copies in B.
- Body names restart per component; `get_scene_info` reports owners and the tree.
- Placement: a translated and rotated component moves its exact body, mesh body and a modeled thread together; `Body::to_local` of a picked point lands on the original face; a sketch created on a face of a placed component extrudes to the expected world position.
- Serialization: round trip; files without components stay version 4 or lower; a file with an owner pointing forward or at a non-component is rejected; depth over 32 rejected.
- Undo of `create_component` restores the previous active component.
- `delete_feature` on a component removes the subtree and nothing else.

App (`uitest.rs`):

- New Component from the toolbar, sketch and extrude inside it, browser shows the body under the component and not under the root; activate root and extrude again; two bodies.
- Hide a component from the browser eye; its body disappears from the viewport and from STL export of the visible set.
- Move a component with the dialog; clicking its face afterwards starts a sketch whose extrude lands on the moved face (regression for the local-coordinate mapping).
- Timeline rollback before the component chip removes it.

## Work breakdown

Each step is a PR that leaves `main` releasable.

1. **Model and format.** `Component`, `Feature.owner`, `Document.active_component`, validation, version 5 gating, `Body.component`. No UI. Scoped `merge`, per-component body names, effective suppression. Core tests.
2. **API.** `create_component`, `activate_component`, scene info and object info, `set_visible`, `delete_feature` subtree, export filters. Reference text. API tests.
3. **Browser and timeline.** Tree browser with activation and visibility, New Component action and toolbar button, chip and tooltips, status bar. UI tests for creation and hiding.
4. **Placement.** `Placement` on the component, Move dialog on a selected component, placement pass in rebuild, `Built.components`, origin triad. `Body::to_local` and the picker conversions. Placement tests.
5. **Deletion with confirmation, docs, release notes.** README sections (Browser, Model, Timeline, Reference), `docs/releases/0.3.0.md`, this plan updated to describe what shipped. Optionally the move-body-to-component command.

Construction planes touch the same rebuild loop and the New Sketch picker. Build planes first (they are the smaller change) and then components, so step 1 extends a loop that already handles plane features; see the ordering note in the planes plan.

## Risks and open questions

- **Semantics change for existing files?** No: with no components, owner is 0 for everything and `merge` filters to the root, which is every body. Behaviour is identical.
- **Pattern body ids** are `feature_id * 1000 + k`. Feature allocation must reserve each pattern's synthetic-ID range, and loading must reject collisions with ordinary feature IDs. Document-wide feature numbering alone does not prevent a later ordinary feature from entering that range.
- **Placement and face anchors.** The `frame`-based re-finding in `exact::candidates` runs on local bodies, so it is unaffected. The risk is a missed picker conversion: the UI test on a moved component is the guard.
- **Does the Transform feature still make sense?** Yes, for moving one body inside a component. The Move dialog chooses: component selected → placement; body or face selected → Transform feature, as today.
- **Active component in undo history** is a judgement call noted above; revisit if it feels wrong in use.
- **Instances.** The flat "feature owned by one component" model does not give two placements of one definition. That needs either copy-on-create (cheap, breaks the link) or a definition/occurrence split (the Fusion way). Out of scope; the data model above does not preclude a later `Component.definition: Option<Id>`.

## Not in 0.3.0

Joints and joint limits, grounding (placement is always explicit), component instances and external components, STEP assembly structure, per-component colours or appearances, dragging to reparent in the browser, and free dragging of component geometry outside Move. The Move dialog includes graphical translation arrows and rotation handles for whole components.
