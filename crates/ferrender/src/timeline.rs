//! A timeline drag previews a position without editing or rebuilding the document.

use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

use eframe::egui_wgpu::wgpu;
use egui::Context;

#[derive(Default)]
pub struct Timeline {
    /// (feature count, document revision at the start of the gesture).
    pub preview: Option<(usize, u64)>,
    waiting: Option<Completion>,
}

struct Completion {
    painted: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    watching: bool,
}

impl Timeline {
    pub fn busy(&self) -> bool {
        self.waiting.is_some()
    }

    pub fn begin_update(&mut self) {
        self.preview = None;
        self.waiting = Some(Completion { painted: Arc::default(), done: Arc::default(), watching: false });
    }

    /// Passed only to the viewport drawn after the synchronous rebuild has finished.
    pub fn paint_signal(&self) -> Option<Arc<AtomicBool>> {
        self.waiting.as_ref().map(|w| w.painted.clone())
    }

    /// Empty scenes and the software renderer need no GPU acknowledgement.
    pub fn software_done(&self) {
        if let Some(w) = &self.waiting {
            w.done.store(true, Ordering::Release);
        }
    }

    pub fn poll(&mut self, ctx: &Context, rev: u64, queue: Option<&wgpu::Queue>) {
        if self.preview.is_some_and(|(_, before)| before != rev) || !ctx.input(|i| i.focused) {
            self.preview = None;
        }
        let Some(w) = &mut self.waiting else { return };
        // The paint callback ran in the previous frame. Its command buffer has now
        // been submitted, so this fence includes the updated model, not the old one.
        if !w.watching && w.painted.load(Ordering::Acquire) {
            w.watching = true;
            if let Some(queue) = queue {
                let done = w.done.clone();
                let ctx = ctx.clone();
                queue.on_submitted_work_done(move || {
                    done.store(true, Ordering::Release);
                    ctx.request_repaint();
                });
            } else {
                w.done.store(true, Ordering::Release);
            }
        }
        // A press made while catching up must be released before another drag can
        // start. It must never become a queued drag when the renderer finishes.
        if w.done.load(Ordering::Acquire) && ctx.input(|i| !i.pointer.any_down() && !i.pointer.any_pressed()) {
            self.waiting = None;
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }
}
