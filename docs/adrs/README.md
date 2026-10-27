# Architecture decision records

One file per decision, numbered in the order the decisions were taken. Each record states its status, date, context, decision and consequences, then keeps the original design document in full below a rule.

| ADR | Decision | Status |
|---|---|---|
| [0001](0001-sketching-tools.md) | Sketching tools: coordinates, arcs, splines and reference images | Shipped in 0.2.0, refined in 0.2.2 |
| [0002](0002-sketching-fixes.md) | Sketching fixes from the 0.2 manual tests | Shipped in 0.2.2 |
| [0003](0003-appearance-follows-the-system.md) | Appearance follows the system, with a remembered override | Shipped in 0.2.1 |
| [0004](0004-solid-primitives.md) | Solid primitives as editable features | Shipped in 0.3.0 |
| [0005](0005-components.md) | Components: nestable containers with one owner per feature | Shipped in 0.3.0 |
| [0006](0006-construction-planes.md) | Construction planes as features | Shipped in 0.3.0 |
| [0007](0007-direct-modeling.md) | Direct modeling and placement in the viewport | Shipped in 0.3.0 |

To add one: copy the header table of any record, take the next number, and write Context, Decision and Consequences before the work starts. Status moves from Proposed to Accepted when the work is merged, or to Superseded with a link to the record that replaces it. The [file format](../FILE_FORMAT.md) is a reference, not a decision record, and stays beside this folder.
