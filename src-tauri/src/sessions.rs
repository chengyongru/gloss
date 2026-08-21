use crate::prompts;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};
use uuid::Uuid;
use windows::{
    Win32::Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW},
    core::PCWSTR,
};

pub const MODEL: &str = "gpt-5.6-luna";
pub const PROMPT_VERSION: u32 = 4;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SessionAction {
    Triage,
    Translate,
}

impl SessionAction {
    pub fn runtime_context(self, data_dir: &Path, selected_text: &str) -> Result<String, String> {
        match self {
            Self::Triage => prompts::triage_context(data_dir, selected_text),
            Self::Translate => prompts::translate_context(data_dir, selected_text),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetadata {
    pub session_id: String,
    pub action: SessionAction,
    pub selected_text: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub model: String,
    pub prompt_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredMessage {
    pub id: String,
    pub turn_id: String,
    pub role: MessageRole,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<MessageIntent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageIntent {
    ExplainSelection,
    GotIt,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStatus {
    Pending,
    Completed,
    Failed,
    Incomplete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttemptRecord {
    pub id: String,
    pub turn_id: String,
    pub status: AttemptStatus,
    pub request: Value,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub transport_attempts: u8,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_items: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSession {
    pub metadata: SessionMetadata,
    pub messages: Vec<StoredMessage>,
    pub attempts: Vec<AttemptRecord>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub session_id: String,
    pub action: SessionAction,
    pub selected_text: String,
    pub messages: Vec<StoredMessage>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub session_id: String,
    pub action: SessionAction,
    pub preview: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "_type", rename_all = "snake_case")]
enum PersistedRecord {
    Metadata {
        #[serde(flatten)]
        metadata: SessionMetadata,
    },
    Message {
        #[serde(flatten)]
        message: StoredMessage,
    },
    Attempt {
        #[serde(flatten)]
        attempt: AttemptRecord,
    },
}

impl ConversationSession {
    pub fn new(action: SessionAction, selected_text: String) -> (Self, String) {
        let now = Utc::now();
        let session_id = Uuid::new_v4().to_string();
        let turn_id = Uuid::new_v4().to_string();
        let first_message = StoredMessage {
            id: Uuid::new_v4().to_string(),
            turn_id: turn_id.clone(),
            role: MessageRole::User,
            content: selected_text.clone(),
            timestamp: now,
            intent: None,
            attempt_id: None,
        };
        (
            Self {
                metadata: SessionMetadata {
                    session_id,
                    action,
                    selected_text,
                    created_at: now,
                    updated_at: now,
                    model: MODEL.to_owned(),
                    prompt_version: PROMPT_VERSION,
                },
                messages: vec![first_message],
                attempts: Vec::new(),
            },
            turn_id,
        )
    }

    pub fn view(&self) -> SessionView {
        SessionView {
            session_id: self.metadata.session_id.clone(),
            action: self.metadata.action,
            selected_text: self.metadata.selected_text.clone(),
            messages: self.messages.clone(),
        }
    }

    pub fn add_follow_up(&mut self, content: String) -> Result<String, String> {
        self.add_user_turn(content, None)
    }

    pub fn add_explain_selection(&mut self, content: String) -> Result<String, String> {
        self.add_user_turn(content, Some(MessageIntent::ExplainSelection))
    }

    pub fn add_got_it(&mut self, content: String) -> Result<String, String> {
        self.add_user_turn(content, Some(MessageIntent::GotIt))
    }

    fn add_user_turn(
        &mut self,
        content: String,
        intent: Option<MessageIntent>,
    ) -> Result<String, String> {
        if self.metadata.action != SessionAction::Triage {
            return Err("Follow-up questions are only available after Triage.".to_owned());
        }
        if content.trim().is_empty() {
            return Err("Write a question before sending.".to_owned());
        }
        let turn_id = Uuid::new_v4().to_string();
        let now = Utc::now();
        self.messages.push(StoredMessage {
            id: Uuid::new_v4().to_string(),
            turn_id: turn_id.clone(),
            role: MessageRole::User,
            content,
            timestamp: now,
            intent,
            attempt_id: None,
        });
        self.metadata.updated_at = now;
        Ok(turn_id)
    }

    pub fn has_user_turn(&self, turn_id: &str) -> bool {
        self.messages
            .iter()
            .any(|message| message.turn_id == turn_id && message.role == MessageRole::User)
    }

    pub fn request_body(
        &self,
        data_dir: &Path,
        learner_profile: Option<&str>,
    ) -> Result<Value, String> {
        let mut input = Vec::new();
        for (index, message) in self.messages.iter().enumerate() {
            match message.role {
                MessageRole::User => input.push(json!({
                    "type": "message",
                    "role": "user",
                    "content": [{
                        "type": "input_text",
                        "text": match message.intent {
                            Some(MessageIntent::ExplainSelection) => {
                                prompts::explain_selection_context(data_dir, &message.content)?
                            }
                            Some(MessageIntent::GotIt) => {
                                prompts::got_it_context(data_dir, &message.content)?
                            }
                            None if index == 0 => {
                                self.metadata.action.runtime_context(data_dir, &self.metadata.selected_text)?
                            }
                            None => message.content.clone(),
                        }
                    }],
                })),
                MessageRole::Assistant => {
                    let output_items = message
                        .attempt_id
                        .as_deref()
                        .and_then(|attempt_id| {
                            self.attempts.iter().find(|attempt| {
                                attempt.id == attempt_id
                                    && attempt.status == AttemptStatus::Completed
                            })
                        })
                        .map(|attempt| attempt.output_items.as_slice())
                        .unwrap_or_default();
                    if output_items.is_empty() {
                        input.push(json!({
                            "type": "message",
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": message.content}],
                        }));
                    } else {
                        input.extend(output_items.iter().cloned());
                    }
                }
            }
        }
        for item in &mut input {
            if let Some(object) = item.as_object_mut() {
                object.remove("id");
            }
        }
        let reasoning = if self
            .messages
            .iter()
            .any(|message| message.role == MessageRole::Assistant)
            && self.metadata.model.to_ascii_lowercase().contains("gpt-5.6")
        {
            json!({"effort": "none", "context": "all_turns"})
        } else {
            json!({"effort": "none"})
        };
        let cache_key = format!(
            "gloss:{}:{}",
            self.metadata.prompt_version,
            &self.metadata.session_id[..8]
        );
        Ok(json!({
            "model": self.metadata.model,
            "store": false,
            "stream": true,
            "instructions": prompts::system(data_dir, learner_profile)?,
            "input": input,
            "include": ["reasoning.encrypted_content"],
            "reasoning": reasoning,
            "text": {"verbosity": "medium"},
            "prompt_cache_key": cache_key,
        }))
    }

    pub fn begin_attempt(&mut self, turn_id: &str, request: Value) -> Result<String, String> {
        if !self.has_user_turn(turn_id) {
            return Err("The requested turn does not exist.".to_owned());
        }
        let attempt_id = Uuid::new_v4().to_string();
        self.attempts.push(AttemptRecord {
            id: attempt_id.clone(),
            turn_id: turn_id.to_owned(),
            status: AttemptStatus::Pending,
            request,
            started_at: Utc::now(),
            finished_at: None,
            transport_attempts: 0,
            text: String::new(),
            response_id: None,
            output_items: Vec::new(),
            usage: None,
            error: None,
        });
        Ok(attempt_id)
    }

    pub fn complete_attempt(
        &mut self,
        attempt_id: &str,
        text: String,
        response_id: Option<String>,
        output_items: Vec<Value>,
        usage: Option<Value>,
        transport_attempts: u8,
    ) -> Result<(), String> {
        let attempt = self
            .attempts
            .iter_mut()
            .find(|attempt| attempt.id == attempt_id)
            .ok_or_else(|| "The response attempt does not exist.".to_owned())?;
        attempt.status = AttemptStatus::Completed;
        attempt.finished_at = Some(Utc::now());
        attempt.transport_attempts = transport_attempts;
        attempt.text = text.clone();
        attempt.response_id = response_id;
        attempt.output_items = output_items;
        attempt.usage = usage;
        let turn_id = attempt.turn_id.clone();
        self.messages.push(StoredMessage {
            id: Uuid::new_v4().to_string(),
            turn_id,
            role: MessageRole::Assistant,
            content: text,
            timestamp: Utc::now(),
            intent: None,
            attempt_id: Some(attempt_id.to_owned()),
        });
        self.metadata.updated_at = Utc::now();
        Ok(())
    }

    pub fn fail_attempt(
        &mut self,
        attempt_id: &str,
        partial: String,
        message: String,
        incomplete: bool,
        transport_attempts: u8,
    ) -> Result<(), String> {
        let attempt = self
            .attempts
            .iter_mut()
            .find(|attempt| attempt.id == attempt_id)
            .ok_or_else(|| "The response attempt does not exist.".to_owned())?;
        attempt.status = if incomplete {
            AttemptStatus::Incomplete
        } else {
            AttemptStatus::Failed
        };
        attempt.finished_at = Some(Utc::now());
        attempt.transport_attempts = transport_attempts;
        attempt.text = partial;
        attempt.error = Some(message);
        self.metadata.updated_at = Utc::now();
        Ok(())
    }
}

pub fn save_session(data_dir: &Path, session: &ConversationSession) -> Result<(), String> {
    let sessions_dir = data_dir.join("sessions");
    fs::create_dir_all(&sessions_dir)
        .map_err(|error| format!("Could not create the history folder: {error}"))?;
    let path = session_path(&sessions_dir, &session.metadata.session_id)?;
    let mut payload = Vec::new();
    write_record(
        &mut payload,
        &PersistedRecord::Metadata {
            metadata: session.metadata.clone(),
        },
    )?;
    for message in &session.messages {
        write_record(
            &mut payload,
            &PersistedRecord::Message {
                message: message.clone(),
            },
        )?;
    }
    for attempt in &session.attempts {
        write_record(
            &mut payload,
            &PersistedRecord::Attempt {
                attempt: attempt.clone(),
            },
        )?;
    }
    atomic_write(&path, &payload)
}

pub fn load_session(data_dir: &Path, session_id: &str) -> Result<ConversationSession, String> {
    let path = session_path(&data_dir.join("sessions"), session_id)?;
    let file = fs::File::open(&path)
        .map_err(|error| format!("Could not open this history item: {error}"))?;
    let mut metadata = None;
    let mut messages = Vec::new();
    let mut attempts = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|error| format!("Could not read this history item: {error}"))?;
        if line.trim().is_empty() {
            continue;
        }
        let record: PersistedRecord = serde_json::from_str(&line)
            .map_err(|error| format!("This history item is damaged: {error}"))?;
        match record {
            PersistedRecord::Metadata { metadata: value } => metadata = Some(value),
            PersistedRecord::Message { message } => messages.push(message),
            PersistedRecord::Attempt { attempt } => attempts.push(attempt),
        }
    }
    Ok(ConversationSession {
        metadata: metadata.ok_or_else(|| "This history item has no metadata.".to_owned())?,
        messages,
        attempts,
    })
}

pub fn list_sessions(data_dir: &Path) -> Result<Vec<SessionSummary>, String> {
    let sessions_dir = data_dir.join("sessions");
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }
    let mut summaries = Vec::new();
    for entry in fs::read_dir(&sessions_dir)
        .map_err(|error| format!("Could not read the history folder: {error}"))?
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if entry.path().extension().and_then(OsStr::to_str) != Some("jsonl") {
            continue;
        }
        let file = match fs::File::open(entry.path()) {
            Ok(file) => file,
            Err(_) => continue,
        };
        let mut first = String::new();
        if BufReader::new(file).read_line(&mut first).is_err() {
            continue;
        }
        let record = match serde_json::from_str::<PersistedRecord>(&first) {
            Ok(record) => record,
            Err(_) => continue,
        };
        if let PersistedRecord::Metadata { metadata } = record {
            summaries.push(SessionSummary {
                session_id: metadata.session_id,
                action: metadata.action,
                preview: compact_preview(&metadata.selected_text),
                updated_at: metadata.updated_at,
            });
        }
    }
    summaries.sort_by_key(|summary| std::cmp::Reverse(summary.updated_at));
    Ok(summaries)
}

pub fn delete_session(data_dir: &Path, session_id: &str) -> Result<(), String> {
    let path = session_path(&data_dir.join("sessions"), session_id)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Could not delete this history item: {error}")),
    }
}

fn session_path(sessions_dir: &Path, session_id: &str) -> Result<PathBuf, String> {
    let parsed =
        Uuid::parse_str(session_id).map_err(|_| "The session ID is invalid.".to_owned())?;
    Ok(sessions_dir.join(format!("{parsed}.jsonl")))
}

fn write_record(target: &mut Vec<u8>, record: &PersistedRecord) -> Result<(), String> {
    serde_json::to_writer(&mut *target, record)
        .map_err(|error| format!("Could not encode the history item: {error}"))?;
    target.push(b'\n');
    Ok(())
}

pub(crate) fn atomic_write(path: &Path, payload: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "The history path has no parent folder.".to_owned())?;
    let temp = parent.join(format!(".{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| format!("Could not create a temporary history file: {error}"))?;
        file.write_all(payload)
            .and_then(|_| file.flush())
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("Could not save the history item: {error}"))?;
        drop(file);

        let from = wide_path(&temp);
        let to = wide_path(path);
        // SAFETY: both UTF-16 paths are NUL-terminated and point to files in the same folder.
        unsafe {
            MoveFileExW(
                PCWSTR(from.as_ptr()),
                PCWSTR(to.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
        .map_err(|error| format!("Could not commit the history item: {error}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn compact_preview(text: &str) -> String {
    const MAX_DISPLAY_WIDTH: usize = 44;

    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut preview = String::new();
    let mut display_width = 0;
    for character in one_line.chars() {
        let character_width = if character.is_ascii() { 1 } else { 2 };
        if display_width + character_width > MAX_DISPLAY_WIDTH {
            preview.push('…');
            break;
        }
        preview.push(character);
        display_width += character_width;
    }
    preview
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_contains_only_conversation_messages_as_input() {
        let (session, _) = ConversationSession::new(
            SessionAction::Triage,
            "I've been awarded the title.".to_owned(),
        );
        let data_dir = std::env::temp_dir().join(format!("gloss-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&data_dir).unwrap();
        let body = session.request_body(&data_dir, None).unwrap();
        fs::remove_dir_all(&data_dir).unwrap();
        assert_eq!(body["model"], MODEL);
        let initial = body["input"][0]["content"][0]["text"].as_str().unwrap();
        assert!(initial.contains("<runtime_context>"));
        assert!(initial.contains("I've been awarded the title."));
        assert!(
            body["instructions"]
                .as_str()
                .unwrap()
                .contains("conversational learning agent")
        );
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn preview_is_bounded_by_display_width() {
        let preview = compact_preview(&"学".repeat(30));
        assert_eq!(preview, format!("{}…", "学".repeat(22)));

        let mixed = compact_preview(&format!("{}{}", "a".repeat(40), "学".repeat(3)));
        assert_eq!(mixed, format!("{}学学…", "a".repeat(40)));
    }

    #[test]
    fn completed_output_items_are_replayed_without_response_ids() {
        let (mut session, turn_id) =
            ConversationSession::new(SessionAction::Triage, "Selected text".to_owned());
        let attempt_id = session
            .begin_attempt(&turn_id, json!({"request": true}))
            .unwrap();
        session
            .complete_attempt(
                &attempt_id,
                "Explanation".to_owned(),
                Some("resp_1".to_owned()),
                vec![json!({
                    "id": "msg_1",
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "Explanation"}]
                })],
                None,
                1,
            )
            .unwrap();
        session.add_follow_up("Why?".to_owned()).unwrap();
        let data_dir = std::env::temp_dir().join(format!("gloss-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&data_dir).unwrap();
        let body = session.request_body(&data_dir, None).unwrap();
        fs::remove_dir_all(&data_dir).unwrap();
        assert!(body["input"][1].get("id").is_none());
        assert_eq!(body["input"][1]["type"], "message");
        assert_eq!(
            body["input"].as_array().unwrap().last().unwrap()["content"][0]["text"],
            "Why?"
        );
        assert_eq!(body["reasoning"]["context"], "all_turns");
    }

    #[test]
    fn explain_selection_is_a_user_turn_with_runtime_context() {
        let (mut session, _) =
            ConversationSession::new(SessionAction::Triage, "Selected text".to_owned());
        session
            .add_explain_selection("been awarded".to_owned())
            .unwrap();
        let data_dir = std::env::temp_dir().join(format!("gloss-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&data_dir).unwrap();
        let body = session.request_body(&data_dir, None).unwrap();
        fs::remove_dir_all(&data_dir).unwrap();

        assert_eq!(session.messages[1].role, MessageRole::User);
        assert_eq!(
            session.messages[1].intent,
            Some(MessageIntent::ExplainSelection)
        );
        let shortcut = body["input"][1]["content"][0]["text"].as_str().unwrap();
        assert!(shortcut.contains("passage the learner selected"));
        assert!(shortcut.contains("been awarded"));
    }

    #[test]
    fn got_it_is_a_user_learning_signal_with_runtime_context() {
        let (mut session, _) =
            ConversationSession::new(SessionAction::Triage, "Selected text".to_owned());
        session.add_got_it("been awarded".to_owned()).unwrap();
        let data_dir = std::env::temp_dir().join(format!("gloss-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&data_dir).unwrap();
        let body = session.request_body(&data_dir, None).unwrap();
        fs::remove_dir_all(&data_dir).unwrap();

        assert_eq!(session.messages[1].role, MessageRole::User);
        assert_eq!(session.messages[1].intent, Some(MessageIntent::GotIt));
        let signal = body["input"][1]["content"][0]["text"].as_str().unwrap();
        assert!(signal.contains("newly learned and now understand"));
        assert!(signal.contains("been awarded"));
    }
}
