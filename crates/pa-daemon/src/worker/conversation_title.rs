//! Conversation titles in the worker: the one-shot automatic title after a
//! top-level session's first settled turn, and the `generate_session_title`
//! command behind `/name` with no argument. Both call the `titleModel`
//! generator in `pa-core`; neither ever fails a turn.
use super::{
    emit_worker_event_with, json, response_failure, response_success, session_summary, EventPump,
    SessionCore, SessionEngine, Value, Worker,
};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use pa_core::session_engine::conversation_title::{
    generate_conversation_title, ConversationTitleRequest,
};

use crate::protocol::DaemonResponse;

/// Shared by the command arms and the turn runner so a manual rename or
/// title request can stop an automatic generation still in flight.
#[derive(Default)]
pub(crate) struct ConversationTitles {
    /// Set once a title was generated or attempted; automatic generation
    /// runs at most once per worker.
    started: AtomicBool,
    task: Mutex<Option<tokio::task::AbortHandle>>,
}

impl ConversationTitles {
    /// Abort an automatic generation in flight (a name was set by hand).
    pub(crate) fn cancel(&self) {
        if let Some(task) = self.task.lock().unwrap().take() {
            task.abort();
        }
    }
}

/// What the automatic title needs from the turn runner.
pub(super) struct AutoTitleContext {
    pub(super) core: Arc<Mutex<SessionCore>>,
    pub(super) events: Arc<EventPump>,
    pub(super) engine: Arc<dyn SessionEngine>,
    pub(super) user_bash: Arc<crate::user_bash::UserBash>,
    pub(super) agent_dir: std::path::PathBuf,
    /// Environment participants are scripted episodes, never titled.
    pub(super) participant: bool,
}

/// Name a top-level session after the settled turn, once. A turn that
/// failed or was aborted leaves the session unstarted so a later turn can
/// still title it.
pub(super) fn schedule_after_turn(titles: &Arc<ConversationTitles>, context: AutoTitleContext) {
    if context.participant || titles.started.load(Ordering::SeqCst) {
        return;
    }
    let (cwd, session_id, messages) = {
        let core = context.core.lock().unwrap();
        let Some(store) = core.store.as_ref() else {
            return;
        };
        if !core.created
            || core.rlm_depth != 0
            || core.runtime_kind != "top-level"
            || store.session_name().is_some()
        {
            return;
        }
        (
            core.cwd.clone(),
            store.session_id().to_string(),
            store.messages(),
        )
    };
    let Some(model) =
        pa_core::settings::SettingsManager::create(&cwd, &context.agent_dir).get_title_model()
    else {
        return;
    };
    if titles.started.swap(true, Ordering::SeqCst) {
        return;
    }
    let task_titles = Arc::clone(titles);
    let task = tokio::spawn(async move {
        let title = generate_conversation_title(ConversationTitleRequest {
            agent_dir: &context.agent_dir,
            model: &model,
            messages: &messages,
        })
        .await;
        let Some(title) = title else {
            return;
        };
        // The setting, the name, and the session may have moved on while
        // the model call ran.
        let current =
            pa_core::settings::SettingsManager::create(&cwd, &context.agent_dir).get_title_model();
        if current.as_ref() != Some(&model) {
            return;
        }
        {
            let mut core = context.core.lock().unwrap();
            let Some(store) = core.store.as_mut() else {
                return;
            };
            if store.session_id() != session_id || store.session_name().is_some() {
                return;
            }
            if store
                .persist_entry("session_info", json!({ "name": title }))
                .is_err()
            {
                return;
            }
        }
        emit_worker_event_with(
            &context.core,
            &context.events,
            json!({ "type": "session_info_changed", "name": title }),
        );
        // The sender identity follows the live name.
        let summary = {
            let core = context.core.lock().unwrap();
            session_summary(
                &core,
                &context
                    .engine
                    .effective_thinking_level()
                    .unwrap_or_else(|| "default".to_string()),
                context.engine.model_metadata(),
                context.engine.model_fallback_message(),
                context.user_bash.is_running(),
                context.engine.is_quota_parked(),
            )
        };
        if let Ok(summary) = serde_json::to_value(&summary) {
            context.engine.set_session_summary(summary);
        }
        task_titles.task.lock().unwrap().take();
    });
    *titles.task.lock().unwrap() = Some(task.abort_handle());
}

impl Worker {
    /// `generate_session_title`: name the session with the configured title
    /// model and persist the result like `set_session_name`. Replaces any
    /// automatic generation in flight; fails with a message the client
    /// shows verbatim.
    pub(super) async fn handle_generate_session_title(&self) -> DaemonResponse {
        const COMMAND: &str = "generate_session_title";
        if let Err(response) = self.require_created(COMMAND) {
            return response;
        }
        let (cwd, messages) = {
            let core = self.core.lock().unwrap();
            let messages: Vec<Value> = core
                .store
                .as_ref()
                .map(crate::session_store::SessionFile::messages)
                .unwrap_or_default();
            (core.cwd.clone(), messages)
        };
        let Some(model) = pa_core::settings::SettingsManager::create(&cwd, &self.config.agent_dir)
            .get_title_model()
        else {
            return response_failure(
                None,
                COMMAND,
                "No conversation title model is configured. Set one in /settings.",
                None,
            );
        };
        self.titles.cancel();
        let title = generate_conversation_title(ConversationTitleRequest {
            agent_dir: &self.config.agent_dir,
            model: &model,
            messages: &messages,
        })
        .await;
        let Some(title) = title else {
            return response_failure(
                None,
                COMMAND,
                "Could not generate a session title from the current conversation.",
                None,
            );
        };
        // A manual title also settles the automatic one.
        self.titles.started.store(true, Ordering::SeqCst);
        let renamed = self.handle_rename(COMMAND, &json!({ "name": title }));
        if !renamed.success {
            return renamed;
        }
        response_success(None, COMMAND, Some(json!({ "name": title })))
    }
}
