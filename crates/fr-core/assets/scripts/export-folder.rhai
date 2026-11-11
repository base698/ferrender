// Export every design in a folder as STL (and STEP for exact bodies).
const META = #{
    name: "Export a folder",
    description: "Opens every .ferr in a folder and writes an STL (and a STEP) beside it.",
    inputs: [
        #{ name: "folder", kind: "folder", initial: "" },
        #{ name: "step", kind: "bool", initial: true, label: "Also write STEP" },
    ],
};

fn run(inputs) {
    let files = list_files(inputs.folder, ".ferr");
    if files.len() == 0 { fail("no .ferr files in " + inputs.folder); }
    let written = [];
    for (file, i) in files {
        progress(i.to_float() / files.len(), basename(file));
        open(#{ path: file });
        if errors().len() > 0 { log("skipped " + basename(file) + ": " + errors()[0].error); continue; }
        let stl = join(inputs.folder, basename(file) + ".stl");
        export_stl(#{ path: stl });
        written.push(stl);
        if inputs.step && scene().bodies.filter(|b| b.kind == "exact").len() > 0 {
            let step = join(inputs.folder, basename(file) + ".step");
            export_step(#{ path: step });
            written.push(step);
        }
    }
    log("wrote " + written.len() + " files");
    written
}
