//! Persistent provenance must survive reordering, or fail instead of misbinding.
use fr_core::{Session, FeatureKind, api::execute, exact, tag::{Tag,Origin,Kind,Level}};
use glam::DVec3;
use serde_json::{json,Value};
fn cmd(s:&mut Session,c:Value)->Value {execute(s,&c,None).unwrap_or_else(|e|panic!("{c}: {e}"))}
fn block()->Session {let mut s=Session::default();cmd(&mut s,json!({"op":"primitive","type":"box","width":40,"depth":20,"height":10}));s}
fn resolve(s:&Session,body:u32,tag:Tag,p:DVec3)->Result<(exact::FaceInfo,Level),String>{let b=s.built.body(body).unwrap();exact::resolve_face(&b.solids,&b.tags,&exact::FacePick {points:[p,p],tag:Some(tag)})}
#[test]
fn primitive_roles_survive_resizing_rotation_and_lump_order() {
    let mut s=block(); let body=s.built.bodies[0].id;
    let b=s.built.body(body).unwrap(); let old=exact::face_tag_at(&b.solids,&b.tags,DVec3::new(40.,10.,5.)).unwrap();
    assert!(matches!(&old.origin,Origin::Semantic {role,..} if role=="box:x:max"));
    cmd(&mut s,json!({"op":"edit_feature","feature":body,"width":70,"rotate":[0,0,90]}));
    let (face,level)=resolve(&s,body,old,DVec3::ZERO).unwrap();
    assert_eq!(level,Level::Tag); assert!((face.at.y-70.).abs()<1e-6);
    // The fallback identity for unclassified tools also never uses lump ordinals.
    let b=&s.built.bodies[0]; let mut two=b.solids.clone();two.extend(exact::place(b.solids.clone(),&fr_core::exact::Place::Shift(DVec3::X*100.)));
    let first=exact::fresh_tags(&two,99);two.reverse();let reversed=exact::fresh_tags(&two,99);
    assert_eq!(first[0],reversed[1]);assert_eq!(first[1],reversed[0]);
}
#[test]
fn split_pieces_keep_boundary_identity_when_the_cut_moves() {
    let mut s=block(); let body=s.built.bodies[0].id;
    cmd(&mut s,json!({"op":"primitive","type":"box","width":2,"depth":22,"height":6,"position":[19,-1,6],"operation":"cut"}));
    let cut=s.doc.features.last().unwrap().id;
    let b=s.built.body(body).unwrap();
    let left=exact::face_tag_at(&b.solids,&b.tags,DVec3::new(5.,10.,10.)).unwrap();
    let right=exact::face_tag_at(&b.solids,&b.tags,DVec3::new(35.,10.,10.)).unwrap();
    assert_ne!(left,right);assert!(matches!(left.origin,Origin::Patch {..}));
    cmd(&mut s,json!({"op":"edit_feature","feature":cut,"position":[25,-1,6]}));
    // A deliberately wrong location cannot override the known left-piece identity.
    let (face,level)=resolve(&s,body,left,DVec3::new(38.,10.,10.)).unwrap();
    assert_eq!(level,Level::Tag);assert!(face.at.x<25.,"{:?}",face.at);
    let (face,_)=resolve(&s,body,right,DVec3::new(2.,10.,10.)).unwrap();assert!(face.at.x>27.);
}
#[test]
fn a_split_pick_in_the_removed_region_is_ambiguous_not_nearest() {
    let mut s=block();let body=s.built.bodies[0].id;
    let b=s.built.body(body).unwrap();let tag=exact::face_tag_at(&b.solids,&b.tags,DVec3::new(20.,10.,10.)).unwrap();
    cmd(&mut s,json!({"op":"primitive","type":"box","width":2,"depth":22,"height":6,"position":[19,-1,6],"operation":"cut"}));
    let err=resolve(&s,body,tag,DVec3::new(20.,10.,10.)).err().unwrap();assert!(err.contains("ambiguous"),"{err}");
}
#[test]
fn generated_fillet_face_keeps_its_source_edge_when_other_edges_are_added() {
    let mut s=block();let body=s.built.bodies[0].id;
    let p=DVec3::new(40.,10.,10.);let other=DVec3::new(0.,10.,10.);
    let b=s.built.body(body).unwrap();let edge=exact::edge_tag_at(&b.solids,&b.tags,p);let other_edge=exact::edge_tag_at(&b.solids,&b.tags,other);
    cmd(&mut s,json!({"op":"fillet_edges","body":body,"edges":[p.to_array()],"radius":1}));
    let blend=s.doc.features.last().unwrap().id;
    let b=s.built.body(body).unwrap();let face=exact::faces_tagged(&b.solids,&b.tags).into_iter().find(|f|f.kind=="cylinder").unwrap();
    let tag=face.tag.unwrap();assert!(matches!(tag.origin,Origin::Derived {..}));
    let FeatureKind::Blend(f)=&mut s.doc.feature_mut(blend).unwrap().kind else {panic!()};
    f.edges=vec![other,p];f.tags=vec![other_edge,edge];s.rebuild();assert!(s.built.errors.is_empty(),"{:?}",s.built.errors);
    let (found,level)=resolve(&s,body,tag,DVec3::ZERO).unwrap();assert_eq!(level,Level::Tag);assert!(found.at.x>39.);
}
#[test]
fn legacy_ordinal_and_ambiguous_pattern_tags_migrate_only_at_a_unique_saved_point() {
    let mut s=block();let body=s.built.bodies[0].id;
    let mut old=Tag::new(Origin::Made {feature:body,n:999},Kind::Plane);old.schema=0;
    let p=DVec3::new(40.,10.,5.);let (face,level)=resolve(&s,body,old.clone(),p).unwrap();assert_eq!(level,Level::Position);assert!(!face.tag.unwrap().legacy());
    cmd(&mut s,json!({"op":"edit_feature","feature":body,"width":70}));
    assert!(resolve(&s,body,old,p).is_err(),"legacy ordinal must not jump to resized/replacement face");
    cmd(&mut s,json!({"op":"pattern","feature":body,"type":"linear","axis":"x","count":2,"spacing":100}));
    let b=&s.built.bodies[1]; let at=DVec3::new(135.,10.,10.);let current=exact::face_tag_at(&b.solids,&b.tags,at).unwrap();
    let mut old=current.root().clone();old.schema=0;let mut legacy=old.copy(0);legacy.schema=0;
    let (found,level)=resolve(&s,b.id,legacy.clone(),at).unwrap();assert_eq!(level,Level::Position);assert_eq!(found.tag,Some(current));
    let mut doubled=b.solids.clone();doubled.extend(b.solids.clone());let mut tags=b.tags.clone();tags.extend(b.tags.clone());
    assert!(exact::resolve_face(&doubled,&tags,&exact::FacePick {points:[at,at],tag:Some(legacy)}).is_err());
}
#[test]
fn disconnected_profile_caps_keep_sketch_identity_when_selection_order_changes() {
    let mut s=Session::default();cmd(&mut s,json!({"op":"create_sketch","plane":"XY"}));
    cmd(&mut s,json!({"op":"add_geometry","items":[{"type":"rect","from":[0,0],"to":[10,10]},{"type":"rect","from":[20,0],"to":[30,10]}]}));
    cmd(&mut s,json!({"op":"extrude","distance":5,"operation":"new"}));
    let body=s.built.bodies[0].id;let b=s.built.body(body).unwrap();
    let tag=exact::face_tag_at(&b.solids,&b.tags,DVec3::new(5.,5.,5.)).unwrap();
    assert!(matches!(tag.origin,Origin::ProfileCap {..}));
    let FeatureKind::Extrude(e)=&mut s.doc.feature_mut(body).unwrap().kind else {panic!()};e.profiles.reverse();s.rebuild();
    let (f,level)=resolve(&s,body,tag,DVec3::new(25.,5.,5.)).unwrap();assert_eq!(level,Level::Tag);assert!(f.at.x<10.);
}
#[test]
fn a_removed_generated_fillet_face_does_not_rebind_to_another_blend() {
    let mut s=block();let body=s.built.bodies[0].id;
    let p=DVec3::new(40.,10.,10.);let other=DVec3::new(0.,10.,10.);
    let b=s.built.body(body).unwrap();let other_edge=exact::edge_tag_at(&b.solids,&b.tags,other);
    cmd(&mut s,json!({"op":"fillet_edges","body":body,"edges":[p.to_array()],"radius":1}));
    let blend=s.doc.features.last().unwrap().id;let b=s.built.body(body).unwrap();
    let tag=exact::faces_tagged(&b.solids,&b.tags).into_iter().find(|f|f.kind=="cylinder").unwrap().tag.unwrap();
    let FeatureKind::Blend(f)=&mut s.doc.feature_mut(blend).unwrap().kind else {panic!()};f.edges=vec![other];f.tags=vec![other_edge];s.rebuild();
    assert!(s.built.errors.is_empty());assert!(resolve(&s,body,tag,DVec3::new(0.,10.,9.5)).is_err());
}
#[test]
fn fillet_generated_face_survives_radius_and_base_dimensions_changing() {
    let mut s=block();let body=s.built.bodies[0].id;
    cmd(&mut s,json!({"op":"fillet_edges","body":body,"edges":[[40,10,10]],"radius":1}));
    let blend=s.doc.features.last().unwrap().id;let b=s.built.body(body).unwrap();
    let tag=exact::faces_tagged(&b.solids,&b.tags).into_iter().find(|f|f.kind=="cylinder").unwrap().tag.unwrap();
    let FeatureKind::Blend(f)=&mut s.doc.feature_mut(blend).unwrap().kind else {panic!()};f.size=fr_core::Value {expr:"2 mm".into(),v:2.};
    cmd(&mut s,json!({"op":"edit_feature","feature":body,"width":60,"depth":30}));
    assert!(s.built.errors.is_empty(),"{:?}",s.built.errors);
    let (found,level)=resolve(&s,body,tag,DVec3::ZERO).unwrap();assert_eq!(level,Level::Tag);assert!(found.at.x>58.);
}
#[test]
fn shell_inside_face_survives_wall_thickness_and_box_size_edits() {
    let mut s=block();let body=s.built.bodies[0].id;
    cmd(&mut s,json!({"op":"shell","body":body,"open_faces":[[20,10,10]],"thickness":1}));
    let shell=s.doc.features.last().unwrap().id;let b=s.built.body(body).unwrap();
    let tag=exact::face_tag_at(&b.solids,&b.tags,DVec3::new(39.,10.,5.)).unwrap();
    assert!(matches!(tag.origin,Origin::Derived {..}),"{tag:?}");
    let FeatureKind::Shell(f)=&mut s.doc.feature_mut(shell).unwrap().kind else {panic!()};f.thickness=fr_core::Value {expr:"2 mm".into(),v:2.};
    cmd(&mut s,json!({"op":"edit_feature","feature":body,"width":60}));
    let (found,level)=resolve(&s,body,tag,DVec3::ZERO).unwrap();assert_eq!(level,Level::Tag);assert!((found.at.x-58.).abs()<1e-6);
}
#[test]
fn text_faces_survive_size_and_depth_edits_but_not_replaced_letters() {
    let mut s=Session::default();cmd(&mut s,json!({"op":"text","text":"B","operation":"new","height":10,"depth":2}));
    let body=s.built.bodies[0].id;let b=s.built.body(body).unwrap();
    let tag=exact::faces_tagged(&b.solids,&b.tags).into_iter().find(|f|f.normal.z>0.9).unwrap().tag.unwrap();
    assert!(matches!(tag.origin,Origin::Semantic {..}),"{tag:?}");
    cmd(&mut s,json!({"op":"edit_feature","feature":body,"height":80,"depth":4}));
    let (found,level)=resolve(&s,body,tag.clone(),DVec3::ZERO).unwrap();assert_eq!(level,Level::Tag);assert!((found.at.z-4.).abs()<1e-6);
    cmd(&mut s,json!({"op":"edit_feature","feature":body,"text":"A"}));assert!(resolve(&s,body,tag,DVec3::ZERO).is_err());
}
#[test]
fn drill_barrel_survives_diameter_and_depth_edits() {
    let mut s=block();let body=s.built.bodies[0].id;
    cmd(&mut s,json!({"op":"hole","body":body,"at":[20,10,10],"diameter":4,"depth":5}));
    let hole=s.doc.features.last().unwrap().id;let b=s.built.body(body).unwrap();
    let tag=exact::face_tag_at(&b.solids,&b.tags,DVec3::new(22.,10.,8.)).unwrap();assert!(matches!(tag.origin,Origin::Semantic {..}),"{tag:?}");
    let FeatureKind::Hole(h)=&mut s.doc.feature_mut(hole).unwrap().kind else {panic!()};h.diameter=Some(fr_core::Value {expr:"6 mm".into(),v:6.});h.depth=Some(fr_core::Value {expr:"7 mm".into(),v:7.});s.rebuild();
    assert!(s.built.errors.is_empty(),"{:?}",s.built.errors);let (found,level)=resolve(&s,body,tag,DVec3::ZERO).unwrap();assert_eq!(level,Level::Tag);assert_eq!(found.kind,"cylinder");
}
#[test]
fn fillet_corner_patch_keeps_three_source_faces_when_radius_changes() {
    let mut s=block();let body=s.built.bodies[0].id;
    cmd(&mut s,json!({"op":"fillet_edges","body":body,"edges":"all","radius":1}));
    let blend=s.doc.features.last().unwrap().id;let b=s.built.body(body).unwrap();
    let corner=exact::faces_tagged(&b.solids,&b.tags).into_iter().find(|f|f.kind=="sphere").unwrap();
    let tag=corner.tag.unwrap();assert!(matches!(tag.origin,Origin::Derived {..}),"{tag:?}");
    let FeatureKind::Blend(f)=&mut s.doc.feature_mut(blend).unwrap().kind else {panic!()};f.size=fr_core::Value {expr:"2 mm".into(),v:2.};s.rebuild();
    assert!(s.built.errors.is_empty(),"{:?}",s.built.errors);assert_eq!(resolve(&s,body,tag,corner.at).unwrap().1,Level::Tag);
}
#[test]
fn freeform_sweep_retains_provenance_through_boolean_trim_and_clean() {
    let mut s=Session::default();cmd(&mut s,json!({"op":"create_sketch","plane":"XY"}));
    cmd(&mut s,json!({"op":"add_geometry","items":[{"type":"spline","points":[[0,0],[7,-3],[14,-3],[20,0]]},{"type":"line","from":[20,0],"to":[20,10]},{"type":"line","from":[20,10],"to":[0,10]},{"type":"line","from":[0,10],"to":[0,0]}]}));
    cmd(&mut s,json!({"op":"extrude","distance":10,"operation":"new"}));
    let body=s.built.bodies[0].id;let b=s.built.body(body).unwrap();
    let face=exact::faces_tagged(&b.solids,&b.tags).into_iter().find(|f|f.kind=="freeform").unwrap();
    let tag=face.tag.unwrap();assert!(matches!(tag.origin,Origin::Swept {..}),"{tag:?}");
    cmd(&mut s,json!({"op":"primitive","type":"box","width":2,"depth":30,"height":12,"position":[15,-10,-1],"operation":"cut"}));
    let cut=s.doc.features.last().unwrap().id;
    let (found,_) = resolve(&s,body,tag,DVec3::new(7.,-3.,5.)).unwrap();let learned=found.tag.unwrap();
    cmd(&mut s,json!({"op":"edit_feature","feature":cut,"position":[16,-10,-1]}));
    assert_eq!(resolve(&s,body,learned,DVec3::new(7.,-3.,5.)).unwrap().1,Level::Tag);
}
#[test]
fn boolean_join_provenance_does_not_depend_on_operand_order() {
    let mut s=block();cmd(&mut s,json!({"op":"primitive","type":"box","width":20,"depth":20,"height":10,"position":[30,0,0],"operation":"new"}));
    let a=&s.built.bodies[0];let b=&s.built.bodies[1];
    let (ab,at)=exact::boolean_tagged((&a.solids,&a.tags),(&b.solids,&b.tags),fr_core::csg::Bool::Union,99).unwrap();
    let (ba,bt)=exact::boolean_tagged((&b.solids,&b.tags),(&a.solids,&a.tags),fr_core::csg::Bool::Union,99).unwrap();
    let first:std::collections::BTreeSet<_>=at.iter().flatten().flatten().cloned().collect();
    let second:std::collections::BTreeSet<_>=bt.iter().flatten().flatten().cloned().collect();assert_eq!(first,second);
    let tag=exact::face_tag_at(&ab,&at,DVec3::new(20.,10.,10.)).unwrap();assert!(matches!(tag.origin,Origin::Merged {..}),"{tag:?}");
    assert_eq!(exact::resolve_face(&ba,&bt,&exact::FacePick {points:[DVec3::ZERO;2],tag:Some(tag)}).unwrap().1,Level::Tag);
}

