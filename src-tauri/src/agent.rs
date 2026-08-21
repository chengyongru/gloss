use crate::{
    app_state::{AppState, OverlayState},
    overlay, profile, responses,
    sessions::{
        self, ConversationSession, MessageRole, SessionAction, SessionSummary, SessionView,
    },
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AgentEvent {
    Started {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "attemptId")]
        attempt_id: String,
    },
    Completed {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "attemptId")]
        attempt_id: String,
        session: SessionView,
    },
    Failed {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "attemptId")]
        attempt_id: String,
        message: String,
        partial: String,
        incomplete: bool,
    },
}

#[tauri::command]
pub async fn start_action(
    app: AppHandle,
    state: State<'_, AppState>,
    action: SessionAction,
) -> Result<SessionView, String> {
    let selection = match state.overlay_snapshot()? {
        OverlayState::Ready { selection } => selection,
        OverlayState::CaptureError { message } => return Err(message),
        OverlayState::Idle => {
            return Err("Select text, then open Gloss with the shortcut.".to_owned());
        }
    };
    let (session, turn_id) = ConversationSession::new(action, selection.text);
    let session_id = session.metadata.session_id.clone();
    let view = session.view();
    {
        let mut sessions = state.sessions.lock().await;
        sessions.insert(session_id.clone(), session);
    }
    *state
        .active_session_id
        .lock()
        .map_err(|_| "The active session state is unavailable.".to_owned())? =
        Some(session_id.clone());
    overlay::expand_to_card(&app)?;
    begin_turn(app, session_id, turn_id).await?;
    Ok(view)
}

#[tauri::command]
pub async fn submit_follow_up(
    app: AppHandle,
    state: State<'_, AppState>,
    content: String,
) -> Result<SessionView, String> {
    let session_id = active_session_id(&state)?;
    let (turn_id, view) = {
        let mut sessions = state.sessions.lock().await;
        let session = ensure_session_loaded(&state, &mut sessions, &session_id)?;
        let turn_id = session.add_follow_up(content)?;
        (turn_id, session.view())
    };
    begin_turn(app, session_id, turn_id).await?;
    Ok(view)
}

#[tauri::command]
pub async fn explain_selection(
    app: AppHandle,
    state: State<'_, AppState>,
    selected_text: String,
) -> Result<SessionView, String> {
    let session_id = active_session_id(&state)?;
    let (turn_id, view) = {
        let mut sessions = state.sessions.lock().await;
        let session = ensure_session_loaded(&state, &mut sessions, &session_id)?;
        let turn_id = session.add_explain_selection(selected_text)?;
        (turn_id, session.view())
    };
    begin_turn(app, session_id, turn_id).await?;
    Ok(view)
}

#[tauri::command]
pub async fn mark_got_it(
    app: AppHandle,
    state: State<'_, AppState>,
    selected_text: String,
) -> Result<(), String> {
    let session_id = active_session_id(&state)?;
    let (view, persisted) = {
        let mut sessions = state.sessions.lock().await;
        let session = ensure_session_loaded(&state, &mut sessions, &session_id)?;
        session.add_got_it(selected_text)?;
        (session.view(), session.clone())
    };
    persist_session(&state, &persisted)?;
    profile::update_now(app, view).await
}

#[tauri::command]
pub async fn retry_turn(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let session_id = active_session_id(&state)?;
    let turn_id = {
        let mut sessions = state.sessions.lock().await;
        let session = ensure_session_loaded(&state, &mut sessions, &session_id)?;
        session
            .attempts
            .last()
            .map(|attempt| attempt.turn_id.clone())
            .ok_or_else(|| "There is no turn to retry.".to_owned())?
    };
    begin_turn(app, session_id, turn_id).await
}

#[tauri::command]
pub async fn get_active_session(state: State<'_, AppState>) -> Result<Option<SessionView>, String> {
    let session_id = state
        .active_session_id
        .lock()
        .map_err(|_| "The active session state is unavailable.".to_owned())?
        .clone();
    let Some(session_id) = session_id else {
        return Ok(None);
    };
    let mut sessions = state.sessions.lock().await;
    let session = ensure_session_loaded(&state, &mut sessions, &session_id)?;
    Ok(Some(session.view()))
}

#[tauri::command]
pub async fn list_history(state: State<'_, AppState>) -> Result<Vec<SessionSummary>, String> {
    let data_dir = state.data_dir()?;
    tauri::async_runtime::spawn_blocking(move || sessions::list_sessions(&data_dir))
        .await
        .map_err(|error| format!("Could not read history: {error}"))?
}

#[tauri::command]
pub async fn load_history(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
) -> Result<SessionView, String> {
    let data_dir = state.data_dir()?;
    let id = session_id.clone();
    let session =
        tauri::async_runtime::spawn_blocking(move || sessions::load_session(&data_dir, &id))
            .await
            .map_err(|error| format!("Could not load history: {error}"))??;
    let view = session.view();
    state
        .sessions
        .lock()
        .await
        .insert(session_id.clone(), session);
    *state
        .active_session_id
        .lock()
        .map_err(|_| "The active session state is unavailable.".to_owned())? = Some(session_id);
    overlay::expand_to_card(&app)?;
    Ok(view)
}

