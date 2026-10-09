//! Screen-space geometry capture for drawing tools. Attachment is transient;
//! the returned Snap uses the ordinary saved coincidence constraint path.
use egui::Pos2;
use fr_core::{Geom, Id, Sketch};
use glam::DVec2;
use crate::app::{Snap, Tool};

pub const ACQUIRE_PX: f32 = 10.0;
pub const RELEASE_PX: f32 = 18.0;
pub const POINT_PX: f32 = 8.0;

#[derive(Clone, Copy, Debug)]
pub struct Candidate { pub entity: Id, pub point: DVec2, pub distance: f32 }

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Context {
    pub sketch: Id,
    pub tool: Tool,
    pub revision: u64,
    pub camera: fr_core::render::Camera,
    pub plane: fr_core::Plane,
}

#[derive(Clone, Copy, Debug)]
struct Sample { time: f64, distance: f32, outward_speed: f32 }

#[derive(Default, Debug)]
pub struct Capture {
    context: Option<Context>,
    entity: Option<Id>,
    blocked: Option<Id>,
    sample: Option<Sample>,
    current: Option<Snap>,
}

impl Capture {
    pub fn clear(&mut self) { *self = Self::default(); }
    #[cfg(test)]
    pub fn current(&self) -> Option<Snap> { self.current }

    /// A release is latched until the pointer leaves that entity's release band,
    /// so the next stationary frame cannot immediately reattach after a flick.
    pub fn update(&mut self, context: Context, time: f64, endpoint: Option<(Id,DVec2)>, candidates: &[Candidate], bypass: bool) -> Option<Snap> {
        if self.context != Some(context) { self.clear(); self.context = Some(context); }
        self.current = None;
        if bypass { self.entity = None; self.blocked = None; self.sample = None; return None; }
        if let Some((id,p)) = endpoint {
            self.entity = None; self.sample = None;
            self.current = Some(Snap { p, point: Some(id), on: None, h: false, v: false, axis: [false, false] });
            return self.current;
        }
        if self.blocked.is_some_and(|id| candidates.iter().find(|c| c.entity == id).is_none_or(|c| c.distance > RELEASE_PX)) { self.blocked = None; }
        if let Some(id) = self.entity {
            if let Some(candidate) = candidates.iter().find(|c| c.entity == id) {
                let mut outward_speed = 0.0;
                let flick = self.sample.is_some_and(|last| {
                    let dt = time-last.time;
                    if !(0.001..=0.12).contains(&dt) { return false; }
                    let away = candidate.distance-last.distance;
                    outward_speed = away / dt as f32;
                    let acceleration = (outward_speed-last.outward_speed) / dt as f32;
                    // Following a line quickly must not release it: only a
                    // deliberate movement away, already 8 px off, can break.
                    candidate.distance >= 8.0 && away >= 4.0 && outward_speed >= 450.0 && acceleration >= 12_000.0
                });
                if candidate.distance <= RELEASE_PX && !flick {
                    self.sample = Some(Sample { time, distance: candidate.distance, outward_speed });
                    return self.show(*candidate);
                }
                if flick { self.blocked = Some(id); }
            }
            self.entity = None; self.sample = None;
        }
        let best = candidates.iter().filter(|c| c.distance <= ACQUIRE_PX && Some(c.entity) != self.blocked)
            .min_by(|a,b| a.distance.total_cmp(&b.distance));
        if let Some(best) = best {
            self.entity = Some(best.entity);
            self.sample = Some(Sample { time, distance: best.distance, outward_speed: 0.0 });
            return self.show(*best);
        }
        None
    }

    fn show(&mut self, c: Candidate) -> Option<Snap> {
        self.current = Some(Snap { p: c.point, point: None, on: Some(c.entity), h: false, v: false, axis: [false, false] });
        self.current
    }
}

pub fn nearest_point(sk: &Sketch, cursor: Pos2, project: impl Fn(DVec2)->Pos2) -> Option<(Id,DVec2)> {
    sk.points.iter().map(|(&id,&p)| (id,p,project(p).distance(cursor))).filter(|(_,_,d)| d.is_finite() && *d <= POINT_PX)
        .min_by(|a,b| a.2.total_cmp(&b.2)).map(|(id,p,_)|(id,p))
}

