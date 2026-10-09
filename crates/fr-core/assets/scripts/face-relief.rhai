// A printable relief from a depth map or photo, mounted on a plaque.
const META = #{
    name: "Face relief",
    description: "Builds a relief from an image, smooths and trims it, thickens it into a solid, mounts it on a plaque and exports an STL.",
    inputs: [
        #{ name: "image", kind: "file", initial: "", help: "A depth map (bright = near) or a photo." },
        #{ name: "width", kind: "length", initial: "100 mm" },
        #{ name: "height", kind: "length", initial: "7 mm", help: "How high the brightest pixel stands." },
        #{ name: "resolution", kind: "integer", initial: 400 },
        #{ name: "smooth", kind: "integer", initial: 4, label: "Smoothing passes" },
        #{ name: "triangles", kind: "integer", initial: 60000, label: "Triangles after decimation" },
        #{ name: "trim", kind: "length", initial: "8 mm", help: "Border trimmed from each side." },
        #{ name: "thickness", kind: "length", initial: "2 mm", help: "Solid thickness under the relief." },
        #{ name: "plaque", kind: "length", initial: "3 mm", help: "Plaque thickness; 0 for none." },
        #{ name: "stl", kind: "file", initial: "", help: "Where to write the STL; empty for beside the image." },
    ],
};

fn run(inputs) {
    if inputs.image == "" { fail("choose an image"); }
    progress(0.05, "relief");
    let made = mesh_from_image(#{ path: inputs.image, width: inputs.expr.width, depth: inputs.expr.height, base: "0 mm", resolution: inputs.resolution, blur: 1 });
    let body = made.changed_bodies[0].id;
    let size = made.changed_bodies[0].size;
    if inputs.smooth > 0 { progress(0.2, "smoothing"); mesh_smooth(#{ body: body, iterations: inputs.smooth, strength: 0.5 }); }
    progress(0.35, "decimating");
    mesh_decimate(#{ body: body, target: inputs.triangles });
    let t = inputs.trim;
    if t > 0.0 {
        progress(0.5, "trimming");
        mesh_cut(#{ body: body, plane: #{ origin: [t, 0, 0], normal: [-1, 0, 0] }, cap: false });
        mesh_cut(#{ body: body, plane: #{ origin: [size[0] - t, 0, 0], normal: [1, 0, 0] }, cap: false });
        mesh_cut(#{ body: body, plane: #{ origin: [0, t, 0], normal: [0, -1, 0] }, cap: false });
        mesh_cut(#{ body: body, plane: #{ origin: [0, size[1] - t, 0], normal: [0, 1, 0] }, cap: false });
    }
    progress(0.65, "thickening");
    mesh_offset(#{ body: body, distance: inputs.expr.thickness, direction: [0, 0, -1] });
    if inputs.plaque > 0.0 {
        progress(0.8, "plaque");
        let s = create_sketch(#{ plane: "XY", offset: -inputs.thickness }).sketch;
        add_geometry(#{ sketch: s, items: [#{ type: "rect", from: [t - 5.0, t - 5.0], to: [size[0] - t + 5.0, size[1] - t + 5.0] }] });
        let plaque = extrude(#{ sketch: s, distance: -inputs.plaque, operation: "new" }).changed_bodies[0].id;
        fillet_edges(#{ body: plaque, edges: "all", radius: inputs.plaque / 3.0 });
        combine(#{ target: body, tools: [plaque], operation: "join" });
    }
    let m = mesh_measure(#{ body: body });
    log("relief: " + m.triangles + " triangles, watertight " + m.watertight + ", volume " + m.volume.round() + " mm^3");
    let stl = if inputs.stl == "" { join(document_dir(), basename(inputs.image) + "-relief.stl") } else { inputs.stl };
    export_stl(#{ path: stl });
    log("wrote " + stl);
    stl
}