#[test]
fn blending_and_shelling_another_lump_preserve_freeform_identity() {
    let mut s=Session::default();cmd(&mut s,json!({"op":"create_sketch","plane":"XY"}));
    cmd(&mut s,json!({"op":"add_geometry","items":[{"type":"spline","points":[[0,0],[7,-3],[14,-3],[20,0]]},{"type":"line","from":[20,0],"to":[20,10]},{"type":"line","from":[20,10],"to":[0,10]},{"type":"line","from":[0,10],"to":[0,0]}]}));
    cmd(&mut s,json!({"op":"extrude","distance":10,"operation":"new"}));
    cmd(&mut s,json!({"op":"primitive","type":"box","width":20,"depth":20,"height":10,"position":[30,0,0],"operation":"new"}));
    let lumps:Vec<_>=s.built.bodies.iter().flat_map(|b|b.solids.clone()).collect();
    let tags:Vec<_>=s.built.bodies.iter().flat_map(|b|b.tags.clone()).collect();
    let p=DVec3::new(40.,0.,10.);let edge=exact::EdgePick {points:[p,p],tag:exact::edge_tag_at(&lumps,&tags,p)};
    let blend=exact::blend(&lumps,&tags,&[edge],1.,false,99).unwrap();assert_eq!(blend.tags[0],tags[0]);
    let p=DVec3::new(40.,10.,10.);let face=exact::FacePick {points:[p,p],tag:exact::face_tag_at(&lumps,&tags,p)};
    let shell=exact::shell(&lumps,&tags,&[face],1.,99).unwrap();assert_eq!(shell.tags[0],tags[0]);
}

