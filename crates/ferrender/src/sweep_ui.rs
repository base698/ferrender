//! The Sweep dialog's "Along path" control: which parts of the path the sweep
//! covers. A track from the start of the path to its end shows each part as a
//! bar with a handle at either end; below it each part has its two numbers.

use egui::{Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use fr_core::Sketch;
use fr_core::profile::Chain;
use glam::DVec2;

use crate::theme::Palette;

/// The shortest part the control will make, as a fraction of the path.
pub const SHORTEST: f64 = 0.01;
/// Handles land on hundredths of the path.
const STEP: f64 = 0.01;

/// What the control edits: the feature's list, where empty means the whole path.
pub fn shown(spans: &[[f64; 2]]) -> Vec<[f64; 2]> {
    if spans.is_empty() { return vec![[0.0, 1.0]]; }
    let mut list = spans.to_vec();
    list.sort_by(|a, b| a[0].total_cmp(&b[0]));
    list
}

/// What the feature stores for a list the control shows.
pub fn stored(list: &[[f64; 2]]) -> Vec<[f64; 2]> {
    if let [[a, b]] = list && *a <= 1e-9 && *b >= 1.0 - 1e-9 { return Vec::new(); }
    list.to_vec()
}

/// How far the end `k` of part `i` may move: between its own other end and its neighbour.
pub fn limits(list: &[[f64; 2]], i: usize, k: usize) -> (f64, f64) {
    if k == 0 {
        (if i == 0 { 0.0 } else { list[i - 1][1] }, list[i][1] - SHORTEST)
    } else {
        (list[i][0] + SHORTEST, list.get(i + 1).map_or(1.0, |next| next[0]))
    }
}

/// Moves one end of a part, keeping the parts in order, apart and no shorter than [`SHORTEST`].
pub fn set(list: &mut [[f64; 2]], i: usize, k: usize, to: f64) {
    let (lo, hi) = limits(list, i, k);
    if hi < lo { return; }
    list[i][k] = ((to / STEP).round() * STEP).clamp(lo, hi);
}

/// Adds a part in the middle of the widest uncovered stretch. False when there is no room.
pub fn add(list: &mut Vec<[f64; 2]>) -> bool {
    let mut gaps: Vec<(f64, f64)> = Vec::new();
    let mut at = 0.0;
    for [a, b] in list.iter() {
        gaps.push((at, *a));
        at = *b;
    }
    gaps.push((at, 1.0));
    // Of gaps that are equally wide, the one nearest the start of the path.
    let mut widest = (0.0, 0.0);
    for gap in gaps {
        if gap.1 - gap.0 > widest.1 - widest.0 + 1e-9 { widest = gap; }
    }
    let (lo, hi) = widest;
    if hi - lo < 3.0 * SHORTEST { return false; }
    // The middle half of the gap, on the grid.
    let quarter = (hi - lo) / 4.0;
    let (a, b) = (((lo + quarter) / STEP).round() * STEP, ((hi - quarter) / STEP).round() * STEP);
    if b - a < SHORTEST { return false; }
    list.push([a, b]);
    list.sort_by(|x, y| x[0].total_cmp(&y[0]));
    true
}

/// The control. Returns true when it changed the list.
pub fn spans(ui: &mut Ui, spans: &mut Vec<[f64; 2]>, colors: &Palette) -> bool {
    let before = spans.clone();
    let mut list = shown(spans);
    ui.vertical(|ui| {
        // The track: the whole path from left to right.
        let (rect, _) = ui.allocate_exact_size(vec2(230.0, 24.0), Sense::hover());
        let rail = Rect::from_center_size(rect.center(), vec2(rect.width() - 12.0, 6.0));
        let x = |t: f64| rail.left() + t as f32 * rail.width();
        ui.painter().rect_filled(rail, 3.0, colors.disabled.gamma_multiply(0.5));
        for i in 0..list.len() {
            ui.painter().rect_filled(Rect::from_x_y_ranges(x(list[i][0])..=x(list[i][1]), rail.y_range()), 3.0, colors.accent);
            for k in 0..2 {
                let handle = Rect::from_center_size(pos2(x(list[i][k]), rect.center().y), vec2(11.0, 22.0));
                let what = if k == 0 { "start" } else { "end" };
                let grip = ui.interact(handle, ui.id().with(("sweep-part", i, k)), Sense::drag())
                    .on_hover_text(format!("Drag the {what} of part {}: {:.2} of the way along the path", i + 1, list[i][k]));
                if grip.dragged() && let Some(pointer) = grip.interact_pointer_pos() {
                    set(&mut list, i, k, f64::from((pointer.x - rail.left()) / rail.width()));
                }
                let lit = grip.hovered() || grip.dragged();
                let knob = Rect::from_center_size(pos2(x(list[i][k]), rect.center().y), vec2(7.0, if lit { 20.0 } else { 16.0 }));
                ui.painter().rect_filled(knob, 2.0, if lit { colors.ink } else { colors.panel });
                ui.painter().rect_stroke(knob, 2.0, Stroke::new(1.2, if lit { colors.ink } else { colors.accent }), StrokeKind::Inside);
            }
        }
        // The same parts as numbers.
        let mut remove = None;
        let many = list.len() > 1;
        for i in 0..list.len() {
            ui.horizontal(|ui| {
                for k in 0..2 {
                    let (lo, hi) = limits(&list, i, k);
                    let mut value = list[i][k];
                    let name = if k == 0 { "from" } else { "to" };
                    ui.label(RichText::new(name).color(colors.muted));
                    if ui.add(egui::DragValue::new(&mut value).range(lo..=hi.max(lo)).speed(0.005).fixed_decimals(2)).on_hover_text(format!("Part {} {name}: a fraction of the path's length, 0 at its start and 1 at its end", i + 1)).changed() {
                        set(&mut list, i, k, value);
                    }
                }
                if many && ui.small_button("\u{d7}").on_hover_text("Remove this part").clicked() { remove = Some(i); }
            });
        }
        if let Some(i) = remove { list.remove(i); }
        ui.horizontal(|ui| {
            let mut probe = list.clone();
            if ui.add_enabled(add(&mut probe), egui::Button::new("Add part").small()).on_hover_text("Sweep another stretch of the path as well").clicked() { list = probe; }
            let whole = stored(&list).is_empty();
            if ui.add_enabled(!whole, egui::Button::new("Whole path").small()).clicked() { list = vec![[0.0, 1.0]]; }
        });
    });
    *spans = stored(&list);
    *spans != before
}

/// The stretches of a path that the parts cover, as points in sketch coordinates, for drawing.
pub fn covered(sk: &Sketch, chain: &Chain, spans: &[[f64; 2]]) -> Vec<Vec<DVec2>> {
    // The path as one run of points, each piece turned to face the way the path is walked.
    let mut run: Vec<DVec2> = Vec::new();
    for (seg, id) in chain.segs.iter().zip(&chain.ids) {
        let mut pts = sk.polyline(*id);
        let from = seg.ends().0;
        if !matches!(seg, fr_core::profile::Seg::Circle(..)) && pts.first().is_some_and(|p| p.distance(from) > pts.last().unwrap().distance(from)) { pts.reverse(); }
        run.extend(pts);
    }
    let total: f64 = run.windows(2).map(|w| w[0].distance(w[1])).sum();
    if total <= 0.0 { return Vec::new(); }
    shown(spans).into_iter().map(|[a, b]| {
        let (from, to) = (a * total, b * total);
        let mut out = Vec::new();
        let mut at = 0.0;
        for w in run.windows(2) {
            let step = w[0].distance(w[1]);
            let (lo, hi) = (from.max(at), to.min(at + step));
            if step > 0.0 && hi > lo {
                if out.is_empty() { out.push(w[0].lerp(w[1], (lo - at) / step)); }
                out.push(w[0].lerp(w[1], (hi - at) / step));
            }
            at += step;
        }
        out
    }).filter(|part: &Vec<DVec2>| part.len() >= 2).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_whole_path_is_stored_as_nothing_and_shown_as_one_part() {
        assert_eq!(shown(&[]), vec![[0.0, 1.0]]);
        assert!(stored(&[[0.0, 1.0]]).is_empty());
        assert_eq!(stored(&[[0.0, 0.5]]), vec![[0.0, 0.5]]);
        assert_eq!(shown(&[[0.6, 0.7], [0.1, 0.3]]), vec![[0.1, 0.3], [0.6, 0.7]]);
    }

    #[test]
    fn ends_stay_in_order_apart_and_on_hundredths() {
        let mut list = vec![[0.1, 0.3], [0.6, 0.7]];
        set(&mut list, 0, 1, 0.456);
        assert_eq!(list[0], [0.1, 0.46]);
        // An end cannot pass its neighbour or its own other end.
        set(&mut list, 0, 1, 0.9);
        assert_eq!(list[0], [0.1, 0.6]);
        set(&mut list, 1, 0, 0.2);
        assert_eq!(list[1], [0.6, 0.7]);
        set(&mut list, 1, 0, 0.95);
        assert!((list[1][0] - 0.69).abs() < 1e-12);
        set(&mut list, 0, 0, -3.0);
        assert_eq!(list[0][0], 0.0);
        set(&mut list, 1, 1, 7.0);
        assert_eq!(list[1][1], 1.0);
    }

    #[test]
    fn a_new_part_goes_in_the_widest_gap() {
        let mut list = vec![[0.0, 0.2]];
        assert!(add(&mut list));
        assert_eq!(list.len(), 2);
        assert!((list[1][0] - 0.4).abs() < 1e-9 && (list[1][1] - 0.8).abs() < 1e-9, "{list:?}");
        // The whole path has no room for another.
        assert!(!add(&mut vec![[0.0, 1.0]]));
        // From 0.1 to 0.3 and 0.6 to 0.7 the widest gap is 0.3 to 0.6.
        let mut list = vec![[0.1, 0.3], [0.6, 0.7]];
        assert!(add(&mut list));
        assert_eq!(list.len(), 3);
        assert!(list[1][0] > 0.3 && list[1][1] < 0.6, "{list:?}");
    }
}
