//! Desktop open requests may arrive before the first application frame.
//! Queue them until the UI can use its normal load/error/unsaved-work flow.
use std::{collections::VecDeque, path::PathBuf, sync::{Arc, Mutex}};

#[derive(Default)]
struct State {
    pending: VecDeque<Result<PathBuf, String>>,
    context: Option<egui::Context>,
}

#[derive(Clone, Default)]
pub struct OpenRequests(Arc<Mutex<State>>);

impl OpenRequests {
    pub fn attach(&self, context: &egui::Context) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.context = Some(context.clone());
        if !state.pending.is_empty() { context.request_repaint(); }
    }

    pub fn push(&self, request: Result<PathBuf, String>) {
        let context = {
            let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
            // Coalesce duplicate pending deliveries; do not lose a later explicit reopen.
            if state.pending.back() == Some(&request) { return; }
            if state.pending.len() >= 32 { return; }
            state.pending.push_back(request);
            state.context.clone()
        };
        if let Some(context) = context { context.request_repaint(); }
    }

    pub fn pop(&self) -> Option<Result<PathBuf, String>> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let request = state.pending.pop_front();
        if !state.pending.is_empty() && let Some(context) = &state.context { context.request_repaint(); }
        request
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_delivery_is_retained_and_later_reopen_is_not_discarded() {
        let requests = OpenRequests::default();
        let file = PathBuf::from("/tmp/tracing with spaces 雪.ferr");
        requests.push(Ok(file.clone()));
        requests.push(Ok(file.clone()));
        requests.attach(&egui::Context::default());
        assert_eq!(requests.pop(), Some(Ok(file.clone())));
        assert!(requests.pop().is_none());
        requests.push(Ok(file.clone()));
        assert_eq!(requests.pop(), Some(Ok(file)));
    }
}
