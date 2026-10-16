// Variants: set a parameter to each value in a list, rebuild, and export an STL per value.
const META = #{
    name: "Export variants",
    description: "Sets one parameter to each value in a list and exports an STL for each.",
    inputs: [
        #{ name: "parameter", kind: "text", initial: "width", help: "The parameter to vary." },
        #{ name: "values", kind: "text", initial: "20 mm, 30 mm, 40 mm", help: "Comma-separated expressions." },
        #{ name: "folder", kind: "folder", initial: "", help: "Where the STLs go; empty means beside the document." },
        #{ name: "stem", kind: "text", initial: "{name}-{parameter}-{value}" },
    ],
};

fn run(inputs) {
    let folder = if inputs.folder == "" { document_dir() } else { inputs.folder };
    let original = params()[inputs.parameter];
    if original == () { fail("there is no parameter called " + inputs.parameter); }
    let values = inputs.values.split(",");
    let done = [];
    for (v, i) in values {
        let value = v;
        value.trim();
        progress(i.to_float() / values.len(), "exporting " + value);
        set_parameter(#{ name: inputs.parameter, expr: value });
        if errors().len() > 0 { fail("the design does not build with " + inputs.parameter + " = " + value + ": " + errors()[0].error); }
        let compact = value;
        compact.replace(" ", "");
        let file = scene().file;
        let name = name_template(inputs.stem, #{ name: if file == () { "design" } else { basename(file) }, parameter: inputs.parameter, value: compact });
        let path = join(folder, name + ".stl");
        export_stl(#{ path: path });
        log("wrote " + path);
        done.push(path);
    }
    set_parameter(#{ name: inputs.parameter, expr: original.expr });
    done
}
