use crate::{app_state::AppState, network, oauth};
use futures_util::StreamExt;
use rand::Rng;
use reqwest::{StatusCode, header};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

const RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
const MAX_AUTOMATIC_RETRIES: u8 = 2;

#[derive(Debug)]
pub struct StreamResult {
    pub text: String,
    pub response_id: Option<String>,
    pub output_items: Vec<Value>,
    pub usage: Option<Value>,
    pub transport_attempts: u8,
}

#[derive(Debug)]
pub struct StreamFailure {
    pub message: String,
    pub partial: String,
    pub incomplete: bool,
    pub transport_attempts: u8,
}

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AgentEvent<'a> {
    Delta {
        #[serde(rename = "sessionId")]
        session_id: &'a str,
        #[serde(rename = "attemptId")]
        attempt_id: &'a str,
        delta: &'a str,
    },
}

#[derive(Default)]
struct ParsedStream {
    text: String,
    response_id: Option<String>,
    output_items: Vec<Value>,
    usage: Option<Value>,
    completed: bool,
    terminal_error: Option<String>,
}

pub async fn stream_response(
    app: &AppHandle,
    state: &AppState,
    session_id: &str,
    attempt_id: &str,
    request_body: &Value,
    emit_deltas: bool,
) -> Result<StreamResult, StreamFailure> {
    let client = network::build_client(state, Duration::from_secs(10), Duration::from_secs(180))
        .map_err(|message| failure(&message, 0))?;
    let mut automatic_retries = 0_u8;
    let mut transport_attempts = 0_u8;
    let mut force_refresh_next = false;
    let mut unauthorized_refresh_used = false;

    loop {
        transport_attempts += 1;
        let token = oauth::valid_token(state, force_refresh_next)
            .await
            .map_err(|message| failure(&message, transport_attempts))?;
        force_refresh_next = false;
        let response = client
            .post(RESPONSES_URL)
            .bearer_auth(&token.access_token)
            .header("chatgpt-account-id", &token.account_id)
            .header("OpenAI-Beta", "responses=experimental")
            .header("originator", "gloss")
            .header(header::USER_AGENT, "Gloss/0.1.0")
            .header(header::ACCEPT, "text/event-stream")
            .json(request_body)
            .send()
            .await;

        let response = match response {
            Ok(response) => response,
            Err(error) => {
                if automatic_retries < MAX_AUTOMATIC_RETRIES {
                    automatic_retries += 1;
                    retry_delay(automatic_retries, None).await;
                    continue;
                }
                return Err(StreamFailure {
                    message: friendly_transport_error(&error),
                    partial: String::new(),
                    incomplete: false,
                    transport_attempts,
                });
            }
        };

        if response.status() == StatusCode::UNAUTHORIZED && !unauthorized_refresh_used {
            unauthorized_refresh_used = true;
            force_refresh_next = true;
            continue;
        }

        if !response.status().is_success() {
            let status = response.status();
            let retry_after = response
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok());
            if is_transient_status(status) && automatic_retries < MAX_AUTOMATIC_RETRIES {
                automatic_retries += 1;
                retry_delay(automatic_retries, retry_after).await;
                continue;
            }
            let detail = response
                .json::<Value>()
                .await
                .ok()
                .and_then(|body| response_error_message(&body));
            return Err(StreamFailure {
                message: friendly_http_error(status, detail.as_deref()),
                partial: String::new(),
                incomplete: false,
                transport_attempts,
            });
        }

        let mut parsed = ParsedStream::default();
        let mut pending = Vec::new();
        let mut stream = response.bytes_stream();
        let mut stream_error = None;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) => {
                    pending.extend_from_slice(&chunk);
                    for event in drain_sse_events(&mut pending) {
                        if let Err(message) = handle_sse_event(
                            app,
                            session_id,
                            attempt_id,
                            &event,
                            &mut parsed,
                            emit_deltas,
                        ) {
                            stream_error = Some(message);
                            break;
                        }
                    }
                    if stream_error.is_some() || parsed.completed || parsed.terminal_error.is_some()
                    {
                        break;
                    }
                }
                Err(error) => {
                    stream_error = Some(friendly_transport_error(&error));
                    break;
                }
            }
        }
        if stream_error.is_none() && !pending.is_empty() {
            let mut final_event = pending;
            if !final_event.ends_with(b"\n\n") && !final_event.ends_with(b"\r\n\r\n") {
                final_event.extend_from_slice(b"\n\n");
            }
            for event in drain_sse_events(&mut final_event) {
                if let Err(message) = handle_sse_event(
                    app,
                    session_id,
                    attempt_id,
                    &event,
                    &mut parsed,
                    emit_deltas,
                ) {
                    stream_error = Some(message);
                    break;
                }
            }
        }

        if let Some(message) = parsed.terminal_error.or(stream_error) {
            if parsed.text.is_empty() && automatic_retries < MAX_AUTOMATIC_RETRIES {
                automatic_retries += 1;
                retry_delay(automatic_retries, None).await;
                continue;
            }
            return Err(StreamFailure {
                message,
                partial: parsed.text,
                incomplete: true,
                transport_attempts,
            });
        }
        if !parsed.completed {
            if parsed.text.is_empty() && automatic_retries < MAX_AUTOMATIC_RETRIES {
                automatic_retries += 1;
                retry_delay(automatic_retries, None).await;
                continue;
            }
            return Err(StreamFailure {
                message: "The response ended before it was complete. You can retry safely."
                    .to_owned(),
                partial: parsed.text,
                incomplete: true,
                transport_attempts,
            });
        }
        return Ok(StreamResult {
            text: parsed.text,
            response_id: parsed.response_id,
            output_items: parsed.output_items,
            usage: parsed.usage,
            transport_attempts,
        });
    }
}

