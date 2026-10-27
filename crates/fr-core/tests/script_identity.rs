//! Script output identity must survive order changes without rebinding later features.
use fr_core::{Session, api, script::{self, Request}};
use serde_json::json;

fn source(order: &[(&str, i32)]) -> String {
    let commands: String = order.iter().map(|(key, x)| format!(
        r#"primitive(#{{output_key:"{key}",type:"box",width:10,depth:10,height:10,position:[{x},0,0]}});"#)).collect();
    format!("const META = #{{name:\"identity\"}}; fn run(i) {{ {commands} }}")
}
fn rerun(s: &Session, source: String) -> Session {
    let mut next = s.fork();
    let previous = next.doc.features.clone();
    next.doc.features.clear();
    next.doc.reuse_feature_ids(&previous);
    next.rebuild();
    script::run(&mut next, &Request::new(source)).unwrap();
    next
}

#[test]
fn keyed_outputs_keep_identity_after_reordering_and_save_reload() {
    let mut first = Session::default();
    script::run(&mut first, &Request::new(source(&[("left",0),("right",30)]))).unwrap();
    let left = first.doc.features[0].id;
    let right = first.doc.features[1].id;
    let saved = fr_core::io::to_json(&first.doc);
    let restored = Session::new(fr_core::io::from_json(&saved).unwrap());
    let mut second = rerun(&restored, source(&[("right",30),("left",0)]));
    assert_eq!(second.doc.features[0].id,right);
    assert_eq!(second.doc.features[1].id,left);
    api::execute(&mut second,&json!({"op":"transform","body":left,"translate":[0,0,20]}),None).unwrap();
    assert_eq!(second.built.body(left).unwrap().mesh.bbox().unwrap().0.z,20.0);
    assert_eq!(second.built.body(right).unwrap().mesh.bbox().unwrap().0.z,0.0);
    assert!(second.built.errors.is_empty());
}

#[test]
fn loop_outputs_require_unique_keys_instead_of_using_iteration_order() {
    let good = r#"const META=#{name:"loop",inputs:[#{name:"reverse",kind:"bool",initial:false}]};
    fn run(i) {let keys=if i.reverse { ["b","a"] } else { ["a","b"] };
      for k in keys { primitive(#{output_key:k,type:"box",width:10,depth:10,height:10,position:if k=="a" {[0,0,0]} else {[30,0,0]}});}}
    "#;
    let mut first=Session::default();script::run(&mut first,&Request::new(good)).unwrap();
    let previous=first.doc.features.clone();let a=previous[0].id;let b=previous[1].id;
    first.doc.features.clear();first.doc.reuse_feature_ids(&previous);first.rebuild();
    let mut req=Request::new(good);req.inputs=json!({"reverse":true});script::run(&mut first,&req).unwrap();
    assert_eq!(first.doc.features.iter().map(|f|f.id).collect::<Vec<_>>(),[b,a]);
    let bad=good.replace("output_key:k,","");
    let mut s=Session::default();let before=s.doc.clone();
    let err=api::execute(&mut s,&json!({"op":"run_script","source":bad}),None).unwrap_err();
    assert!(err.contains("output_key"),"{err}");assert_eq!(s.doc,before);
}

#[test]
fn duplicate_keys_abort_without_overwriting_the_design() {
    let mut s=Session::default();api::execute(&mut s,&json!({"op":"primitive","type":"sphere","diameter":4}),None).unwrap();
    let before=s.doc.clone();let err=api::execute(&mut s,&json!({"op":"run_script","source":source(&[("same",0),("same",30)])}),None).unwrap_err();
    assert!(err.contains("output_key"),"{err}");assert_eq!(s.doc,before);
}

#[test]
fn legacy_unkeyed_outputs_and_changed_call_sites_are_not_guessed() {
    let code=r#"const META=#{name:"old"};fn run(i){primitive(#{type:"box",width:10,depth:10,height:10});}"#;
    let mut old=Session::default();script::run(&mut old,&Request::new(code)).unwrap();
    let id=old.doc.features[0].id;
    let same=rerun(&old,code.into());assert_eq!(same.doc.features[0].id,id);
    let changed=rerun(&old,code.replace("width:10","width:20"));assert_ne!(changed.doc.features[0].id,id);
    old.doc.features[0].script_key=None;
    let migrated=rerun(&old,code.into());assert_ne!(migrated.doc.features[0].id,id);
}
