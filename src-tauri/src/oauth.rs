use crate::{app_state::AppState, network, sessions};
use base64::{Engine as _, engine::general_purpose};
use rand::RngCore;
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, State};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use url::Url;

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const SCOPE: &str = "openid profile email offline_access api.connectors.read api.connectors.invoke";
const ACCOUNT_CLAIM_CONTAINER: &str = "https://api.openai.com/auth";
const ACCOUNT_CLAIM: &str = "chatgpt_account_id";
const TOKEN_FILE: &str = "oauth.json";
const MIN_TOKEN_TTL_MS: i64 = 60_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthToken {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at_ms: i64,
    pub account_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStart {
    pub authorization_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AuthStatus {
    SignedOut,
    Connected { expires_at_ms: i64 },
    Error { message: String },
}

#[derive(Debug, Deserialize)]
struct TokenExchangePayload {
    id_token: String,
    access_token: String,
    refresh_token: String,
}

#[derive(Debug, Deserialize)]
struct TokenRefreshPayload {
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
}

struct PendingCallback {
    stream: TcpStream,
    code: String,
}

pub async fn start_login(app: AppHandle, app_state: &AppState) -> Result<AuthStart, String> {
    let client =
        network::build_client(app_state, Duration::from_secs(10), Duration::from_secs(30))?;
    let data_dir = app_state.data_dir()?;
    let listener = TcpListener::bind(("127.0.0.1", 1455))
        .await
        .map_err(|_| "Gloss couldn't open the local sign-in callback. Close another sign-in window and try again.".to_owned())?;
    let verifier = random_urlsafe(32);
    let challenge = general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_urlsafe(16);
    let mut url =
        Url::parse(AUTHORIZE_URL).map_err(|_| "The sign-in URL is invalid.".to_owned())?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("scope", SCOPE)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state)
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("originator", "gloss");

    tauri::async_runtime::spawn(async move {
        let result = match receive_callback(listener, &state).await {
            Ok(mut callback) => {
                let result = exchange_code(&client, &callback.code, &verifier)
                    .await
                    .and_then(|token| save_token(&data_dir, &token).map(|_| token));
                match &result {
                    Ok(_) => write_callback_response(&mut callback.stream, 200, SUCCESS_HTML).await,
                    Err(message) => {
                        write_callback_response(&mut callback.stream, 400, &error_html(message))
                            .await;
                    }
                }
                result
            }
            Err(message) => Err(message),
        };
        let status = match result {
            Ok(token) => AuthStatus::Connected {
                expires_at_ms: token.expires_at_ms,
            },
            Err(message) => AuthStatus::Error { message },
        };
        let _ = app.emit("auth-status", status);
    });

    Ok(AuthStart {
        authorization_url: url.into(),
    })
}

pub fn status(data_dir: &Path) -> Result<AuthStatus, String> {
    match load_token(data_dir)? {
        Some(token) => Ok(AuthStatus::Connected {
            expires_at_ms: token.expires_at_ms,
        }),
        None => Ok(AuthStatus::SignedOut),
    }
}

pub fn logout(data_dir: &Path) -> Result<(), String> {
    match fs::remove_file(data_dir.join(TOKEN_FILE)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Could not remove the saved sign-in: {error}")),
    }
}

pub async fn valid_token(state: &AppState, force_refresh: bool) -> Result<OAuthToken, String> {
    let _guard = state.auth_lock.lock().await;
    let data_dir = state.data_dir()?;
    let token =
        load_token(&data_dir)?.ok_or_else(|| "Sign in with ChatGPT to continue.".to_owned())?;
    let now = unix_time_ms()?;
    if !force_refresh && token.expires_at_ms - now > MIN_TOKEN_TTL_MS {
        return Ok(token);
    }

    let client = network::build_client(state, Duration::from_secs(10), Duration::from_secs(30))?;
    let refreshed = refresh_token(&client, &token).await.or_else(|message| {
        if !force_refresh && token.expires_at_ms > now {
            Ok(token.clone())
        } else {
            Err(message)
        }
    })?;
    save_token(&data_dir, &refreshed)?;
    Ok(refreshed)
}

async fn receive_callback(
    listener: TcpListener,
    expected_state: &str,
) -> Result<PendingCallback, String> {
    let (mut stream, _) = tokio::time::timeout(Duration::from_secs(120), listener.accept())
        .await
        .map_err(|_| "ChatGPT sign-in timed out. Start a new sign-in.".to_owned())?
        .map_err(|_| "Gloss couldn't receive the ChatGPT sign-in callback.".to_owned())?;
    let result = match read_http_request(&mut stream).await {
        Ok(request) => parse_callback_request(&request, expected_state),
        Err(message) => Err(message),
    };
    match result {
        Ok(code) => Ok(PendingCallback { stream, code }),
        Err(message) => {
            write_callback_response(&mut stream, 400, &error_html(&message)).await;
            Err(message)
        }
    }
}

async fn read_http_request(stream: &mut TcpStream) -> Result<String, String> {
    let mut data = Vec::with_capacity(2048);
    let mut buffer = [0_u8; 2048];
    loop {
        let count = stream
            .read(&mut buffer)
            .await
            .map_err(|_| "Gloss couldn't read the ChatGPT sign-in callback.".to_owned())?;
        if count == 0 {
            break;
        }
        data.extend_from_slice(&buffer[..count]);
        if data.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if data.len() > 16 * 1024 {
            return Err("The ChatGPT sign-in callback was too large.".to_owned());
        }
    }
    String::from_utf8(data).map_err(|_| "The ChatGPT sign-in callback was invalid.".to_owned())
}

fn parse_callback_request(request: &str, expected_state: &str) -> Result<String, String> {
    let first_line = request
        .lines()
        .next()
        .ok_or_else(|| "The ChatGPT sign-in callback was empty.".to_owned())?;
    let mut parts = first_line.split_whitespace();
    if parts.next() != Some("GET") {
        return Err("The ChatGPT sign-in callback used an unsupported method.".to_owned());
    }
    let target = parts
        .next()
        .ok_or_else(|| "The ChatGPT sign-in callback had no URL.".to_owned())?;
    let url = Url::parse(&format!("http://localhost{target}"))
        .map_err(|_| "The ChatGPT sign-in callback URL was invalid.".to_owned())?;
    if url.path() != "/auth/callback" {
        return Err("The ChatGPT sign-in callback path was invalid.".to_owned());
    }
    let params = url
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    let received_state = params
        .get("state")
        .ok_or_else(|| "The ChatGPT sign-in callback had no state.".to_owned())?;
    if received_state.as_ref() != expected_state {
        return Err("The ChatGPT sign-in state did not match. Start again.".to_owned());
    }
    if let Some(error) = params.get("error") {
        return Err(format!("ChatGPT sign-in was not completed ({error})."));
    }
    params
        .get("code")
        .map(|code| code.to_string())
        .filter(|code| !code.is_empty())
        .ok_or_else(|| "The ChatGPT sign-in callback had no authorization code.".to_owned())
}

async fn write_callback_response(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = if status == 200 { "OK" } else { "Bad Request" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

async fn exchange_code(client: &Client, code: &str, verifier: &str) -> Result<OAuthToken, String> {
    let response = client
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", REDIRECT_URI),
        ])
        .send()
        .await
        .map_err(|_| {
            "Gloss couldn't reach the ChatGPT token service. Check the proxy and try again."
                .to_owned()
        })?;
    if !response.status().is_success() {
        let status = response.status();
        let detail = oauth_error_detail(response).await;
        return Err(format!(
            "ChatGPT token exchange failed with HTTP {}{}.",
            status.as_u16(),
            detail.map_or_else(String::new, |message| format!(": {message}"))
        ));
    }
    let payload = response
        .json::<TokenExchangePayload>()
        .await
        .map_err(|_| "ChatGPT returned an invalid token response.".to_owned())?;
    token_from_exchange(payload)
}

async fn refresh_token(client: &Client, old: &OAuthToken) -> Result<OAuthToken, String> {
    let response = client
        .post(TOKEN_URL)
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": &old.refresh_token,
            "client_id": CLIENT_ID,
        }))
        .send()
        .await
        .map_err(|_| {
            "Gloss couldn't refresh the ChatGPT sign-in. Check the proxy and try again.".to_owned()
        })?;
    if !response.status().is_success() {
        let status = response.status();
        let detail = oauth_error_detail(response).await;
        return Err(format!(
            "ChatGPT token refresh failed with HTTP {}{}. Sign in again.",
            status.as_u16(),
            detail.map_or_else(String::new, |message| format!(": {message}"))
        ));
    }
    let payload = response
        .json::<TokenRefreshPayload>()
        .await
        .map_err(|_| "ChatGPT returned an invalid refresh response.".to_owned())?;
    token_from_refresh(payload, old)
}

