# A face relief from a photograph, over MCP

This is the 0.4 mesh acceptance run: a printable bas-relief of a person's face made from one photograph, using only Ferrender's tools through its MCP server, with a depth map produced outside Ferrender. It was performed on 2026-10-08. The design and images from that run (the depth map, the step screenshots and the mounted relief) are now archived in `~/Documents/3d/ferrender/models/face-relief/` on Synology `Backup/3d`, rather than in this repository, since they are a likeness of a real person. Reproduce it with your own photo.

## What you need

- Ferrender 0.4 built in release (`cargo build --release -p ferrender`, after the verified OpenCascade setup in the README), and an MCP client that can call its tools. The run below used a small stdio driver; Claude Desktop or any MCP client works the same way.
- A portrait on a plain background. A cut-out on white is ideal.
- A **depth map**: an 8-bit grey image where bright is near. Ferrender does not ship a depth model. Any monocular depth estimator produces one (Depth Anything, MiDaS, or the depth channel of a phone portrait). Without one, the fallback is a *dome prior plus shading*, which is what this run used, in a dozen lines of Python with numpy and Pillow:

```python
im = Image.open(photo).convert("RGB"); im.thumbnail((640, 640))
a = np.asarray(im) / 255.0
lum = 0.2126*a[...,0] + 0.7152*a[...,1] + 0.0722*a[...,2]
mask = (a.min(axis=2) < 0.93)                      # the person, not the white background
dome = gaussian_blur(mask, 40)                     # soft hill, highest at the middle of the head
dome = clip((dome - 0.35) / 0.65, 0, 1) ** 0.8 * mask
detail = (lum - gaussian_blur(lum, 6)) * mask      # shading becomes small relief
tone = (gaussian_blur(lum, 6) - 0.5) * mask        # hair and beard a little deeper
depth = 0.72*dome + 0.18*clip(0.5 + 0.8*tone, 0, 1)*mask + 0.10*clip(0.5 + 3*detail, 0, 1)*mask
save_png(gaussian_blur(depth, 1.2) * mask)
```

A real depth map gives a better likeness; the dome fallback gives a recognisable one.

## The run

Each step is one MCP command (`execute_ferrender_commands`), with `get_viewport_screenshot` after it to judge the result. Lengths are in millimetres.

1. **Relief.** `{"op":"mesh_from_image","path":".../depth.png","width":100,"depth":7,"base":0,"resolution":420,"blur":1}`. 352 000 triangles, an open surface 100 mm wide; `mesh_measure` says `open_edges: 1678` (its rim). Bright is high, so the nose stands highest.
2. **Smooth.** `{"op":"mesh_smooth","body":1,"iterations":4,"strength":0.5}`. Taubin smoothing takes the pixel steps out without shrinking the volume (29 040 mm³ before and after).
3. **Decimate.** `{"op":"mesh_decimate","body":1,"target":60000}`. Quadric edge collapse to 60 000 triangles; the outline is kept.
4. **Trim the border.** Four cuts with free planes, keeping the inside, no cap: `{"op":"mesh_cut","body":1,"plane":{"origin":[8,0,0],"normal":[-1,0,0]},"keep":"negative","cap":false}` and the same at x = 92 and y = 2, 98. The relief is now 84 × 96 mm.
5. **Make it solid.** `{"op":"mesh_offset","body":1,"distance":2,"direction":[0,0,-1]}`. The open surface is thickened 2 mm straight down with walls around the rim: 117 200 triangles, `watertight: true`, 16 128 mm³.
6. **A plaque.** A sketch on XY offset −2 mm, a 94 × 106 mm rectangle, extruded −3 mm as a new body, edges filleted 1 mm.
7. **Mount it.** `{"op":"combine","target":1,"tools":[10],"operation":"join"}`. The mesh boolean splits only the triangles near the plaque's top face, so this takes a few seconds for a 117 000-triangle relief. The result has 126 764 triangles.
8. **Export and save.** `export_stl` to `face-relief.stl` (6.3 MB) and `save` to `face-relief.ferr`, which is a container holding the depth map, the mesh and a geometry cache.

Total wall time for the run, including eight software-rendered screenshots, was about three minutes; the operations themselves take seconds.

## What came out

The top view shows the head's outline, the brow, the eye sockets, the smile and the beard as relief; the plaque frames it. In the viewport the software renderer's edge lines stipple a dense mesh, so judge the shape from the shaded GPU view or from the exported STL in a slicer. The combine left 35 open edges along the seam between the relief's rim and the plaque; `{"op":"mesh_repair","body":1,"fill_holes":12}` closes them, and slicers close such seams on their own.

Printed at 100 mm wide, 7 mm of relief on a 5 mm slab, the depth steps are 7 mm / 256 levels, well under a 0.2 mm layer.

## The same thing as a script

`face-relief.rhai` in **Scripts → Samples** does steps 1 to 8 from an image path and the sizes above as inputs, and `ferrender run face-relief.rhai --input image=depth.png --input stl=out.stl` does it headless.
