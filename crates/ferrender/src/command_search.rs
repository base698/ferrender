//! Contextual command search. Palette input never reaches the modeling tools.
use egui::{Context, Key, Modifiers};
use fr_core::{CKind, FeatureKind, Op};
use crate::app::{Action, App, Dialog, Drag, Tool};

#[derive(Default)]
pub struct CommandSearch {
    pub open: bool,
    pub query: String,
    pub selected: usize,
    /// Also blocks the frame which accepts or dismisses the palette.
    pub block_input: bool,
    focus: bool,
    opening_frame: Option<u64>,
    blocked_frame: Option<u64>,
    commands: Vec<Command>,
    revision: Option<u64>,
}

impl CommandSearch {
    pub fn begin_frame(&mut self, ctx:&Context) {
        self.block_input=self.open || self.blocked_frame==Some(ctx.cumulative_frame_nr());
    }
}

#[derive(Clone)]
pub struct Command {
    pub title: &'static str,
    pub action: Action,
    pub shortcut: &'static str,
    pub hint: &'static str,
    pub unavailable: Option<&'static str>,
    aliases: &'static str,
}

pub fn can_open(app: &App) -> bool {
    app.dialog == Dialog::None && app.value_edit.is_none() && app.rename.is_none()
        && app.reference_editor.sketch_id().is_none() && app.file_error.is_none()
        && !app.show_about && app.recover.is_empty() && !app.show_params
        && app.drag == Drag::None && !app.reference_drag.is_dragging() && !app.gizmo.is_active()
        && app.timeline.preview.is_none() && !app.timeline.busy()
}

pub fn open(app: &mut App, ctx: &Context) {
    if !can_open(app) { return; }
    egui::Popup::close_all(ctx);
    let commands=commands(app);
    app.command_search = CommandSearch { open: true, block_input: true, focus: true, opening_frame:Some(ctx.cumulative_frame_nr()),blocked_frame:Some(ctx.cumulative_frame_nr()),commands,revision:Some(app.session.rev), ..Default::default() };
}

/// Consume the printable event paired with an unmodified shortcut key.
pub fn consume_shortcut_text(ctx: &Context, letter: char) {
    ctx.input_mut(|i| i.events.retain(|event| !matches!(event,egui::Event::Text(text) if text.eq_ignore_ascii_case(&letter.to_string()))));
}