fn token_from_exchange(payload: TokenExchangePayload) -> Result<OAuthToken, String> {
    if payload.id_token.is_empty()
        || payload.access_token.is_empty()
        || payload.refresh_token.is_empty()
    {
        return Err("ChatGPT returned an incomplete token response.".to_owned());
    }
    let account_id = decode_account_id(&payload.id_token)
        .or_else(|_| decode_account_id(&payload.access_token))?;
    let expires_at_ms =
        decode_expiry_ms(&payload.access_token).or_else(|_| decode_expiry_ms(&payload.id_token))?;
    Ok(OAuthToken {
        access_token: payload.access_token,
        refresh_token: payload.refresh_token,
        expires_at_ms,
        account_id,
    })
}

fn token_from_refresh(
    payload: TokenRefreshPayload,
    old: &OAuthToken,
) -> Result<OAuthToken, String> {
    let access_token = payload
        .access_token
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| old.access_token.clone());
    let refresh_token = payload
        .refresh_token
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| old.refresh_token.clone());
    let account_id = payload
        .id_token
        .as_deref()
        .and_then(|token| decode_account_id(token).ok())
        .or_else(|| decode_account_id(&access_token).ok())
        .unwrap_or_else(|| old.account_id.clone());
    let expires_at_ms = decode_expiry_ms(&access_token)?;
    Ok(OAuthToken {
        access_token,
        refresh_token,
        expires_at_ms,
        account_id,
    })
}

