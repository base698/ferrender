# Structural member estimates (0.4 development)

This first pass lets a Rhai script or MCP client tag measured lumber geometry and
estimate the elastic response of a planar frame. It is intended for comparing
proposed shed joist/beam arrangements using explicit dimensions and loading.
It does not determine whether an existing shed or repair passes inspection.

The implementation has five commands, automatically available as Rhai functions
and through MCP's `execute_ferrender_commands`. In the app, the [Loads panel](#the-loads-panel)
runs the quick case and draws its result on the model.

| Command | Purpose |
|---|---|
| `set_structural_member` | Set species, grade, nominal label, reference design values, per-property factors, density and optional end bearings on an extrusion. |
| `remove_structural_member` | Remove a tag, including one whose geometry was deleted. |
| `get_structural_members` | Inspect tags, actual sections, world centerlines, lengths, supplied-bearing clear spans and parallel centerline distances. Unsupported members carry an error. |
| `analyze_structure` | Build and solve a planar frame; return reactions, stress ratios, colors and deflection polylines. Does not edit the design. |
| `estimate_structure` | The quick case: one deck pressure over every tagged member as a simple span. Works out deck widths from spacing, can analyse a trial section, and can keep the case with the design. |

## The Loads panel

**Inspect → Loads** (also in the View menu and command search) puts one evenly spread
load on a deck and shows how each tagged member carries it.

- **Storage load** and **Deck weight** are pressures on the deck, in psf or kPa. The
  members' own weight is added when the box is ticked and the lumber has a density.
- Each member is a simple span between its two ends, supported at the middle of its
  bearing when a bearing length was entered. Members must be level and all run along X
  or all along Y.
- **Carries** is the width of deck a member holds up. Left empty, it is half the gap to
  the member on each side; a member at the end of the row carries as much again on its
  open side. Type a width to replace it, for example for two boards fixed together.
- The table gives each member's bending and shear ratio (stress over the allowable you
  entered; above 1 is over), its sag, and span over sag.
- On the model each member is drawn bent, with the sag exaggerated by the slider,
  coloured green through red by its worst ratio, with arrows where it is loaded.
- **Compare with every member at** analyses a trial width and depth beside the design as
  built, without changing the design. Switch the drawing between the two. To change the
  design itself, edit the member's sketch or the parameter that sizes it; the panel
  follows the real geometry.
- **Lumber** holds the species, grade, E, Fb, Fv, density and bearing. *Tag selected
  body* makes the selected plain rectangular extrusion a member; *Use for all members*
  applies the values to every member.
- **Done** keeps the load case with the design.

The same case is the `estimate_structure` command:

```json
{"op":"estimate_structure","case":{"live_pa":1436,"dead_pa":239,"include_self_weight":true,
  "tributary_mm":{"21":150},"trial_section_mm":[38.1,285.75]},"remember":true}
```

Everything under Allowables and interpretation below applies to the panel too. It
treats each member alone: nothing is shared through the deck, and two boards fixed
together are two members unless you give them their widths by hand.

## Starting on a laptop

Open `examples/structural-joist.rhai` in the script runner or call `run_script`
with that file's absolute path. It creates a rectangular joist, places pin/roller
supports at the centers of its supplied end bearings, applies an area load and
self-weight, and returns the report. Its material values, density and dimensions
are illustrative. Replace them with measurements and verified design values.
The script starts a new design, so save any existing work first.

Alternatively, create a closed rectangular sketch on YZ, extrude along X using
**New Body**, and send the commands below. `42` is an example body/feature id;
replace it with the extrusion's returned `feature` id.

```json
{
  "op": "set_structural_member",
  "body": 42,
  "member": {
    "species": "Replace with grade stamp species",
    "grade": "Replace with verified grade",
    "nominal_size": "2x8 (label only)",
    "design_values": {
      "e_mpa": 10000,
      "fb_mpa": 8,
      "fv_mpa": 1,
      "source": "ILLUSTRATIVE: replace with NDS edition, table and row",
      "factors": {
        "bending": {"CD": 1.0},
        "shear": {"CD": 1.0},
        "elasticity": {}
      }
    },
    "density_kg_m3": 500,
    "end_bearing_mm": [38.1, 38.1]
  }
}
```