pub fn commands(app: &App) -> Vec<Command> {
    let mut list=Vec::new();
    let mut add=|title,action,shortcut,hint,aliases,unavailable| list.push(Command {title,action,shortcut,hint,aliases,unavailable});
    let sketch=app.sketch();
    let bodies=!app.session.built.bodies.is_empty();
    let exact=app.session.built.bodies.iter().any(|b|b.is_exact());
    let need_body=(!bodies).then_some("Create a body first.");
    let need_exact=(!exact).then_some("Create an exact solid first.");
    let selected=!app.sel.is_empty();
    add("Undo",Action::Undo,"","Undo the last change.","edit history",(!app.session.can_undo()).then_some("There is nothing to undo."));
    add("Redo",Action::Redo,"","Redo the last undone change.","edit history",(!app.session.can_redo()).then_some("There is nothing to redo."));
    add("New Sketch",Action::NewSketch,"","Draw on a plane or flat face.","create sketch drawing",None);
    add("Text / Emboss",Action::Text,"",if sketch.is_some(){"Creates solid lettering; finishes the current sketch."}else{"Create solid lettering, or emboss or engrave a flat face."},"text font lettering letters emboss engrave label",None);
    let profiles=app.doc().sketches().any(|(f,s)|!f.suppressed && app.session.built.sketch_plane(app.doc(),f.id).is_some() && !fr_core::profile::profiles(s).is_empty());
    add("Extrude",Action::Extrude,"E","Pull a closed profile or flat face into a solid.","model extrude pad pocket",(!(profiles || app.sel_face.as_ref().is_some_and(|f|f.plane.is_some()))).then_some("Draw a closed profile or select a flat face first."));
    add("Revolve",Action::Revolve,"","Turn a closed profile around an axis.","model revolve lathe",(!profiles).then_some("Draw a closed profile first."));
    if let Some((_,sk))=sketch {
        add("Finish Sketch",Action::FinishSketch,"","Return to the model.","exit close sketch done",None);
        for (title,tool,key,hint) in [
            ("Select",Tool::Select,"V","Select and drag sketch geometry."),
            ("Line",Tool::Line,"L","Draw connected straight lines."),
            ("Rectangle",Tool::Rect,"R","Draw a rectangle from opposite corners."),
            ("Circle",Tool::Circle,"C","Draw a circle from its center and radius."),
            ("Arc",Tool::Arc,"A","Draw an arc using its center and endpoints."),
            ("3-Point Arc",Tool::Arc3,"","Choose two endpoints, then the bulge."),
            ("Tangent Arc",Tool::TangentArc,"","Continue a line or arc smoothly."),
            ("Spline",Tool::Spline,"","Draw a curve through four editable points."),
            ("Point",Tool::Point,"P","Place a sketch point."),
            ("Polygon",Tool::Polygon,"","Draw a regular polygon."),
            ("Dimension",Tool::Dimension,"D","Set a length, diameter, radius or angle."),
            ("Trim",Tool::Trim,"T","Cut an edge back to intersecting geometry."),
        ] {add(title,Action::Tool(tool),key,hint,"sketch draw geometry",None);}
        add("Project Face",Action::Tool(Tool::Project),"","Copy a body's face outline into this sketch.","sketch project geometry",need_body);
        add("Point Coordinates",Action::PointCoordinates,"","Enter exact sketch point coordinates.","sketch x y position",None);
        add("Reference Image",Action::ReferenceImage,"","Import, position or scale a tracing image.","sketch reference image trace picture",None);
        add("Toggle Construction",Action::Construction,"X","Use sketch geometry as a guide.","sketch construction guide",None);
        let selection=(!selected).then_some("Select sketch geometry first.");
        add("Offset",Action::Offset,"O","Make a parallel copy of selected sketch geometry.","sketch offset",selection);
        add("Mirror Sketch",Action::MirrorSketch,"","Select geometry, then Shift-select the mirror line last.","sketch mirror reflection",selection);
        add("Fillet Corner",Action::Fillet,"","Round a selected sketch corner.","sketch fillet round",selection);
        add("Chamfer Corner",Action::Chamfer,"","Cut a selected sketch corner straight.","sketch chamfer bevel",selection);
        add("Cut",Action::Cut,"","Cut selected sketch geometry.","edit clipboard",selection);
        add("Copy",Action::Copy,"","Copy selected sketch geometry.","edit clipboard",selection);
        add("Paste",Action::Paste,"","Paste copied sketch geometry.","edit clipboard",None);
        add("Select All",Action::SelectAll,"","Select all sketch geometry.","edit sketch",None);
        for (title,kind) in [("Coincident",CKind::Coincident),("Horizontal",CKind::Horizontal),("Vertical",CKind::Vertical),("Parallel",CKind::Parallel),("Perpendicular",CKind::Perpendicular),("Tangent",CKind::Tangent),("Equal",CKind::Equal),("Concentric",CKind::Concentric),("Collinear",CKind::Collinear),("Midpoint",CKind::Midpoint),("Symmetric",CKind::Symmetric),("Fix",CKind::Fix)] {
            add(title,Action::Constrain(kind),"",kind.needs(),"sketch constraint lock",sk.normalize(kind,&app.sel).is_err().then_some(kind.needs()));
        }
    } else {
        for (kind,title) in crate::primitives::NAMES.into_iter().enumerate() {add(title,Action::Primitive(kind),"","Create a native solid primitive.","model primitive solid",None);}
        add("New Component",Action::NewComponent,"","Create and activate a component.","model component assembly",None);
        add("Activate Root",Action::ActivateRoot,"","Create subsequent geometry in the root component.","model component root",(app.doc().active_component==0).then_some("Root is already active."));
        add("Construction Plane",Action::Plane,"","Create an offset, midplane or three-point plane.","model construction plane datum offset",None);
        let can_move=match app.sel_component {
            Some(id)=>id!=0 && app.doc().feature(id).is_some_and(|f|matches!(f.kind,FeatureKind::Component(_))),
            None=>app.target_body().is_some() || app.session.built.bodies.len()==1,
        };
        add("Move / Rotate / Scale",Action::Transform,"M","Move the selected body or component.","model move translate rotate scale transform",(!can_move).then_some("Select a body or component first."));
        add("Remove Body",Action::RemoveBody,"","Remove bodies at this history step; keep earlier copies.","model remove body",need_body);
        add("Split Body",Action::SplitBody,"","Divide a solid with a flat face or plane.","model split cut plane",need_exact);
        let two=(app.session.built.bodies.len()<2).then_some("Create at least two bodies first.");
        add("Join Bodies",Action::JoinBodies,"","Join selected bodies into one.","model join union combine",two);
        add("Combine",Action::Combine,"","Join, cut or intersect selected bodies.","model boolean combine cut subtract intersect",two);
        let chosen=app.sel_feature.or(app.sel_body);
        let source=app.doc().features.iter().take(app.doc().active()).any(|f|!f.suppressed && !app.session.built.errors.contains_key(&f.id) && app.session.built.components.contains_key(&f.owner)
            && (Some(f.id)==chosen || f.owner==app.doc().active_component)
            && (matches!(f.kind,FeatureKind::Extrude(_)|FeatureKind::Revolve(_)|FeatureKind::Primitive(_)|FeatureKind::Import(_)) || matches!(&f.kind,FeatureKind::Text(t) if t.op==Op::New)));
        add("Pattern / Mirror",Action::Pattern,"","Repeat a source in a line, grid, circle or mirror.","model pattern array mirror circular linear grid",(!source).then_some("Create a repeatable solid feature first."));
        add("Fillet Edges",Action::Blend(false),"","Round exact solid edges.","model fillet round",need_exact);
        add("Chamfer Edges",Action::Blend(true),"","Bevel exact solid edges.","model chamfer bevel",need_exact);
        add("Shell",Action::Shell,"","Hollow an exact solid.","model shell wall thickness",need_exact);
        add("Hole",Action::Hole,"","Drill a hole into an exact solid.","model hole drill counterbore countersink",need_exact);
        add("Thread",Action::Thread,"","Add threads to a rod or hole.","model thread screw",need_exact);
        add("Measure",Action::Measure,"I","Measure distances and angles.","view measure inspect",need_body);
    }
    let deleting=if sketch.is_some() {selected} else {app.sel_feature.is_some_and(|id|app.doc().feature(id).is_some())};
    add("Delete",Action::Delete,"Del","Delete the selected geometry or feature.","edit delete",(!deleting).then_some("Select geometry or a feature first."));
    for (title,action,hint) in [
        ("New Design",Action::New,"Start a new design."),("Open Design",Action::Open,"Open a saved Ferrender design."),
        ("Save",Action::Save,"Save the current design."),("Save As",Action::SaveAs,"Save the design to another file."),
        ("Recover Unsaved Work",Action::Recover,"Open available recovery copies."),("Import STL",Action::Import,"Import a mesh body."),
        ("Parameters",Action::Parameters,"Edit named dimensions and values."),("Assistant",Action::Assistant,"Open the modeling assistant."),
        ("About Ferrender",Action::About,"Check version and commit information."),
    ] {add(title,action,"",hint,"file edit help",None);}
    add("Export STL",Action::Export,"","Export visible bodies for printing.","file export stl print",need_body);
    add("Export STEP",Action::ExportStep,"","Export visible exact solids.","file export step cad",need_exact);
    for (title,name) in [("Home View","iso"),("Top View","top"),("Front View","front"),("Right View","right"),("Back View","back"),("Left View","left"),("Bottom View","bottom")] {add(title,Action::View(name),"","Change the viewing direction.","view camera",None);}
    add("Fit View",Action::Fit,"F","Fit the design in the view.","view zoom fit",None);
    add("Section Analysis",Action::Section,"","Inspect the inside of a model.","view section cut inspect",need_body);
    list
}