async fn oauth_error_detail(response: Response) -> Option<String> {
    let payload = response.json::<Value>().await.ok()?;
    let message = payload
        .pointer("/error/message")
        .and_then(Value::as_str)
        .or_else(|| payload.get("error_description").and_then(Value::as_str))
        .or_else(|| payload.get("error").and_then(Value::as_str))?;
    let clean = message
        .chars()
        .filter(|character| !character.is_control())
        .take(240)
        .collect::<String>();
    (!clean.is_empty()).then_some(clean)
}

fn decode_claims(token: &str) -> Result<Value, String> {
    let encoded = token
        .split('.')
        .nth(1)
        .ok_or_else(|| "ChatGPT returned an invalid token.".to_owned())?;
    let decoded = general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .or_else(|_| general_purpose::URL_SAFE.decode(encoded))
        .map_err(|_| "ChatGPT returned an invalid token.".to_owned())?;
    serde_json::from_slice(&decoded).map_err(|_| "ChatGPT returned an invalid token.".to_owned())
}

fn decode_account_id(token: &str) -> Result<String, String> {
    decode_claims(token)?
        .get(ACCOUNT_CLAIM_CONTAINER)
        .and_then(|value| value.get(ACCOUNT_CLAIM))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| "The ChatGPT account ID was missing from the token.".to_owned())
}

fn decode_expiry_ms(token: &str) -> Result<i64, String> {
    decode_claims(token)?
        .get("exp")
        .and_then(Value::as_i64)
        .filter(|expiry| *expiry > 0)
        .and_then(|expiry| expiry.checked_mul(1000))
        .ok_or_else(|| "The ChatGPT token had no valid expiration time.".to_owned())
}

fn save_token(data_dir: &Path, token: &OAuthToken) -> Result<(), String> {
    let payload = serde_json::to_vec_pretty(token)
        .map_err(|_| "Could not encode the ChatGPT sign-in.".to_owned())?;
    sessions::atomic_write(&data_dir.join(TOKEN_FILE), &payload)
        .map_err(|error| format!("Could not save the ChatGPT sign-in: {error}"))
}