```json
{
  "op": "analyze_structure",
  "analysis": {
    "plane": "XZ",
    "bodies": [42],
    "supports": [
      {"location": {"body": 42, "at": 0}, "fix": [true, true, false]},
      {"location": {"body": 42, "at": 1}, "fix": [false, true, false]}
    ],
    "point_loads": [
      {"location": {"body": 42, "at": 0.5}, "force_n": [0, -1000]}
    ],
    "area_loads": [
      {"body": 42, "pressure_pa": 2000, "tributary_width_mm": 406.4, "direction": [0, -1]}
    ],
    "include_self_weight": true,
    "deformation_scale": 20
  }
}
```

The JSON example supports the physical endpoints. End bearings are reporting
metadata; they do not reposition supports. The Rhai example instead uses
`at = bearing / (2 * length)` and `1 - at` for the support centerlines and retains
the uniformly loaded end overhangs. Choose support locations to match the actual
assembly. A measured clear span is the distance between inside bearing faces;
it is different from member length and support-centerline span.

## Geometry and units

- All structural inputs/outputs explicitly use **mm, N, N mm, MPa, kg/m³ and Pa**,
  regardless of the CAD document's display units. `1 psi = 0.006894757293 MPa`,
  `1 psf = 47.88025898 Pa`, `1 lbf = 4.448221615 N`.
- `at` is a fraction of the physical extrusion, from its signed start toward
  its signed end. Negative/symmetric extrusion and component placement are
  respected. Support and point-load stations split a member into beam elements.
- The rectangle's measured sides, centroid and extrusion distance supply section
  dimensions and centerline; nominal size does not change stiffness or capacity.
  Expression-driven geometry is extracted again on each analysis. The sample's
  length remains parametric; rerun the script to change its section input sizes.
- A section edge must be normal to the analysis plane, so the actual bending
  orientation determines `I = b h³ / 12` and `S = b h² / 6`. A flat board does not
  receive the upright board's stiffness. Rolled/biaxial sections are rejected.
- Only single-profile, four-sided, untapered, unmodified New Body extrusions are
  accepted. Curves, holes, taper, Through All, body transforms and pattern copies
  need further implementation. Position/rotate a member using its sketch or
  component. Later Boolean operations in its component are conservatively refused,
  even if they may not intersect it. Failed, suppressed and rolled-back members
  are errors; no stale result is reused.
- Optional `end_bearing_mm` reports `length - start_bearing - end_bearing` as
  `clear_span_mm`. Without bearing measurements the clear span is `null`.
  `parallel_spacing` measures overlapping parallel centerline separation in 3D;
  it does not infer deck load sharing or effective tributary width. For more than
  128 valid tagged bodies spacing pairs are omitted and flagged.

## Loads, supports and joints

`plane` is `XY`, `XZ` or `YZ`. Force components and `fix` translation flags follow
the listed axes; the third flag restrains rotation about their cross product.
`[true,true,false]` is a pin, `[false,true,false]` a roller in the second axis,
and `[true,true,true]` a fixed support. No support is inferred from solid contact.

`uniform_loads: [{"body":42,"force_n_per_mm":[0,-1]}]` applies a full-member
line load. Multiple loads sum. Area loads become line loads as
`pressure_pa * tributary_width_mm * 1e-6`, applied along the supplied unit
direction, per **actual** member length, not horizontal projection. Negative
components represent downward loads in XZ/YZ. Point loads optionally include
`moment_n_mm`. Loads act on the centerline; eccentric/torsional loads are absent.

`include_self_weight` is mandatory. `true` adds gravity in world -Z using each
member's supplied density and actual area; missing density and XY gravity are
errors. The reported line load includes self-weight. Decking, contents, snow and
other loads must be supplied explicitly. Avoid including lumber self-weight
again in the area load. Loading is one caller-defined scenario; no design load
combinations, live/dead separation or duration-factor selection are generated.

Omitting `bodies` selects all tags, including hidden bodies. Independent members
remain independent even if they touch or overlap in projection. To declare a
rigid joint at coincident 3D centerlines, supply:

```json
{"connections": [[{"body":42,"at":1}, {"body":57,"at":0}]]}
```

