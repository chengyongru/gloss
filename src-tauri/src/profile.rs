use crate::{
    app_state::AppState,
    prompts, responses,
    sessions::{self, MODEL, MessageRole, SessionView},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use tauri::{AppHandle, Manager, State};

const PROFILE_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LearnerProfile {
    version: u32,
    updated_at: DateTime<Utc>,
    conversations_observed: u32,
    overall: CefrEstimate,
    dimensions: Vec<DimensionEstimate>,
    observations: Vec<ProfileObservation>,
    #[serde(default)]
    observed_session_ids: Vec<String>,
    #[serde(default)]
    processed_turn_ids: Vec<String>,
}

impl Default for LearnerProfile {
    fn default() -> Self {
        Self {
            version: PROFILE_VERSION,
            updated_at: Utc::now(),
            conversations_observed: 0,
            overall: CefrEstimate {
                level: CefrLevel::InsufficientEvidence,
                confidence: 0.0,
                rationale: "Not enough multi-turn learning evidence yet.".to_owned(),
            },
            dimensions: Vec::new(),
            observations: Vec::new(),
            observed_session_ids: Vec::new(),
            processed_turn_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CefrEstimate {
    level: CefrLevel,
    confidence: f32,
    rationale: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
enum CefrLevel {
    A1,
    A2,
    B1,
    B2,
    C1,
    C2,
    #[serde(rename = "insufficient_evidence")]
    InsufficientEvidence,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Dimension {
    Reading,
    Vocabulary,
    Grammar,
    Pragmatics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DimensionEstimate {
    dimension: Dimension,
    level: CefrLevel,
    confidence: f32,
    evidence: String,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileObservation {
    dimension: Dimension,
    cefr_level: CefrLevel,
    descriptor: String,
    evidence: String,
    evidence_count: u32,
    first_seen_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfilePatch {
    overall: CefrEstimate,
    dimensions: Vec<PatchDimension>,
    observations: Vec<PatchObservation>,
}

#[derive(Debug, Deserialize)]
struct PatchDimension {
    dimension: Dimension,
    level: CefrLevel,
    confidence: f32,
    evidence: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchObservation {
    dimension: Dimension,
    cefr_level: CefrLevel,
    descriptor: String,
    evidence: String,
}

pub fn queue_update(app: AppHandle, session: SessionView) {
    let user_messages = session
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .collect::<Vec<_>>();
    if user_messages.len() < 2 {
        return;
    }
    let Some(latest_turn_id) = user_messages.last().map(|message| message.turn_id.clone()) else {
        return;
    };

    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let _guard = state.profile_lock.lock().await;
        let mut profile = match load_or_default(&state) {
            Ok(profile) => profile,
            Err(_) => return,
        };
        if profile.processed_turn_ids.contains(&latest_turn_id) {
            return;
        }
        let request = match profile_request(&state, &profile, &session) {
            Ok(request) => request,
            Err(_) => return,
        };
        let response = match responses::stream_response(
            &app,
            &state,
            "learner-profile",
            &latest_turn_id,
            &request,
            false,
        )
        .await
        {
            Ok(response) => response,
            Err(_) => return,
        };
        let patch = match serde_json::from_str::<ProfilePatch>(&response.text) {
            Ok(patch) => patch,
            Err(_) => return,
        };
        apply_patch(&mut profile, patch, &session.session_id, &latest_turn_id);
        let _ = save(&state, &profile);
    });
}

pub fn read_for_prompt(state: &AppState) -> Result<Option<String>, String> {
    let profile = match load(state) {
        Ok(profile) => profile,
        Err(message) if message == "missing" => return Ok(None),
        Err(message) => return Err(message),
    };
    serde_json::to_string_pretty(&public_profile(&profile))
        .map(Some)
        .map_err(|error| format!("Could not encode the learner profile: {error}"))
}

#[tauri::command]
pub fn get_learner_profile(state: State<'_, AppState>) -> Result<Value, String> {
    let profile = load_or_default(&state)?;
    Ok(public_profile(&profile))
}

fn public_profile(profile: &LearnerProfile) -> Value {
    json!({
        "framework": "CEFR",
        "updatedAt": profile.updated_at,
        "conversationsObserved": profile.conversations_observed,
        "overall": profile.overall,
        "dimensions": profile.dimensions,
        "recentObservations": profile.observations.iter().take(24).collect::<Vec<_>>(),
    })
}

fn load(state: &AppState) -> Result<LearnerProfile, String> {
    let path = state.data_dir()?.join("learner-profile.json");
    match fs::read_to_string(path) {
        Ok(value) => serde_json::from_str(&value)
            .map_err(|error| format!("The learner profile is damaged: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err("missing".to_owned()),
        Err(error) => Err(format!("Could not read the learner profile: {error}")),
    }
}

fn load_or_default(state: &AppState) -> Result<LearnerProfile, String> {
    match load(state) {
        Ok(profile) => Ok(profile),
        Err(message) if message == "missing" => Ok(LearnerProfile::default()),
        Err(message) => Err(message),
    }
}

fn save(state: &AppState, profile: &LearnerProfile) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(profile)
        .map_err(|error| format!("Could not encode the learner profile: {error}"))?;
    sessions::atomic_write(&state.data_dir()?.join("learner-profile.json"), &bytes)
}

fn profile_request(
    state: &AppState,
    profile: &LearnerProfile,
    session: &SessionView,
) -> Result<Value, String> {
    let initial_turn_id = session
        .messages
        .first()
        .map(|message| message.turn_id.as_str());
    let follow_up_conversation = session
        .messages
        .iter()
        .filter(|message| Some(message.turn_id.as_str()) != initial_turn_id)
        .map(|message| {
            json!({
                "role": message.role,
                "content": message.content,
                "intent": message.intent,
            })
        })
        .collect::<Vec<_>>();
    let context = json!({
        "currentProfile": {
            "overall": profile.overall,
            "dimensions": profile.dimensions,
            "recentObservations": profile.observations.iter().take(24).collect::<Vec<_>>(),
        },
        "readingContext": {
            "selectedText": session.selected_text,
        },
        "evidenceConversation": follow_up_conversation,
    });
    Ok(json!({
        "model": MODEL,
        "store": false,
        "stream": true,
        "instructions": prompts::learner_profile(&state.data_dir()?)?,
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": context.to_string()}],
        }],
        "reasoning": {"effort": "none"},
        "text": {
            "verbosity": "low",
            "format": {
                "type": "json_schema",
                "name": "cefr_learner_profile_patch",
                "strict": true,
                "schema": profile_patch_schema()
            }
        },
        "prompt_cache_key": "gloss:cefr-learner-profile:v3"
    }))
}

fn profile_patch_schema() -> Value {
    let levels = json!(["A1", "A2", "B1", "B2", "C1", "C2", "insufficient_evidence"]);
    let dimensions = json!(["reading", "vocabulary", "grammar", "pragmatics"]);
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "overall": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "level": {"type": "string", "enum": levels},
                    "confidence": {"type": "number", "minimum": 0, "maximum": 1},
                    "rationale": {"type": "string", "maxLength": 240}
                },
                "required": ["level", "confidence", "rationale"]
            },
            "dimensions": {
                "type": "array",
                "maxItems": 4,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "dimension": {"type": "string", "enum": dimensions},
                        "level": {"type": "string", "enum": levels},
                        "confidence": {"type": "number", "minimum": 0, "maximum": 1},
                        "evidence": {"type": "string", "maxLength": 240}
                    },
                    "required": ["dimension", "level", "confidence", "evidence"]
                }
            },
            "observations": {
                "type": "array",
                "maxItems": 6,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "dimension": {"type": "string", "enum": dimensions},
                        "cefrLevel": {"type": "string", "enum": levels},
                        "descriptor": {"type": "string", "maxLength": 160},
                        "evidence": {"type": "string", "maxLength": 240}
                    },
                    "required": ["dimension", "cefrLevel", "descriptor", "evidence"]
                }
            }
        },
        "required": ["overall", "dimensions", "observations"]
    })
}

