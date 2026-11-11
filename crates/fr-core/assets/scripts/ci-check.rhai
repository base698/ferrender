// A check for CI: the design must build without errors and have the expected bodies.
const META = #{
    name: "CI check",
    description: "Fails when the open design has timeline errors, fewer bodies than expected, or any body that is not watertight.",
    inputs: [
        #{ name: "min_bodies", kind: "integer", initial: 1 },
        #{ name: "report", kind: "file", initial: "", help: "Optional CSV of body id, name, triangles, volume, open edges." },
    ],
};

fn run(inputs) {
    let problems = [];
    for e in errors() { problems.push("feature " + e.feature + " (" + e.name + "): " + e.error); }
    let bodies = scene().bodies;
    if bodies.len() < inputs.min_bodies { problems.push("expected at least " + inputs.min_bodies + " bodies, found " + bodies.len()); }
    for b in bodies {
        if b.open_edges > 0 { problems.push("body " + b.id + " (" + b.name + ") has " + b.open_edges + " open edges"); }
    }
    if inputs.report != "" {
        let rows = [["id", "name", "triangles", "volume", "open_edges"]];
        for b in bodies { rows.push([b.id, b.name, b.triangles, b.volume, b.open_edges]); }
        write_csv(inputs.report, rows);
    }
    for p in problems { log(p); }
    if problems.len() > 0 { fail(problems.len() + " problem(s) found; see the log"); }
    log("ok: " + bodies.len() + " bodies, no errors");
    #{ bodies: bodies.len(), problems: 0 }
}
