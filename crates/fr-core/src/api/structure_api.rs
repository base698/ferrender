use super::*;
use crate::structure::{self, MemberTag};

pub(super) fn set_member(s: &mut Session, c: &J) -> R<J> {
    let id = id_of(c, "body")?;
    if !matches!(
        s.doc.feature(id).map(|f| &f.kind),
        Some(FeatureKind::Extrude(_))
    ) {
        return Err("a member tag must refer to a direct extrusion feature".into());
    }
    let member: MemberTag = serde_json::from_value(c["member"].clone())
        .map_err(|e| format!("invalid structural member: {e}"))?;
    member.validate()?;
    if s.doc.structural_members.len() >= 1024 && !s.doc.structural_members.contains_key(&id) {
        return Err("at most 1024 structural member tags are supported".into());
    }
    s.edit_without_rebuild(|d| {
        d.structural_members.insert(id, member.clone());
        Ok(())
    })?;
    Ok(json!({"body":id,"member":member}))
}

pub(super) fn remove_member(s: &mut Session, c: &J) -> R<J> {
    let id = id_of(c, "body")?;
    let removed = s.edit_without_rebuild(|d| Ok(d.structural_members.remove(&id).is_some()))?;
    Ok(json!({"body":id,"removed":removed}))
}

pub(super) fn members(s: &Session) -> R<J> {
    let mut geometries = Vec::new();
    let members: Vec<_> = s
        .doc
        .structural_members
        .iter()
        .map(|(&id, tag)| match structure::geometry(s, id, tag) {
            Ok(g) => {
                let out = json!({"body":id,"member":tag,"geometry":g});
                geometries.push(g);
                out
            }
            Err(e) => json!({"body":id,"member":tag,"error":e}),
        })
        .collect();
    // Only parallel members with overlapping longitudinal extents have a
    // meaningful spacing here. This is measured centerline separation in 3D,
    // not automatically an effective tributary width or a structural connection.
    let mut spacing = Vec::new();
    if geometries.len() <= 128 {
        for (i, a) in geometries.iter().enumerate() {
            let axis = (a.end_mm - a.start_mm) / a.length_mm;
            for b in geometries.iter().skip(i + 1) {
                let other = (b.end_mm - b.start_mm) / b.length_mm;
                if axis.dot(other).abs() < 1.0 - 1e-8 {
                    continue;
                }
                let first = (b.start_mm - a.start_mm).dot(axis);
                let last = (b.end_mm - a.start_mm).dot(axis);
                if first.min(last) >= a.length_mm || first.max(last) <= 0.0 {
                    continue;
                }
                let delta = b.start_mm - a.start_mm;
                spacing.push(json!({"bodies":[a.body,b.body],"centerline_distance_mm":(delta - axis * delta.dot(axis)).length()}));
            }
        }
    }
    Ok(
        json!({"units":"mm","members":members,"parallel_spacing":spacing,"spacing_omitted":geometries.len() > 128}),
    )
}
