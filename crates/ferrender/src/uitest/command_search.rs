//! Search and move shortcuts must remain separate from document editing input.
use super::*;
use fr_core::Id;

fn command(h:&mut H,c:serde_json::Value)->serde_json::Value {h.state_mut().execute(&c).unwrap_or_else(|e|panic!("{c}: {e}"))}
fn letter(h:&mut H,key:Key,text:&str) {
    // The host emits the printable character with its key event. Harness::event
    // deliberately gives each queued event a separate frame, so batch this pair.
    h.input_mut().events.push(Event::Key {key,physical_key:None,pressed:true,repeat:false,modifiers:Modifiers::NONE});
    h.input_mut().events.push(Event::Text(text.into()));h.step();
    h.event(Event::Key {key,physical_key:None,pressed:false,repeat:false,modifiers:Modifiers::NONE});h.step();
}
fn search(h:&mut H,text:&str) {
    letter(h,Key::S,"s");h.run_steps(2);
    assert!(h.state().command_search.open,"S should open command search");
    assert!(h.state().command_search.query.is_empty(),"opening S must not become query text: {:?}",h.state().command_search.query);
    h.event(Event::Text(text.into()));h.run_steps(2);
    assert_eq!(h.state().command_search.query,text);
}
fn block(h:&mut H)->Id {command(h,json!({"op":"primitive","type":"box","width":20,"depth":20,"height":20}))["feature"].as_u64().unwrap() as Id}

#[test]
fn search_finds_text_and_emboss_and_enter_does_not_confirm_the_text_dialog() {
    for query in ["text","emboss"] {
        let mut h=state_harness();let before=h.state().doc().clone();
        search(&mut h,query);
        let found=crate::command_search::filtered(h.state());
        assert_eq!(found[0].action,Action::Text);
        assert_eq!(found[0].unavailable,None);
        assert_eq!(h.state().doc(),&before);
        key(&mut h,Key::Enter);
        assert!(!h.state().command_search.open);
        assert!(matches!(h.state().dialog,Dialog::Text(_)),"Enter should open Text, not apply its default lettering");
        assert_eq!(h.state().doc(),&before);
        assert!(h.state().session.built.bodies.is_empty());
    }
}

#[test]
fn sketch_search_is_contextual_and_select_moves_to_v() {
    let mut h=state_harness();h.state_mut().create_sketch(Plane::XY);h.run_steps(2);
    run(&mut h,Action::Tool(Tool::Line));let before=h.state().doc().clone();
    search(&mut h,"circle");
    assert_eq!(h.state().tool,Tool::Line,"opening search must not switch to Select");
    assert_eq!(crate::command_search::filtered(h.state())[0].action,Action::Tool(Tool::Circle));
    assert!(!crate::command_search::commands(h.state()).iter().any(|c|matches!(c.action,Action::Primitive(_))));
    key(&mut h,Key::Enter);assert_eq!(h.state().tool,Tool::Circle);
    assert!(h.state().clicks.is_empty());assert_eq!(h.state().doc(),&before);
    key(&mut h,Key::V);assert_eq!(h.state().tool,Tool::Select);
    search(&mut h,"emboss");
    assert!(crate::command_search::filtered(h.state())[0].hint.contains("finishes"));
    key(&mut h,Key::Enter);assert_eq!(h.state().mode,Mode::Model);
    assert!(matches!(h.state().dialog,Dialog::Text(_)));
    assert_eq!(h.state().doc().features.len(),before.features.len());
}

#[test]
fn arrow_navigation_runs_the_highlighted_command_and_escape_preserves_selection() {
    let mut h=state_harness();let body=block(&mut h);h.state_mut().sel_body=Some(body);
    let before=h.state().doc().clone();search(&mut h,"view");
    let count=crate::command_search::filtered(h.state()).iter().filter(|c|c.unavailable.is_none()).count();
    assert!(count>3);
    key(&mut h,Key::ArrowUp);assert_eq!(h.state().command_search.selected,count-1);
    key(&mut h,Key::ArrowDown);assert_eq!(h.state().command_search.selected,0);
    key(&mut h,Key::ArrowDown);
    let Action::View(view)=crate::command_search::filtered(h.state())[1].action else {panic!("second view must be a camera command")};
    key(&mut h,Key::Enter);
    assert_eq!((h.state().cam.yaw,h.state().cam.pitch),fr_core::render::Camera::named(view).unwrap());
    assert_eq!(h.state().doc(),&before);
    search(&mut h,"remove");key(&mut h,Key::Escape);
    assert!(!h.state().command_search.open);assert_eq!(h.state().sel_body,Some(body));
    assert_eq!(h.state().dialog,Dialog::None);assert_eq!(h.state().doc(),&before);
}