fn load_token(data_dir: &Path) -> Result<Option<OAuthToken>, String> {
    match fs::read_to_string(data_dir.join(TOKEN_FILE)) {
        Ok(value) => serde_json::from_str(&value).map(Some).map_err(|_| {
            "The saved ChatGPT sign-in is damaged. Sign out and sign in again.".to_owned()
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Could not read the saved ChatGPT sign-in: {error}")),
    }
}

fn random_urlsafe(byte_count: usize) -> String {
    let mut bytes = vec![0_u8; byte_count];
    rand::rng().fill_bytes(&mut bytes);
    general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn unix_time_ms() -> Result<i64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .map_err(|_| "The system clock is invalid.".to_owned())
}

fn error_html(message: &str) -> String {
    let safe = message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    CALLBACK_PAGE
        .replace("{{title}}", "Sign-in failed")
        .replace(
            "{{message}}",
            &format!("{safe} Return to Gloss and try again."),
        )
}

const SUCCESS_HTML: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Connected to Gloss</title><style>html{font-family:"Segoe UI Variable",Segoe UI,sans-serif;color:#18201d;background:#f5f7f4}body{min-height:100vh;margin:0;display:grid;place-items:center}.card{width:min(360px,calc(100vw - 48px));padding:36px;border-radius:28px;background:#fff;box-shadow:0 24px 70px rgba(32,43,38,.14);text-align:center}.mark{width:52px;height:52px;margin:auto;display:grid;place-items:center;border-radius:18px;background:#dff4e7;color:#126b40;font-size:26px}h1{font-size:22px;margin:20px 0 8px}p{color:#5e6863;line-height:1.55;margin:0}</style></head><body><main class="card"><div class="mark">✓</div><h1>Connected to Gloss</h1><p>You can close this tab and return to your reading.</p></main></body></html>"#;

const CALLBACK_PAGE: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{{title}}</title><style>html{font-family:"Segoe UI Variable",Segoe UI,sans-serif;color:#231d1d;background:#f8f5f4}body{min-height:100vh;margin:0;display:grid;place-items:center}.card{width:min(360px,calc(100vw - 48px));padding:36px;border-radius:28px;background:#fff;box-shadow:0 24px 70px rgba(56,35,35,.14);text-align:center}.mark{width:52px;height:52px;margin:auto;display:grid;place-items:center;border-radius:18px;background:#fae6e3;color:#a23d34;font-size:25px;font-weight:700}h1{font-size:22px;margin:20px 0 8px}p{color:#73615f;line-height:1.55;margin:0}</style></head><body><main class="card"><div class="mark">!</div><h1>{{title}}</h1><p>{{message}}</p></main></body></html>"#;

#[tauri::command]
pub async fn start_oauth_login(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<AuthStart, String> {
    start_login(app, &state).await
}

#[tauri::command]
pub async fn get_auth_status(state: State<'_, AppState>) -> Result<AuthStatus, String> {
    let data_dir = state.data_dir()?;
    tauri::async_runtime::spawn_blocking(move || status(&data_dir))
        .await
        .map_err(|error| format!("Could not read the sign-in status: {error}"))?
}

#[tauri::command]
pub async fn logout_oauth(state: State<'_, AppState>) -> Result<(), String> {
    let _guard = state.auth_lock.lock().await;
    let data_dir = state.data_dir()?;
    tauri::async_runtime::spawn_blocking(move || logout(&data_dir))
        .await
        .map_err(|error| format!("Could not sign out: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_requires_matching_state() {
        let request = "GET /auth/callback?code=abc&state=wrong HTTP/1.1\r\n\r\n";
        assert!(parse_callback_request(request, "expected").is_err());
    }

    #[test]
    fn callback_extracts_code() {
        let request = "GET /auth/callback?code=abc&state=expected HTTP/1.1\r\n\r\n";
        assert_eq!(parse_callback_request(request, "expected").unwrap(), "abc");
    }

    #[test]
    fn exchange_tokens_use_jwt_expiry_without_expires_in() {
        let account_claims = serde_json::json!({
            "exp": 4_100_000_000_i64,
            (ACCOUNT_CLAIM_CONTAINER): { (ACCOUNT_CLAIM): "account-123" }
        });
        let access_claims = serde_json::json!({ "exp": 4_000_000_000_i64 });
        let token = token_from_exchange(TokenExchangePayload {
            id_token: unsigned_test_jwt(&account_claims),
            access_token: unsigned_test_jwt(&access_claims),
            refresh_token: "refresh-token".to_owned(),
        })
        .unwrap();
        assert_eq!(token.account_id, "account-123");
        assert_eq!(token.expires_at_ms, 4_000_000_000_000);
    }

    #[test]
    fn large_token_round_trips_through_plaintext_file() {
        let data_dir = std::env::temp_dir().join(format!("gloss-oauth-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&data_dir).unwrap();
        let token = OAuthToken {
            access_token: "a".repeat(6_000),
            refresh_token: "r".repeat(6_000),
            expires_at_ms: 4_000_000_000_000,
            account_id: "account-123".to_owned(),
        };

        save_token(&data_dir, &token).unwrap();
        let loaded = load_token(&data_dir).unwrap().unwrap();

        assert_eq!(loaded.access_token, token.access_token);
        assert_eq!(loaded.refresh_token, token.refresh_token);
        assert_eq!(loaded.expires_at_ms, token.expires_at_ms);
        assert_eq!(loaded.account_id, token.account_id);
        logout(&data_dir).unwrap();
        assert!(!data_dir.join(TOKEN_FILE).exists());
        fs::remove_dir(&data_dir).unwrap();
    }

    fn unsigned_test_jwt(claims: &Value) -> String {
        let encoded = general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap());
        format!("header.{encoded}.signature")
    }
}
