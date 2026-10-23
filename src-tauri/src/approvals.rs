//! Approval popups: one small always-on-top window per pending tool call.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use futures::future::BoxFuture;
use plexus_core::{ApprovalBroker, ApprovalDecision, ApprovalRequest};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::oneshot;

type Pending = HashMap<String, (ApprovalRequest, oneshot::Sender<ApprovalDecision>)>;

#[derive(Clone)]
pub struct Approvals {
    app: AppHandle,
    pending: Arc<Mutex<Pending>>,
}

impl Approvals {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            pending: Default::default(),
        }
    }

    pub fn get(&self, id: &str) -> Option<ApprovalRequest> {
        self.pending
            .lock()
            .unwrap()
            .get(id)
            .map(|(req, _)| req.clone())
    }

    pub fn resolve(&self, id: &str, decision: ApprovalDecision) {
        if let Some((_, tx)) = self.pending.lock().unwrap().remove(id) {
            let _ = tx.send(decision);
        }
        close_window(&self.app, id);
    }
}

fn window_label(id: &str) -> String {
    format!("approval-{id}")
}

fn close_window(app: &AppHandle, id: &str) {
    if let Some(w) = app.get_webview_window(&window_label(id)) {
        let _ = w.close();
    }
}

/// Removes the pending entry and closes the popup when the hub stops waiting
/// (decision made, timed out, or the call was cancelled).
struct Cleanup {
    approvals: Approvals,
    id: String,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        self.approvals.pending.lock().unwrap().remove(&self.id);
        close_window(&self.approvals.app, &self.id);
    }
}

impl ApprovalBroker for Approvals {
    fn request(&self, req: ApprovalRequest) -> BoxFuture<'static, ApprovalDecision> {
        let (tx, rx) = oneshot::channel();
        let id = req.id.clone();
        self.pending
            .lock()
            .unwrap()
            .insert(id.clone(), (req, tx));
        let url = WebviewUrl::App(format!("index.html#/approval/{id}").into());
        if let Err(e) = WebviewWindowBuilder::new(&self.app, window_label(&id), url)
            .title("Plexus · Approve tool call")
            .inner_size(440.0, 420.0)
            .resizable(false)
            .always_on_top(true)
            .center()
            .focused(true)
            .build()
        {
            tracing::error!("could not open approval window: {e}");
        }
        let cleanup = Cleanup {
            approvals: self.clone(),
            id,
        };
        Box::pin(async move {
            let _cleanup = cleanup;
            rx.await.unwrap_or(ApprovalDecision::Deny)
        })
    }
}