#[test]
fn unavailable_and_unmatched_commands_do_nothing_on_enter() {
    let mut h=state_harness();let before=h.state().doc().clone();
    search(&mut h,"join bodies");
    let found=crate::command_search::filtered(h.state());assert_eq!(found.len(),1);assert!(found[0].unavailable.is_some());
    key(&mut h,Key::Enter);assert!(h.state().command_search.open);assert_eq!(h.state().dialog,Dialog::None);
    h.state_mut().command_search.query="does-not-exist".into();h.run_steps(2);
    assert!(crate::command_search::filtered(h.state()).is_empty());
    key(&mut h,Key::ArrowDown);key(&mut h,Key::Enter);
    assert!(h.state().command_search.open);assert_eq!(h.state().doc(),&before);
    key(&mut h,Key::Escape);assert!(!h.state().command_search.open);
}

#[test]
fn palette_blocks_viewport_clicks_and_drawing_until_dismissed() {
    let mut h=state_harness();h.state_mut().create_sketch(Plane::XY);h.run_steps(2);run(&mut h,Action::Tool(Tool::Line));
    let before=h.state().doc().clone();search(&mut h,"rectangle");
    let outside=h.state().vp.left_bottom()+egui::vec2(35.,-50.);click(&mut h,outside);
    assert!(!h.state().command_search.open,"clicking the backdrop should dismiss the palette");
    assert!(h.state().clicks.is_empty(),"the dismissal click must not place a sketch point");
    assert_eq!(h.state().doc(),&before);assert_eq!(h.state().tool,Tool::Line);
    click(&mut h,outside);assert_eq!(h.state().clicks.len(),1,"normal drawing should resume after dismissal");
}

#[test]
fn s_and_m_are_text_while_typing_and_do_not_replace_an_open_operation() {
    let mut h=state_harness();run(&mut h,Action::Text);h.run_steps(3);
    h.get(egui_kittest::kittest::By::new().role(egui::accesskit::Role::TextInput).value("Text")).click();h.run_steps(2);
    letter(&mut h,Key::S,"s");letter(&mut h,Key::M,"m");
    let Dialog::Text(text)=&h.state().dialog else {panic!("typing letters replaced the text operation")};
    assert!(text.text.contains('s') && text.text.contains('m'));
    assert!(!h.state().command_search.open);
    let pos=h.state().vp.left_bottom()+egui::vec2(40.,-40.);click(&mut h,pos);
    key(&mut h,Key::S);key(&mut h,Key::M);
    assert!(matches!(h.state().dialog,Dialog::Text(_)),"even without a focused text field, shortcuts must not replace an open operation");
    assert!(!h.state().command_search.open);
}

#[test]
fn m_opens_move_for_the_selected_body_or_component() {
    let mut h=state_harness();let body=block(&mut h);h.state_mut().sel_body=Some(body);
    let before=h.state().doc().clone();letter(&mut h,Key::M,"m");
    assert!(matches!(&h.state().dialog,Dialog::Transform(d) if d.body==body));
    assert_eq!(h.state().doc(),&before);run(&mut h,Action::Cancel);h.run_steps(2);
    let component=command(&mut h,json!({"op":"create_component","name":"Movable","activate":true}))["component"].as_u64().unwrap() as Id;
    h.state_mut().sel_component=Some(component);h.state_mut().sel_body=None;h.run_steps(2);
    letter(&mut h,Key::M,"m");
    assert!(matches!(&h.state().dialog,Dialog::MoveComponent(d) if d.component==component));
}

#[test]
fn sketch_menu_exposes_solid_text_and_search_has_a_mouse_entry() {
    let mut h=state_harness();h.state_mut().create_sketch(Plane::XY);h.run_steps(2);
    h.get_by_label("Sketch").click();h.run_steps(2);
    h.get_by_label("Text / Emboss…").click();h.run_steps(2);
    assert_eq!(h.state().mode,Mode::Model);assert!(matches!(h.state().dialog,Dialog::Text(_)));
    run(&mut h,Action::Cancel);h.run_steps(2);
    h.get_by_label("Edit").click();h.run_steps(2);
    h.get_by_label_contains("Search Commands").click();h.run_steps(3);
    assert!(h.state().command_search.open);
    h.event(Event::Text("box".into()));h.run_steps(2);
    h.get_by_label("Box").click();h.run_steps(2);
    assert!(matches!(&h.state().dialog,Dialog::Primitive(d) if d.kind==0));
    assert!(h.state().session.built.bodies.is_empty());
}