fn apply_patch(profile: &mut LearnerProfile, patch: ProfilePatch, session_id: &str, turn_id: &str) {
    let now = Utc::now();
    profile.overall = sanitized_overall(patch.overall);

    for estimate in patch.dimensions.into_iter().take(4) {
        let evidence = compact(&estimate.evidence, 240);
        let next = DimensionEstimate {
            dimension: estimate.dimension,
            level: estimate.level,
            confidence: estimate.confidence.clamp(0.0, 1.0),
            evidence,
            updated_at: now,
        };
        if let Some(existing) = profile
            .dimensions
            .iter_mut()
            .find(|existing| existing.dimension == next.dimension)
        {
            *existing = next;
        } else {
            profile.dimensions.push(next);
        }
    }

    for observation in patch.observations.into_iter().take(6) {
        let descriptor = compact(&observation.descriptor, 160);
        let evidence = compact(&observation.evidence, 240);
        if descriptor.is_empty() || evidence.is_empty() {
            continue;
        }
        if let Some(existing) = profile.observations.iter_mut().find(|existing| {
            existing.dimension == observation.dimension
                && existing.cefr_level == observation.cefr_level
                && existing.descriptor.eq_ignore_ascii_case(&descriptor)
        }) {
            existing.evidence = evidence;
            existing.evidence_count = existing.evidence_count.saturating_add(1);
            existing.last_seen_at = now;
        } else {
            profile.observations.push(ProfileObservation {
                dimension: observation.dimension,
                cefr_level: observation.cefr_level,
                descriptor,
                evidence,
                evidence_count: 1,
                first_seen_at: now,
                last_seen_at: now,
            });
        }
    }

    profile
        .observations
        .sort_by_key(|observation| std::cmp::Reverse(observation.last_seen_at));
    profile.observations.truncate(80);
    if !profile
        .observed_session_ids
        .iter()
        .any(|seen| seen == session_id)
    {
        profile.observed_session_ids.push(session_id.to_owned());
        profile.conversations_observed = profile.conversations_observed.saturating_add(1);
    }
    profile.processed_turn_ids.push(turn_id.to_owned());
    if profile.processed_turn_ids.len() > 1_000 {
        let drain = profile.processed_turn_ids.len() - 1_000;
        profile.processed_turn_ids.drain(..drain);
    }
    profile.updated_at = now;
}

