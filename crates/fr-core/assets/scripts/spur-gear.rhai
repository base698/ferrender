// A spur gear from a few inputs: a generator.
const META = #{
    name: "Spur gear",
    description: "An involute-approximating spur gear with a bore, as one body.",
    inputs: [
        #{ name: "teeth", kind: "integer", initial: 20 },
        #{ name: "module_size", kind: "length", initial: "2 mm", help: "The gear module: pitch diameter = module x teeth." },
        #{ name: "thickness", kind: "length", initial: "6 mm" },
        #{ name: "bore", kind: "length", initial: "5 mm" },
    ],
};

fn run(inputs) {
    let n = inputs.teeth;
    if n < 6 || n > 200 { fail("teeth must be between 6 and 200"); }
    let m = inputs.module_size;
    let pitch_r = m * n / 2.0;
    let outer_r = pitch_r + m;
    let root_r = pitch_r - 1.25 * m;
    // The tooth outline as a polygon around the circle: root, flank, tip, flank, in sketch units (mm).
    let pts = [];
    let step = 360.0 / n;
    for i in 0..n {
        let a0 = (i * step).to_radians();
        let half = (step / 4.0).to_radians();      // half the tooth at the pitch circle
        let tip = half * 0.55;                        // tips are narrower
        let root = half * 1.2;
        pts.push([root_r * (a0 - root).cos(), root_r * (a0 - root).sin()]);
        pts.push([pitch_r * (a0 - half).cos(), pitch_r * (a0 - half).sin()]);
        pts.push([outer_r * (a0 - tip).cos(), outer_r * (a0 - tip).sin()]);
        pts.push([outer_r * (a0 + tip).cos(), outer_r * (a0 + tip).sin()]);
        pts.push([pitch_r * (a0 + half).cos(), pitch_r * (a0 + half).sin()]);
        pts.push([root_r * (a0 + root).cos(), root_r * (a0 + root).sin()]);
    }
    progress(0.2, "sketching " + n + " teeth");
    let sk = create_sketch(#{ plane: "XY", name: "Gear" }).sketch;
    add_geometry(#{ sketch: sk, items: [#{ type: "polygon", points: pts }, #{ type: "circle", center: [0, 0], radius: inputs.bore / 2.0 }] });
    progress(0.6, "extruding");
    let made = extrude(#{ output_key: "gear-solid", sketch: sk, distance: inputs.expr.thickness, operation: "new" });
    let body = made.changed_bodies[0].id;
    edit_feature(#{ feature: made.feature, name: "Gear " + n + "T m" + m });
    log("gear: " + n + " teeth, pitch diameter " + (2.0 * pitch_r) + " mm, outer " + (2.0 * outer_r) + " mm");
    body
}