/// Finds the closest *visible extent* of an entity in screen coordinates. The
/// curve minimizer also handles the ellipse seen when a sketch is oblique.
pub fn project_entity(sk: &Sketch, entity: Id, cursor: Pos2, project: impl Fn(DVec2)->Pos2) -> Option<Candidate> {
    let geometry = &sk.entities.get(&entity)?.geom;
    let p = match geometry {
        Geom::Line { a,b } => {
            let (a,b) = (sk.pos(*a),sk.pos(*b));
            let (pa,pb) = (project(a),project(b));
            let d = pb-pa;
            if !d.is_finite() || d.length_sq() < 1e-10 { return None; }
            a+(b-a)*((cursor-pa).dot(d)/d.length_sq()).clamp(0.0,1.0) as f64
        }
        Geom::Circle { .. } | Geom::Arc { .. } => {
            let (center,radius) = sk.curve(entity)?;
            if !radius.is_finite() || radius <= 0.0 { return None; }
            let (start,sweep) = sk.arc_angles(entity).unwrap_or((0.0,std::f64::consts::TAU));
            let point = |t: f64| center+DVec2::from_angle(start+t)*radius;
            let distance = |t: f64| project(point(t)).distance_sq(cursor) as f64;
            let steps = ((sweep/std::f64::consts::TAU*64.0).ceil() as usize).max(8);
            let step = sweep/steps as f64;
            let index = (0..=steps).min_by(|a,b| distance(*a as f64*step).total_cmp(&distance(*b as f64*step)))?;
            let (mut lo,mut hi) = if matches!(geometry,Geom::Circle { .. }) {
                // A full circle has no endpoint at its parameter seam.
                ((index as f64-1.0)*step,(index as f64+1.0)*step)
            } else { (((index as f64-1.0)*step).max(0.0),((index as f64+1.0)*step).min(sweep)) };
            // Refine the sampled minimum; endpoints remain valid candidates.
            for _ in 0..24 {
                let a = lo+(hi-lo)/3.0; let b = hi-(hi-lo)/3.0;
                if distance(a) < distance(b) { hi=b; } else { lo=a; }
            }
            let t = [0.0,sweep,(lo+hi)*0.5].into_iter().min_by(|a,b| distance(*a).total_cmp(&distance(*b)))?;
            point(t)
        }
        // The nearest point of the drawn curve. Fit points are still picked above. The
        // solver has no enduring point-on-spline relation, so the point is placed on
        // the curve but not tied to it; the coincident request is refused and ignored.
        Geom::Spline { .. } => {
            let line = sk.polyline(entity);
            let mut best: Option<(f32, DVec2)> = None;
            for w in line.windows(2) {
                let (pa, pb) = (project(w[0]), project(w[1]));
                let d = pb - pa;
                if !d.is_finite() { continue; }
                let t = if d.length_sq() < 1e-10 { 0.0 } else { ((cursor - pa).dot(d) / d.length_sq()).clamp(0.0, 1.0) };
                let p = w[0] + (w[1] - w[0]) * t as f64;
                let dist = project(p).distance(cursor);
                if best.is_none_or(|b| dist < b.0) { best = Some((dist, p)); }
            }
            best?.1
        }
    };
    let distance = project(p).distance(cursor);
    (p.is_finite() && distance.is_finite()).then_some(Candidate { entity,point:p,distance })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn context() -> Context { Context { sketch:1,tool:Tool::Point,revision:0,camera:Default::default(),plane:fr_core::Plane::XY } }
    fn candidate(distance: f32) -> Candidate { Candidate { entity:7,point:DVec2::new(12.0,0.0),distance } }
    #[test]
    fn capture_has_hysteresis_and_outward_flick_requires_distance_and_acceleration() {
        let mut capture=Capture::default(); let c=context();
        assert_eq!(capture.update(c,0.0,None,&[candidate(9.0)],false).unwrap().on,Some(7));
        assert!(capture.update(c,0.1,None,&[candidate(14.0)],false).is_some());
        assert!(capture.update(c,0.2,None,&[candidate(18.1)],false).is_none());
        assert!(capture.update(c,0.3,None,&[candidate(12.0)],false).is_none());
        assert!(capture.update(c,0.4,None,&[candidate(2.0)],false).is_some());
        assert!(capture.update(c,0.41,None,&[candidate(2.0)],false).is_some());
        assert!(capture.update(c,0.42,None,&[candidate(9.0)],false).is_none());
        assert!(capture.update(c,0.43,None,&[candidate(9.0)],false).is_none(),"a flick must not immediately recapture");
        capture.update(c,0.5,None,&[candidate(20.0)],false);
        assert!(capture.update(c,0.6,None,&[candidate(9.0)],false).is_some());
        assert_eq!(capture.update(c,0.7,Some((9,DVec2::X)),&[candidate(1.0)],false).unwrap().point,Some(9));
        assert!(capture.update(c,0.8,None,&[candidate(1.0)],true).is_none());
    }
    #[test]
    fn spline_projection_lands_on_the_drawn_curve() {
        let mut sk=Sketch::new(fr_core::Plane::XY);
        let pts=[DVec2::new(0.0,0.0),DVec2::new(10.0,15.0),DVec2::new(20.0,-15.0),DVec2::new(30.0,0.0)].map(|p| sk.add_point(p));
        let spline=sk.add_spline(pts,false).unwrap();
        let project=|p:DVec2| Pos2::new((p.x*10.0) as f32,(p.y*10.0) as f32);
        // Just off the curve near its first bend: the candidate sits on the drawn polyline, not on the chord.
        let at=project_entity(&sk,spline,Pos2::new(100.0,170.0),project).unwrap();
        let line=sk.polyline(spline);
        let on_curve=line.windows(2).map(|w| { let d=w[1]-w[0]; let t=((at.point-w[0]).dot(d)/d.length_squared()).clamp(0.0,1.0); (w[0]+d*t).distance(at.point) }).fold(f64::MAX,f64::min);
        assert!(on_curve<1e-9,"{} off the curve",on_curve);
        assert!(at.point.y>10.0,"near the bend the curve is well above the chord: {:?}",at.point);
        assert!(at.distance<30.0);
        // Beyond an end it stops at the end point.
        let at=project_entity(&sk,spline,Pos2::new(400.0,0.0),project).unwrap();
        assert!(at.point.distance(DVec2::new(30.0,0.0))<1e-9);
    }
    #[test]
    fn arc_projection_stays_on_its_extent_and_uses_screen_distance() {
        let mut sk=Sketch::new(fr_core::Plane::XY);
        let c=sk.add_point(DVec2::ZERO); let a=sk.add_point(DVec2::new(10.0,0.0)); let b=sk.add_point(DVec2::new(0.0,10.0));
        let arc=sk.add(Geom::Arc {c,s:a,e:b},false);
        let project=|p:DVec2| Pos2::new((p.x*10.0) as f32,(p.y*3.0) as f32);
        let at=project_entity(&sk,arc,Pos2::new(-20.0,-30.0),project).unwrap();
        assert!(at.point.distance(sk.pos(b)) < 1e-5,"a point beyond the arc must snap to its endpoint, not the absent circle");
        let at=project_entity(&sk,arc,Pos2::new(65.0,25.0),project).unwrap();
        assert!((at.point.length()-10.0).abs()<1e-9);
        assert!(at.point.x>=0.0 && at.point.y>=0.0);
        let circle=sk.add(Geom::Circle {c,r:10.0},false);
        for y in [-0.8,0.8] {
            let near_seam=project_entity(&sk,circle,Pos2::new(100.0,y),|p|Pos2::new((p.x*10.0) as f32,(p.y*10.0) as f32)).unwrap();
            assert!(near_seam.point.y*y as f64>0.0,"capture must pass smoothly through either side of the circle seam");
        }
    }
}