fn handle_sse_event(
    app: &AppHandle,
    session_id: &str,
    attempt_id: &str,
    event: &[u8],
    parsed: &mut ParsedStream,
    emit_deltas: bool,
) -> Result<(), String> {
    let event = std::str::from_utf8(event)
        .map_err(|_| "ChatGPT returned an invalid streaming response.".to_owned())?
        .replace("\r\n", "\n");
    let data = event
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n");
    if data.is_empty() || data == "[DONE]" {
        return Ok(());
    }
    let value: Value = serde_json::from_str(&data)
        .map_err(|_| "ChatGPT returned an invalid streaming event.".to_owned())?;
    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match event_type {
        "response.output_text.delta" | "response.refusal.delta" => {
            if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                parsed.text.push_str(delta);
                if emit_deltas {
                    let _ = app.emit(
                        "agent-event",
                        AgentEvent::Delta {
                            session_id,
                            attempt_id,
                            delta,
                        },
                    );
                }
            }
        }
        "response.output_item.done" => {
            if let Some(item) = value.get("item") {
                parsed.output_items.push(item.clone());
            }
        }
        "response.completed" => {
            if let Some(response) = value.get("response") {
                parsed.response_id = response
                    .get("id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                if let Some(output) = response.get("output").and_then(Value::as_array) {
                    parsed.output_items = output.clone();
                }
                parsed.usage = response.get("usage").cloned();
            }
            parsed.completed = true;
        }
        "response.incomplete" => {
            parsed.terminal_error = Some(
                value
                    .pointer("/response/incomplete_details/reason")
                    .and_then(Value::as_str)
                    .map(|reason| format!("The response was incomplete ({reason})."))
                    .unwrap_or_else(|| {
                        "The response was incomplete. You can retry safely.".to_owned()
                    }),
            );
        }
        "response.failed" => {
            parsed.terminal_error = Some(
                value
                    .pointer("/response/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("ChatGPT could not complete this response.")
                    .to_owned(),
            );
        }
        "error" => {
            parsed.terminal_error = Some(
                value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("ChatGPT returned an error.")
                    .to_owned(),
            );
        }
        _ => {}
    }
    Ok(())
}

fn drain_sse_events(buffer: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut events = Vec::new();
    loop {
        let boundary = buffer
            .windows(2)
            .position(|window| window == b"\n\n")
            .map(|index| (index, 2))
            .or_else(|| {
                buffer
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|index| (index, 4))
            });
        let Some((index, separator_len)) = boundary else {
            break;
        };
        let event = buffer[..index].to_vec();
        buffer.drain(..index + separator_len);
        if !event.is_empty() {
            events.push(event);
        }
    }
    events
}

fn is_transient_status(status: StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 409 | 429) || status.is_server_error()
}

async fn retry_delay(attempt: u8, retry_after_secs: Option<u64>) {
    let base_ms = retry_after_secs
        .map(|seconds| seconds.saturating_mul(1_000))
        .unwrap_or_else(|| 450_u64.saturating_mul(2_u64.pow((attempt - 1) as u32)));
    let jitter = rand::rng().random_range(0..=220_u64);
    tokio::time::sleep(Duration::from_millis((base_ms + jitter).min(10_000))).await;
}

fn response_error_message(body: &Value) -> Option<String> {
    body.pointer("/error/message")
        .or_else(|| body.get("detail"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn friendly_http_error(status: StatusCode, detail: Option<&str>) -> String {
    match status.as_u16() {
        400 | 422 => detail
            .unwrap_or("The request was not accepted by ChatGPT.")
            .to_owned(),
        401 => "Your ChatGPT sign-in expired. Sign in again and retry.".to_owned(),
        403 => "This ChatGPT account cannot use the requested Codex model.".to_owned(),
        404 => "The Codex Responses endpoint or model is unavailable.".to_owned(),
        429 => "ChatGPT is busy or your account is rate-limited. Try again shortly.".to_owned(),
        code if code >= 500 => "ChatGPT is temporarily unavailable. Try again shortly.".to_owned(),
        code => detail
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("ChatGPT returned HTTP {code}.")),
    }
}

fn friendly_transport_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "The ChatGPT request timed out. Check your connection and retry.".to_owned()
    } else if error.is_connect() {
        "Gloss couldn't reach ChatGPT. Check your connection and retry.".to_owned()
    } else {
        "The ChatGPT connection ended unexpectedly. You can retry safely.".to_owned()
    }
}

fn failure(message: &str, transport_attempts: u8) -> StreamFailure {
    StreamFailure {
        message: message.to_owned(),
        partial: String::new(),
        incomplete: false,
        transport_attempts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drains_lf_and_crlf_events() {
        let mut input = b"data: {\"type\":\"a\"}\n\ndata: {\"type\":\"b\"}\r\n\r\ntail".to_vec();
        let events = drain_sse_events(&mut input);
        assert_eq!(events.len(), 2);
        assert_eq!(input, b"tail");
    }

    #[test]
    fn recognizes_only_agreed_transient_statuses() {
        assert!(is_transient_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_transient_status(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(!is_transient_status(StatusCode::FORBIDDEN));
        assert!(!is_transient_status(StatusCode::UNPROCESSABLE_ENTITY));
    }
}