#[test]
fn palette_preserves_a_pending_line_and_owns_document_shortcuts() {
    let mut h=state_harness();h.state_mut().create_sketch(Plane::XY);h.run_steps(2);run(&mut h,Action::Tool(Tool::Line));
    let start=at(&h,5.,5.);click(&mut h,start);assert_eq!(h.state().clicks.len(),1);
    let point=h.state().clicks[0].p;let before=h.state().doc().clone();
    h.state_mut().session.path=Some(out_dir().join("palette-shortcut-guard.ferr"));
    assert!(h.state().session.dirty);
    h.get_by_label("Edit").click();h.run_steps(2);h.get_by_label_contains("Search Commands").click();h.run_steps(3);
    assert!(h.state().command_search.open);
    h.event(Event::Text("no matching command 123".into()));h.run_steps(2);
    for key in [Key::S,Key::Z] {
        h.event(Event::ModifiersChanged(Modifiers::COMMAND));
        h.event(Event::Key {key,physical_key:None,pressed:true,repeat:false,modifiers:Modifiers::COMMAND});h.step();
        h.event(Event::Key {key,physical_key:None,pressed:false,repeat:false,modifiers:Modifiers::COMMAND});
        h.event(Event::ModifiersChanged(Modifiers::NONE));h.step();
    }
    assert!(h.state().session.dirty,"Command/Ctrl-S in search must not save the document");
    assert_eq!(h.state().doc(),&before,"Command/Ctrl-Z in search must not undo geometry");
    key(&mut h,Key::Escape);
    assert_eq!(h.state().clicks.len(),1);assert_eq!(h.state().clicks[0].p,point);
    assert!(h.state().typed.as_ref().is_some_and(|t|t.fields.iter().all(String::is_empty)),"palette text must not enter the pending line's size boxes");
    assert_eq!(h.state().doc(),&before);assert_eq!(h.state().tool,Tool::Line);
}

#[test]
fn command_palette_renders_in_light_and_dark_appearance() {
    let mut h=harness();block(&mut h);h.state_mut().fit();h.run_steps(2);
    search(&mut h,"");
    save(&mut h,"command-search-light.png");
    h.state_mut().set_appearance(Appearance::Dark);h.run_steps(2);
    save(&mut h,"command-search-dark.png");
    h.event(Event::Text("emboss".into()));h.run_steps(2);
    assert_eq!(crate::command_search::filtered(h.state())[0].action,Action::Text);
    save(&mut h,"command-search-text-dark.png");
}

#[test]
fn command_availability_respects_root_selection_active_owner_and_sketch_selection() {
    let mut h=state_harness();
    command(&mut h,json!({"op":"create_component","name":"Part","activate":true}));
    let body=block(&mut h);run(&mut h,Action::ActivateRoot);
    h.state_mut().sel_feature=None;h.state_mut().sel_body=None;h.state_mut().sel_component=Some(0);
    let enabled=|h:&H,action|crate::command_search::commands(h.state()).into_iter().find(|c|c.action==action).unwrap().unavailable.is_none();
    assert!(!enabled(&h,Action::Transform),"Root cannot move even if there is exactly one body");
    assert!(!enabled(&h,Action::Pattern),"an empty active Root needs an explicit source selection from another component");
    h.state_mut().sel_component=None;h.state_mut().sel_body=Some(body);
    assert!(enabled(&h,Action::Transform));assert!(enabled(&h,Action::Pattern));
    h.state_mut().create_sketch(Plane::XY);h.run_steps(2);
    let Mode::Sketch(sketch)=h.state().mode else {panic!()};
    h.state_mut().sel_feature=Some(sketch);h.state_mut().sel.clear();
    assert!(!enabled(&h,Action::Delete),"Delete in a sketch cannot delete the selected timeline feature");
}

#[test]
fn open_palette_refreshes_available_commands_after_an_external_document_edit() {
    let mut h=state_harness();search(&mut h,"split body");
    assert!(crate::command_search::filtered(h.state())[0].unavailable.is_some());
    block(&mut h);h.run_steps(2);
    assert!(h.state().command_search.open);
    assert_eq!(crate::command_search::filtered(h.state())[0].unavailable,None);
    key(&mut h,Key::Enter);
    assert!(matches!(h.state().dialog,Dialog::Split(_)));
}
