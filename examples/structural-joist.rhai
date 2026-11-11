// Illustrative inputs, not measured shed dimensions or a species/grade lookup.
const META = #{
    name: "Structural joist estimate",
    description: "Build a measured rectangular joist and estimate bending, shear and sag. Replace all example material values before using results.",
    inputs: [
        #{ name: "length", kind: "length", initial: "3000 mm" },
        #{ name: "width", kind: "length", initial: "38.1 mm" },
        #{ name: "depth", kind: "length", initial: "184.15 mm" },
        #{ name: "bearing", kind: "length", initial: "38.1 mm" },
        #{ name: "spacing", kind: "length", initial: "406.4 mm" },
        #{ name: "pressure_pa", kind: "number", initial: 2000.0 },
        #{ name: "e_mpa", kind: "number", initial: 10000.0 },
        #{ name: "fb_mpa", kind: "number", initial: 8.0 },
        #{ name: "fv_mpa", kind: "number", initial: 1.0 },
    ],
};

fn run(inputs) {
    if inputs.bearing * 2.0 >= inputs.length { fail("The bearings leave no clear span."); }
    new_design(#{ units: "mm" });
    set_parameter(#{ name: "joist_length", expr: inputs.expr.length });
    let sk = create_sketch(#{ plane: "YZ" }).sketch;
    add_geometry(#{ sketch: sk, items: [#{ type: "rect",
        from: [-inputs.width / 2.0, -inputs.depth / 2.0],
        to: [inputs.width / 2.0, inputs.depth / 2.0] }] });
    let body = extrude(#{ sketch: sk, distance: "$joist_length", operation: "new" }).feature;
    set_structural_member(#{ body: body, member: #{
        species: "Example only: replace with grade stamp species",
        grade: "Example only: replace with verified grade",
        nominal_size: "2x8 (label only)",
        design_values: #{ e_mpa: inputs.e_mpa, fb_mpa: inputs.fb_mpa, fv_mpa: inputs.fv_mpa,
            source: "ILLUSTRATIVE VALUES: replace with verified NDS edition/table/row and applicable factors",
            factors: #{ bending: #{ CD: 1.0 }, shear: #{ CD: 1.0 } } },
        density_kg_m3: 500.0,
        end_bearing_mm: [inputs.bearing, inputs.bearing]
    } });
    // Center of each measured bearing, with the loaded end overhangs retained.
    let inset = inputs.bearing / (2.0 * inputs.length);
    let result = analyze_structure(#{ analysis: #{ plane: "XZ", bodies: [body],
        supports: [#{ location: #{ body: body, at: inset }, fix: [true, true, false] },
                   #{ location: #{ body: body, at: 1.0 - inset }, fix: [false, true, false] }],
        area_loads: [#{ body: body, pressure_pa: inputs.pressure_pa,
            tributary_width_mm: inputs.spacing, direction: [0.0, -1.0] }],
        include_self_weight: true, deformation_scale: 20.0
    } });
    log("Preliminary estimate only; example inputs must be replaced. No connection or code approval check.");
    log("Bending utilization: " + result.members[0].bending_utilization);
    log("Shear utilization: " + result.members[0].shear_utilization);
    log("Sampled displacement (mm): " + result.members[0].sampled_max_displacement_mm);
    result
}