pub fn filtered(app: &App) -> Vec<Command> {
    let query=app.command_search.query.to_lowercase();
    let words:Vec<_>=query.split_whitespace().collect();
    let mut list:Vec<_>=app.command_search.commands.iter().filter(|c| {
        let text=format!("{} {}",c.title.to_lowercase(),c.aliases);
        words.iter().all(|word|text.contains(word))
    }).cloned().collect();
    list.sort_by_key(|c| (c.unavailable.is_some(),!c.title.eq_ignore_ascii_case(query.trim()),!c.title.to_lowercase().starts_with(query.trim())));
    list
}

pub fn show(app: &mut App, ctx: &Context) {
    if !app.command_search.open {
        if app.command_search.block_input {consume_palette_events(ctx);}
        return;
    }
    app.command_search.block_input=true;
    app.command_search.blocked_frame=Some(ctx.cumulative_frame_nr());
    // Selection is frozen by the modal. External document edits can still arrive
    // through the API, so refresh availability when their revision changes.
    if app.command_search.revision!=Some(app.session.rev) {
        app.command_search.commands=commands(app);
        app.command_search.revision=Some(app.session.rev);
        app.command_search.selected=0;
    }
    if app.command_search.opening_frame==Some(ctx.cumulative_frame_nr()) {consume_shortcut_text(ctx,'s');}
    let escape=ctx.input_mut(|i|i.consume_key(Modifiers::NONE,Key::Escape));
    let enter=ctx.input_mut(|i|i.consume_key(Modifiers::NONE,Key::Enter));
    let down=ctx.input_mut(|i|i.consume_key(Modifiers::NONE,Key::ArrowDown));
    let up=ctx.input_mut(|i|i.consume_key(Modifiers::NONE,Key::ArrowUp));
    let mut execute=None;
    let mut close=escape;
    let modal=egui::Modal::new("command-search".into()).show(ctx,|ui| {
        ui.set_width(470.);
        ui.horizontal(|ui| {ui.heading("Search commands");ui.weak(if app.sketch().is_some(){"Sketch"}else{"Model"});});
        let response=ui.add(egui::TextEdit::singleline(&mut app.command_search.query).id(egui::Id::new("command-search-query")).hint_text("Type a command, such as Text or Extrude").desired_width(f32::INFINITY));
        if app.command_search.focus {response.request_focus();app.command_search.focus=false;}
        let query_changed=response.changed();
        if query_changed {app.command_search.selected=0;}
        let results=filtered(app);
        let count=results.iter().take_while(|c|c.unavailable.is_none()).count();
        if count==0 {app.command_search.selected=0;} else {
            app.command_search.selected=app.command_search.selected.min(count-1);
            if down {app.command_search.selected=(app.command_search.selected+1)%count;}
            if up {app.command_search.selected=(app.command_search.selected+count-1)%count;}
        }
        ui.separator();
        if results.is_empty() {ui.label("No matching commands.");}
        egui::ScrollArea::vertical().max_height(355.).show(ui,|ui| {
            for (index,command) in results.iter().enumerate() {
                let selected=index==app.command_search.selected && command.unavailable.is_none();
                let mut button=egui::Button::new(command.title).selected(selected).min_size(egui::vec2(ui.available_width(),28.));
                if !command.shortcut.is_empty() {button=button.shortcut_text(command.shortcut);}
                let response=ui.add_enabled(command.unavailable.is_none(),button);
                if selected && (down || up || query_changed) {response.scroll_to_me(Some(egui::Align::Center));}
                if response.clicked() {execute=Some(command.action);}
                response.on_hover_text(command.unavailable.unwrap_or(command.hint));
                if let Some(reason)=command.unavailable {ui.small(reason);}
            }
        });
        if let Some(command)=results.get(app.command_search.selected).filter(|c|c.unavailable.is_none()) {
            ui.separator();ui.label(command.hint);
            if enter {execute=Some(command.action);}
        }
        ui.weak("Up / Down: choose   Enter: run   Esc: close");
    });
    close|=modal.should_close();
    // TextEdit does not consume Text/clipboard events. Other floating editors
    // and modeling helpers must not see the palette's input later this frame.
    consume_palette_events(ctx);
    if close || execute.is_some() {
        app.command_search.open=false;
        ctx.memory_mut(|m|m.surrender_focus(egui::Id::new("command-search-query")));
    }
    if !close && let Some(action)=execute {app.run(ctx,action);}
}

fn consume_palette_events(ctx:&Context) {
    ctx.input_mut(|i|i.events.retain(|e|!matches!(e,egui::Event::Key{..}|egui::Event::Text(_)|egui::Event::Copy|egui::Event::Cut|egui::Event::Paste(_))));
}