This joins both translations and rotation. It does **not** verify a connection's
moment capacity. There are no internal hinges/end releases, springs, settlements,
eccentric offsets, automatic intersections or sheathing load distribution yet.
Unrestrained mechanisms and ill-conditioned stiffness systems return errors;
the solver never invents stabilizing springs. Limits are 64 selected CAD members,
128 analysis nodes and 256 elements; supports/loads/joints have bounded counts.

## Allowables and interpretation

Supply reference `E`, `Fb`, `Fv` and the applicable per-property adjustment factors.
The source string must identify their provenance. The accepted factor names for
this first-pass **sawn-lumber ASD** workflow are:

| Property | Accepted factors |
|---|---|
| Bending `Fb` | `CD`, `CM`, `Ct`, `CL`, `CF`, `Cfu`, `Ci`, `Cr` |
| Shear `Fv` | `CD`, `CM`, `Ct`, `Ci` |
| Elasticity `E` | `CM`, `Ct`, `Ci` |

Each adjusted value is the reference value times its factors. Omitted factors
equal one; Ferrender does not establish that omission is applicable. In particular,
it does not infer repetitive-member, size, wet-service, load-duration or stability
allowances, and does not apply `CD` to stiffness. For already adjusted inputs use
unit factors and record that choice in `source` to avoid double adjustment. There
is no built-in species database or licensed NDS table content.

The report contains:

- Support reactions, node translations and rotations, and member force samples.
- Bending stress `max |M| / S`, rectangular shear stress `1.5 max |V| / A`, and
  axial stress (reported without a capacity check). Moment maxima include the
  interior zero-shear location; shear and axial extrema include both ends.
- Separate bending and shear stress/allowable ratios. `bending_shear_utilization`
  is their maximum; `color_rgb` maps that ratio from green through yellow to red
  at one. This color covers only those two checks and is never a whole-member
  approval. Axial force sets `axial_design_required`.
- 33 points per element, with original/displaced world coordinates and scaled
  `deformed_mm` for a later viewport overlay. Uniform-load deflection includes
  its particular solution, so even a fixed-fixed single element has interior sag.
  `sampled_max_displacement_mm` is the sampled translation magnitude, not an
  exact peak or an automatic span-ratio serviceability check.

Every report has `design_checks_complete: false`. Buckling, axial/bending
interaction, bearing stress, connection strength, sistering fasteners/load sharing,
creep, defects/notches, out-of-plane response and inspector acceptance remain
outside this first pass. Two sisters are not automatically treated as a composite
section. Increasing modeled width alone does not demonstrate a viable repair.

## Comparison with Forte and follow-up work

Use the measurements, loading, support locations, reference values/factors and
reaction/deflection results to compare the same assumptions in ForteWEB. The
report is ordinary JSON; it is not a Forte import format and does not invoke
Forte. Keep the manufacturer's repair calculations and the city's requirements
with the proposed repair. This feature does not resolve approval of sistering.

Useful next steps are a viewport overlay consuming the returned colors/polylines,
3D frame DOFs, explicit end releases, persisted load cases, audited NDS factor
selection and additional design checks. Member tags already persist in format 19,
survive undo/redo, and are included in exported Rhai timelines. Designs with no
member tags retain their previous minimum reader version. Analysis requests are
kept in the calling script; computed results are not saved in the CAD document.

Mechanics and input references:

- [TU Delft: Euler–Bernoulli beam elements](https://teachbooks.tudelft.nl/computational-modelling/structural_linear/euler_bernouilli.html)
- [AWC: span calculator design values and adjustment factors](https://awc.org/resources/calculator-help/)
- [AWC: where lumber design values are published](https://awc.org/faq/where-can-i-find-design-values-for-lumber-and-glued-laminated-timber/)
- [ForteWEB: spans and supports](https://www.forteweb.com/Help/Content/C_Tabs/04a_spans_and_supports.htm)
- [ForteWEB: entering horizontal-member loads](https://www.forteweb.com/Help/Content/D_Design%20Concepts/entering_loads_horizontal.htm)

Focused checks (no GUI needed):

```sh
cargo test --locked -p fr-structure
cargo test --locked -p fr-core --test structural_analysis
```
