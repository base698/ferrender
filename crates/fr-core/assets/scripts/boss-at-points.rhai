// A boss (a round stud) at each point of a sketch: a repetitive modelling task.
const META = #{
    name: "Bosses at points",
    description: "Adds a cylindrical boss on the selected body at every point of a sketch.",
    inputs: [
        #{ name: "sketch", kind: "sketch", initial: (), help: "A sketch whose lone points mark the bosses." },
        #{ name: "body", kind: "body", initial: (), help: "The body to join them to (the selected one)." },
        #{ name: "diameter", kind: "length", initial: "6 mm" },
        #{ name: "height", kind: "length", initial: "4 mm" },
        #{ name: "hole", kind: "length", initial: "0 mm", help: "A hole down each boss; 0 for none." },
    ],
};

fn run(inputs) {
    let sketch = inputs.sketch ?? selection().sketches?[0];
    let body = inputs.body ?? selection().bodies?[0];
    if sketch == () { fail("choose a sketch with points"); }
    if body == () { fail("choose the body the bosses go on"); }
    let sk = info(sketch);
    let plane = sk.plane;
    let points = sk.points.filter(|p| p.lone == true);
    if points.len() == 0 { points = sk.points; }
    if points.len() == 0 { fail("the sketch has no points"); }
    let origin = plane.origin; let x = plane.x; let y = plane.y; let n = plane.normal;
    for (p, i) in points {
        progress(i.to_float() / points.len(), "boss " + (i + 1));
        let at = [origin[0] + x[0] * p.x + y[0] * p.y, origin[1] + x[1] * p.x + y[1] * p.y, origin[2] + x[2] * p.x + y[2] * p.y];
        let s = create_sketch(#{ plane: #{ origin: at, normal: n, x: x } }).sketch;
        add_geometry(#{ sketch: s, items: [#{ type: "circle", center: [0, 0], radius: inputs.diameter / 2.0 }] });
        extrude(#{ sketch: s, distance: inputs.expr.height, operation: "join" });
        if inputs.hole > 0.0 {
            let top = [at[0] + n[0] * inputs.height, at[1] + n[1] * inputs.height, at[2] + n[2] * inputs.height];
            hole(#{ body: body, at: top, diameter: inputs.expr.hole, depth: inputs.expr.height });
        }
    }
    log(points.len() + " bosses added");
    points.len()
}