#[tauri::command]
pub async fn delete_history(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    let data_dir = state.data_dir()?;
    let id = session_id.clone();
    tauri::async_runtime::spawn_blocking(move || sessions::delete_session(&data_dir, &id))
        .await
        .map_err(|error| format!("Could not delete history: {error}"))??;
    state.sessions.lock().await.remove(&session_id);
    let mut active = state
        .active_session_id
        .lock()
        .map_err(|_| "The active session state is unavailable.".to_owned())?;
    if active.as_deref() == Some(&session_id) {
        *active = None;
    }
    Ok(())
}

async fn begin_turn(app: AppHandle, session_id: String, turn_id: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    let learner_profile = profile::read_for_prompt(&state)?;
    let data_dir = state.data_dir()?;
    let (attempt_id, request, persisted) = {
        let mut all_sessions = state.sessions.lock().await;
        let session = ensure_session_loaded(&state, &mut all_sessions, &session_id)?;
        let request = session.request_body(&data_dir, learner_profile.as_deref())?;
        let attempt_id = session.begin_attempt(&turn_id, request.clone())?;
        (attempt_id, request, session.clone())
    };
    persist_session(&state, &persisted)?;
    let _ = app.emit(
        "agent-event",
        AgentEvent::Started {
            session_id: session_id.clone(),
            attempt_id: attempt_id.clone(),
        },
    );

    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let outcome =
            responses::stream_response(&app, &state, &session_id, &attempt_id, &request, true)
                .await;

        match outcome {
            Ok(response) => {
                let result = async {
                    let (view, persisted) = {
                        let mut all_sessions = state.sessions.lock().await;
                        let session =
                            ensure_session_loaded(&state, &mut all_sessions, &session_id)?;
                        session.complete_attempt(
                            &attempt_id,
                            response.text,
                            response.response_id,
                            response.output_items,
                            response.usage,
                            response.transport_attempts,
                        )?;
                        (session.view(), session.clone())
                    };
                    persist_session(&state, &persisted)?;
                    Ok::<SessionView, String>(view)
                }
                .await;
                match result {
                    Ok(session) => {
                        let user_turns = session
                            .messages
                            .iter()
                            .filter(|message| message.role == MessageRole::User)
                            .count();
                        if session.action == SessionAction::Triage && user_turns >= 2 {
                            profile::queue_update(app.clone(), session.clone());
                        }
                        let _ = app.emit(
                            "agent-event",
                            AgentEvent::Completed {
                                session_id,
                                attempt_id,
                                session,
                            },
                        );
                    }
                    Err(message) => {
                        let _ = app.emit(
                            "agent-event",
                            AgentEvent::Failed {
                                session_id,
                                attempt_id,
                                message,
                                partial: String::new(),
                                incomplete: false,
                            },
                        );
                    }
                }
            }
            Err(failure) => {
                let persisted = {
                    let mut all_sessions = state.sessions.lock().await;
                    let result = ensure_session_loaded(&state, &mut all_sessions, &session_id)
                        .and_then(|session| {
                            session.fail_attempt(
                                &attempt_id,
                                failure.partial.clone(),
                                failure.message.clone(),
                                failure.incomplete,
                                failure.transport_attempts,
                            )?;
                            Ok(session.clone())
                        });
                    result.ok()
                };
                if let Some(session) = persisted {
                    let _ = persist_session(&state, &session);
                }
                let _ = app.emit(
                    "agent-event",
                    AgentEvent::Failed {
                        session_id,
                        attempt_id,
                        message: failure.message,
                        partial: failure.partial,
                        incomplete: failure.incomplete,
                    },
                );
            }
        }
    });
    Ok(())
}

fn active_session_id(state: &AppState) -> Result<String, String> {
    state
        .active_session_id
        .lock()
        .map_err(|_| "The active session state is unavailable.".to_owned())?
        .clone()
        .ok_or_else(|| "There is no active Gloss session.".to_owned())
}

fn ensure_session_loaded<'a>(
    state: &AppState,
    all_sessions: &'a mut std::collections::HashMap<String, ConversationSession>,
    session_id: &str,
) -> Result<&'a mut ConversationSession, String> {
    if !all_sessions.contains_key(session_id) {
        let loaded = sessions::load_session(&state.data_dir()?, session_id)?;
        all_sessions.insert(session_id.to_owned(), loaded);
    }
    all_sessions
        .get_mut(session_id)
        .ok_or_else(|| "The Gloss session is unavailable.".to_owned())
}

fn persist_session(state: &AppState, session: &ConversationSession) -> Result<(), String> {
    sessions::save_session(&state.data_dir()?, session)
}
