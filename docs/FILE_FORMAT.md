# The Ferrender file format

This is the normative description of `.ferr` files as written by Ferrender 0.4. It covers the two outer forms (plain JSON and the ZIP container), the document object, every feature kind's fields, how references to geometry are stored, the version table and the compatibility rules. The test `crates/fr-core/tests/format_fixture.rs` freezes one example of every feature kind in `crates/fr-core/tests/fixtures/all-features.ferr`; a change that alters serialization must update both the fixture and this document.

Lengths are millimetres, angles are degrees, coordinates are right-handed with Z up. Numbers are JSON numbers; `f32`-precision is noted where it applies.

## Outer forms

A reader tells the forms apart by the first bytes of the file.

### Plain JSON (first byte `{`)

One UTF-8 JSON object, pretty-printed with two-space indentation, at most 64 MiB. This is the only form before 0.4 and remains the form for any design that has no binary payloads. It is what `git diff`, the MCP `open`/`save` commands and older Ferrender builds work with.

### Container (first bytes `PK`)

A ZIP archive, at most 2 GiB and 4096 entries, written when the design carries a reference image or an imported mesh. Entries, in the order written:

| Entry | Compression | Content |
|---|---|---|
| `manifest.json` | Deflate | `{"format":"ferrender-container","version":9,"min_reader":9,"design_version":N,"app":"Ferrender 0.4.0","kernel":{"cadrum":"0.8.20"},"saved":"2026-10-08T21:14:03Z","entries":[...]}`. `min_reader` is the lowest format version that can open the container; `design_version` is the version the JSON inside would carry as a plain file. |
| `design.json` | Deflate | The document object exactly as the plain form, except that payload fields hold a marker object `{"blob": "<entry name>"}` instead of their base64 string. At most 64 MiB. |
| `images/<feature id>.png` | Stored | A sketch's reference image, as the normalised RGBA8 PNG the plain form embeds in base64. At most 20 MiB each. |
| `meshes/<feature id>.mesh` | Deflate (fastest level above 8 MiB) | An imported mesh, indexed. A 32-byte header: the magic `FRMESH01`, then little-endian `u32` vertex count, triangle count, flags (bit 0: vertices are welded) and CRC-32 (IEEE, of everything after the header), then 8 reserved zero bytes. Then the vertices as three little-endian IEEE 754 `f32` each, then the triangles as three little-endian `u32` vertex indices each, counter-clockwise seen from outside. At most 16 000 000 triangles and three vertices per triangle. Readers also accept the pre-release `meshes/<id>.tris` entry: unindexed `f32` triangle triples, 36 bytes per triangle, no header. |
| `cache/index.json` | Deflate | Optional, the geometry cache: `{"design_crc": CRC-32 of the plain JSON of the design, "kernel": "cadrum 0.8.20", "bodies": [{"id", "name", "component", "placement", "local_bounds", "volume", "bounds", "triangles", "tags", "brep": "cache/N.brep" or "mesh": "cache/N.mesh"}], "planes": [{"id", "component", "plane", "corners"}], "errors": {}, "resolutions": {}}`. At most 8 MiB. |
| `cache/<body id>.brep` | Deflate | An exact body in OpenCascade's binary BRep form, as cadrum writes it. The whole cache is at most 32 MiB uncompressed, else none is written. |
| `cache/<body id>.mesh` | Deflate | A mesh body, in the same blob format as `meshes/`. |
| `thumbnail.png` | Stored | Optional. A 256 × 256 isometric render of the visible bodies at save time. At most 4 MiB. |