fn sanitized_overall(mut estimate: CefrEstimate) -> CefrEstimate {
    estimate.confidence = estimate.confidence.clamp(0.0, 1.0);
    estimate.rationale = compact(&estimate.rationale, 240);
    estimate
}

fn compact(value: &str, max_chars: usize) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_patch() -> ProfilePatch {
        ProfilePatch {
            overall: CefrEstimate {
                level: CefrLevel::B1,
                confidence: 0.42,
                rationale: "Can follow the main point but asks about idiomatic nuance.".to_owned(),
            },
            dimensions: Vec::new(),
            observations: vec![PatchObservation {
                dimension: Dimension::Pragmatics,
                cefr_level: CefrLevel::B1,
                descriptor: "Can recognize common attitudes but may need help with implicit irony."
                    .to_owned(),
                evidence: "The follow-up separated literal meaning from intended tone.".to_owned(),
            }],
        }
    }

    #[test]
    fn repeated_cefr_observations_merge() {
        let mut profile = LearnerProfile::default();
        apply_patch(&mut profile, sample_patch(), "session", "turn-1");
        apply_patch(&mut profile, sample_patch(), "session", "turn-2");
        assert_eq!(profile.conversations_observed, 1);
        assert_eq!(profile.observations.len(), 1);
        assert_eq!(profile.observations[0].evidence_count, 2);
    }

    #[test]
    fn public_profile_has_the_frontend_read_model() {
        let profile = LearnerProfile::default();
        let view = public_profile(&profile);
        assert_eq!(view["framework"], "CEFR");
        assert_eq!(view["conversationsObserved"], 0);
        assert_eq!(view["overall"]["level"], "insufficient_evidence");
        assert_eq!(view["dimensions"], json!([]));
        assert_eq!(view["recentObservations"], json!([]));
    }
}