#[test]
fn complementary_torus_patches_use_oriented_source_cycles() {
    let mut s=Session::default();cmd(&mut s,json!({"op":"create_sketch","plane":"XZ"}));
    let sketch=s.doc.features.last().unwrap().id;
    cmd(&mut s,json!({"op":"add_geometry","items":[{"type":"line","from":[6,0],"to":[8,0]},{"type":"arc3","start":[8,0],"through":[10,2],"end":[8,4]},{"type":"line","from":[8,4],"to":[6,4]},{"type":"line","from":[6,4],"to":[6,0]}]}));
    cmd(&mut s,json!({"op":"revolve","axis":"y","angle":360,"operation":"new"}));let body=s.built.bodies[0].id;
    cmd(&mut s,json!({"op":"primitive","type":"cylinder","diameter":4,"height":6,"position":[9.5,0,-1],"operation":"cut"}));let cut=s.doc.features.last().unwrap().id;
    cmd(&mut s,json!({"op":"pattern","feature":cut,"type":"circular","axis":"z","count":2,"angle":30}));
    let b=s.built.body(body).unwrap();let faces:Vec<_>=exact::faces_tagged(&b.solids,&b.tags).into_iter().filter(|f|f.kind=="torus").collect();
    assert!(faces.len()>=2,"expected complementary torus patches, got {}",faces.len());
    for f in &faces {assert!(!matches!(f.tag.as_ref().unwrap().origin,Origin::Surface {..}),"{:?}",f.tag);}
    assert!(faces.iter().any(|f|matches!(&f.tag.as_ref().unwrap().origin,Origin::Patch {cycles,..} if !cycles.is_empty())));
    let FeatureKind::Sketch(sk)=&mut s.doc.feature_mut(sketch).unwrap().kind else {panic!()};
    for point in sk.points.values_mut() {*point*=1.2;point.y*=1.05;}
    cmd(&mut s,json!({"op":"edit_feature","feature":cut,"diameter":4.8,"height":8,"position":[11.4,0,-1.2]}));
    assert!(s.built.errors.is_empty(),"{:?}",s.built.errors);
    for f in faces {assert_eq!(resolve(&s,body,f.tag.unwrap(),DVec3::ZERO).unwrap().1,Level::Tag);}
}

#[test]
fn removing_a_named_split_patch_never_selects_its_surviving_sibling() {
    let mut s=block();let body=s.built.bodies[0].id;
    cmd(&mut s,json!({"op":"primitive","type":"box","width":2,"depth":22,"height":6,"position":[19,-1,6],"operation":"cut"}));
    let b=s.built.body(body).unwrap();let left=exact::face_tag_at(&b.solids,&b.tags,DVec3::new(5.,10.,10.)).unwrap();
    assert!(matches!(left.origin,Origin::Patch {..}));
    cmd(&mut s,json!({"op":"primitive","type":"box","width":23,"depth":22,"height":12,"position":[-1,-1,-1],"operation":"cut"}));
    assert!(s.built.errors.is_empty(),"{:?}",s.built.errors);
    assert!(resolve(&s,body,left,DVec3::new(30.,10.,10.)).is_err(),"a deleted left patch must not bind to the surviving right patch");
}