**Geometry cache.** Written when the file is a container anyway, or when the design took 250 ms or more to rebuild (the API's `save` takes `"cache": true|false` to force or skip it). A reader uses it only when `design_crc` equals the CRC of the design it has just read, `kernel` is its own, and every body read back has the volume and bounds the index says; otherwise the whole cache is ignored and the design rebuilds. A reader whose newest version is below `design.json`'s `version` cannot read the design, but when the file has a cache it shows the cached bodies read-only, so a file saved by a newer Ferrender can still be viewed and exported.

Only these names are allowed; any other entry, any entry whose name contains `\`, starts with `/` or has a `.`/`..`/empty path segment, and any entry declaring more than its limit make the reader refuse the file before decompressing anything. Entry names use `/` as the separator. Entries are not encrypted and never use a compression method other than Deflate or Stored.

**Upgrade and backup.** Every plain file opens in 0.4. When a design that needs the container is saved over an existing plain file, the plain file is first copied, once, to `<name> (0.3 backup).ferr` in the same folder; later saves do not touch the backup. A design without payloads is always written plain, so files from before 0.4 that do not use images or meshes are rewritten byte-compatibly.

## The document object

```json
{
  "format": "ferrender",
  "version": 5,
  "units": "mm",
  "params": [ {"name": "width", "expr": "20 mm"} ],
  "features": [ ... ],
  "hidden_bodies": [ 3 ],
  "next_id": 27,
  "rollback": 12,
  "active_component": 24
}
```

| Field | Type | Meaning |
|---|---|---|
| `format` | string | Always `"ferrender"`. Anything else is refused. |
| `version` | integer | The **lowest** format version able to read this document (see the table below); a reader refuses a higher version than it knows. Readers ignore fields they do not know, so new optional fields do not raise the version. |
| `units` | `"mm"`, `"cm"` or `"in"` | Display and entry units. Stored values are millimetres regardless. Default `"mm"`. |
| `params` | array | Named parameters. `expr` is an expression in the document's units at save time, always with explicit units (`"20 mm"`, `"45 deg"`, `"3"`). At most 1024. |
| `features` | array | The timeline, in order. At most 10 000. |
| `hidden_bodies` | array of ids | Bodies hidden in the viewport. |
| `next_id` | integer | The next feature id. Ids are never reused; pattern copies use `source_feature_id * 1000 + k`. |
| `rollback` | integer, optional | How many features the timeline currently builds; absent means all. |
| `active_component` | integer, optional | The component new features go into; absent or `0` is the root. |

### Values

A dimension or feature value is `{"expr": "...", "v": 20.0}`: the expression as typed (unit-pinned, may reference `$params`) and its evaluated value in millimetres, degrees or as a plain number. Readers re-evaluate `expr` on load; `v` is a cache.

### Features

```json
{ "id": 3, "name": "Extrude1", "suppressed": false, "owner": 24, "kind": { "extrude": { ... } } }
```

`owner` (optional, default `0`) is the component the feature belongs to and must name a `component` feature earlier in the list. `kind` is an object with exactly one key naming the feature kind. The kinds:

| Key | Fields | Notes |
|---|---|---|
| `sketch` | `plane` {`origin`,`x`,`y`}, `points` {id: [x,y]}, `entities` {id: entity}, `constraints` {id: constraint}, `next`, `visible`, `fixed` (array, optional), `arc_guides` (optional), `reference` (optional), `on` (optional plane feature id) | Point `0` is the fixed origin and is always `[0,0]`. `plane` is in the owner component's frame; `on` means the plane follows a construction plane and `plane` is the last resolved value. |
| `extrude` | `sketch`, `profiles` [[entity ids]], `distance` value, `symmetric`, `op`, `through_all`, `taper` (value, optional) | `profiles` names each region by the entity ids on its outer boundary. `op` ∈ `new`/`join`/`cut`/`intersect`. |
| `revolve` | `sketch`, `profiles`, `axis` (`"x"`, `"y"` or `{"line": id}`), `angle` value, `op` | |
| `pattern` | `source` (feature id), `kind`: `{"linear": {axis, count, spacing, second?: {axis, count, spacing}}}`, `{"circular": {axis, count, angle}}` or `{"mirror": {axis}}` | Axes are the owner component's: `0` = X, `1` = Y, `2` = Z. |
| `primitive` | `shape` (`{"box": {width, depth, height}}`, `{"cylinder": {diameter, height}}`, `{"sphere": {diameter}}`, `{"cone": {bottom_diameter, top_diameter, height}}`, `{"torus": {major_radius, tube_radius}}`; all values), `position` [3 values], `rotate` [3 values], `op` | |
| `import` | base64 string (plain form) or `{"blob": "meshes/N.mesh", "triangles": T}` (container) | The plain form is the little-endian `f32` triangle soup of versions 3–8, 36 bytes per triangle, at most 1 000 000 triangles; readers weld it on load. Meshes larger than that only exist in containers. |
| `transform` | `body`, `translate` [3 values], `rotate` [3 values], `scale` value | Scale about the origin, rotate about X then Y then Z, then translate. |
| `mesh_op` | `body`, `op`, `region` (optional) | `op` is one of `{"repair": {fill_holes}}`, `{"decimate": {target, method: "quadric"\|"cluster", preserve_boundary}}`, `{"smooth": {iterations, strength}}`, `{"subdivide": {levels, scheme: "loop"\|"midpoint"}}`, `{"cut": {plane, keep: "negative"\|"positive"\|"both", cap}}`, `{"mirror": {plane, weld}}`, `{"offset": {distance value, direction?}}`, `{"extrude_region": {distance value, direction?}}`, `{"sculpt": {brush: "pull"\|"push"\|"inflate"\|"smooth"\|"flatten", at, radius value, strength value}}`. `region` is `{"sphere": {centre, radius}}`, `{"box": {lo, hi}}`, `{"side": {plane}}`, `{"normal": {direction, degrees}}` or `{"connected": {seed}}`. Applied to an exact body it makes the body a mesh. |
| `relief` | `image` (as a sketch's `reference`), `plane`, `width`, `depth`, `base` (values), `resolution`, `invert`, `blur`, `gamma`, `op` | A height field from the image's luminance on the plane; `resolution` cells along the longer side (2–1200). |
| `combine` | `target`, `tools` [body ids], `op`, `keep_tools` | |
| `blend` | `body`, `edges` [[x,y,z]], `size` value, `chamfer` (bool), `frame` | A fillet when `chamfer` is false. Edges are named by a point on them; see References. |
| `shell` | `body`, `faces` [[x,y,z]], `thickness` value, `frame` | |
| `hole` | `body`, `at` [[x,y,z]], `dir` [x,y,z], `shape` (`simple`/`counterbore`/`countersink`), `fit` (`plain`/`close`/`normal`/`loose`/`tapped`), `thread` (catalog name, e.g. `"M3x0.5"`), `diameter`, `depth`, `tip_angle`, `head_diameter`, `head_depth`, `head_angle` (values or null), `modeled`, `left`, `extra` | `depth` null means through. |
| `thread` | `body`, `face` [x,y,z], `frame`, `thread`, `offset`, `length`, `left`, `extra` | |
| `text` | `text`, `plane`, `height`, `depth`, `spacing`, `angle`, `x`, `y` (values), `align` (`left`/`center`/`right`), `op`, `body`, `face`, `frame` | `body`/`face` are set for raised or engraved text on a face, null for free-standing text. |
| `split` | `body`, `plane` (`{"origin": "XY"\|"XZ"\|"YZ"}`, `{"plane": id}` or `{"face": {...}}`) | |
| `remove` | `bodies` [ids] | Removes bodies at this point of the timeline. |
| `component` | `placement` {`translate` [3 values], `rotate` [3 values]}, `visible` | The component's parent is the feature's `owner`. |
| `plane` | `kind` (`{"offset": {base, distance}}`, `{"midplane": {a, b, flip}}`, `{"three_point": {points}}`), `visible`, `visibility_pinned` | `base`, `a`, `b` are plane references: `{"origin": "XY"}`, `{"plane": id}` or `{"face": {...}}`; `points` are `{"world": [x,y,z]}`, `{"vertex": {...}}` or `{"sketch_point": {sketch, point}}`. |

Sketch entities are `{"type": "line", "a": p, "b": p, "construction": false}`, `{"type": "circle", "c": p, "r": 3.0, ...}`, `{"type": "arc", "c": p, "s": p, "e": p, ...}` (counter-clockwise from `s` to `e`) and `{"type": "spline", "a": p, "b": p, "c": p, "d": p, ...}` (interpolating through four fit points), where `p` is a point id. `arc_guides` records how an arc was drawn: `{"kind": "through", "point": id}` for a three-point arc or `{"kind": "tangent", "source": entity, "start": point}` for a tangent arc. `fixed` lists points and circles that are locked (projected geometry).

Constraints are `{"kind": "...", "refs": [ids], "value": value?}`. Geometric kinds: `coincident`, `horizontal`, `vertical`, `parallel`, `perpendicular`, `collinear`, `tangent`, `equal`, `midpoint`, `concentric`, `symmetric`, `fix`. Dimension kinds carry a `value`: `distance`, `radius`, `diameter`, `angle`, `position_x`, `position_y`. `refs` are in the canonical order the solver expects; a file whose refs are out of order is refused.

A sketch's `reference` image is `{"png": <base64 or blob marker>, "name", "pixel_width", "pixel_height", "origin": [x,y], "width", "rotation", "opacity", "visible"}`; the pixels are always a normalised RGBA8 PNG with no metadata, at most 4 megapixels and 8192 on a side.

### References to faces, edges and vertices

Features that act on existing geometry (`blend`, `shell`, `hole`, `thread`, `text`, `plane`, `split`) name a face or edge by **a point on it in world coordinates** and the **bounding box of the body at the time of the pick** (`frame`: `[[xmin,ymin,zmin],[xmax,ymax,zmax]]`). On rebuild the feature looks for the face or edge nearest that point, both where it was and at the same relative position inside the body's current bounds, so the reference survives the body changing size.

Since format 10 a reference also carries a **tag** saying how the face was made, and resolves by tag first. A face tag is `{"origin": ORIGIN, "kind": KIND}` with `kind` one of `plane`, `cylinder`, `cone`, `sphere`, `torus`, `freeform` and `origin` one of:

| Origin | Meaning |
|---|---|
| `{"swept": {"feature": F, "entity": E}}` | The side face an extrude or revolve `F` swept from sketch entity `E`. |
| `{"cap": {"feature": F, "end": false\|true}}` | The start or end cap of `F`. |
| `{"made": {"feature": F, "n": N}}` | The `N`th face made by a fillet, chamfer, shell, hole, thread, primitive, split or text `F`, in kernel order. |
| `{"split": {"of": TAG, "n": N}}` | Piece `N` of an earlier face that a later operation cut up. |
| `{"copy": {"of": TAG, "n": N}}` | The same face on copy `N` of a pattern. |

A plane reference (`split.plane`, `plane.kind.offset.base`, midplane faces, `mesh_op` cuts and mirrors) is `{"origin": "XY"|"XZ"|"YZ"}`, `{"plane": id}`, `{"face": {body, at, frame, tag?}}` or, since format 11, `{"free": PLANE}` with a plane given outright in the owner component's frame.

An edge tag is `{"faces": [TAG, TAG]}`, the two faces it separates, in sorted order. Tags are stored in `blend.tags` (one per entry of `edges`, `null` where the edge had none), `shell.tags` (one per `faces`), `thread.tag`, `text.tag` and the `tag` of a `{"face": ...}` plane reference. On rebuild a pick is resolved at one of three levels, which `get_object_info` reports as `resolved`: `tag` (a face or edge with the very tag; among equals, the nearest to the point), `origin` (one with the same tag ignoring split and copy ordinals, nearest to the point), `position` (the pre-0.4 search). A reference with no tag learns the tag of what it found on its first successful build, so files from before format 10 gain tags as they are opened, and are stamped 10 when next saved. A pick whose tagged face has been removed altogether fails with the usual "no longer there" error rather than relocating.

## Version table

| `version` | First written by | Added |
|---|---|---|
| 1 | 0.1.0 | The base: sketches with lines, circles and arcs, extrude, revolve, import, transform, combine, pattern, fillet, chamfer, shell, hole, thread. |
| 2 | 0.1.x | `text`. |
| 3 | 0.2.0 | Reference images, splines, three-point and tangent arc guides, `position_x`/`position_y` dimensions. |
| 4 | 0.2.2 | Persistent angle locks (a single-line `angle` constraint). |
| 5 | 0.3.0 | Components (`owner`, `component`, `active_component`), construction planes (`plane`, `sketch.on`). |
| 6 | 0.3.0 | Rectangular grid patterns (`pattern.kind.linear.second`). |
| 7 | 0.3.0 | `primitive`. |
| 8 | 0.3.0 | `split` and `remove`. |
| 9 | 0.4.0 | The ZIP container. A plain JSON file is never stamped 9; only `manifest.json`'s `min_reader` carries it. |
| 10 | 0.4.0 | Face and edge tags on references (`blend.tags`, `shell.tags`, `thread.tag`, `text.tag`, plane `face.tag`). |
| 11 | 0.4.0 | `mesh_op` and `relief` features; the `free` plane reference. |

The writer computes the lowest version that covers what the document uses; a reader accepts any version up to the newest it knows and refuses higher ones with "this file was written by a newer version of Ferrender". There is no migration code: every version's documents deserialize directly, with absent fields taking their defaults. The version is therefore a promise about *readers*, not a schema identifier, and a design can go down in version when the feature that required it is deleted.

## Limits

| Limit | Value |
|---|---|
| Plain file or `design.json` | 64 MiB |
| Container | 2 GiB, 4096 entries |
| Geometry cache | 32 MiB uncompressed in all, index 8 MiB |
| Features | 10 000 |
| Parameters | 1024 |
| Items in one sketch (points + entities + constraints + guides) | 10 000 |
| Expression length | 4096 bytes |
| Imported mesh | 16 000 000 triangles (1 000 000 in the inline plain form); import files up to 800 MB |
| Reference image | 4 megapixels, 8192 px per side, 20 MiB as PNG |
| Document ids | `next_id` below `u32::MAX / 1000 − 10 000`, because pattern copies take `id * 1000 + k` |

On load the document is validated against these limits and against structural rules (unique ids below `next_id`, points referenced by entities exist, constraint refs canonical, owners precede their features, no component cycles deeper than 32) before anything reaches the solver or the kernel. Broken *dependencies* (a deleted sketch, a face that is gone) are not load errors: the file opens and the feature shows the error in the timeline so it can be repaired.

## Recovery copies

Unsaved work is copied to `<config dir>/recovery/*.ferr-recovery`: a JSON object `{"format": "ferrender-recovery", "path": ..., "saved": <unix seconds>, "doc": <the plain document object>}`. Recovery copies always use the plain form, payloads inline, and are private to the machine.

## Related

`docs/0.4-release.md` for the geometry cache, tags and mesh blobs planned on top of this; `README.md` › Files for the user-facing description.
