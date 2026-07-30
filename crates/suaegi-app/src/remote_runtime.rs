//! Remote Suaegi/Orca runtime pairing metadata and reachability checks.
//!
//! Pairing credentials are stored in the OS keychain. Persisted settings keep
//! only the display name and endpoint so diagnostic exports cannot leak tokens.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use crypto_box::aead::{AeadCore, AeadInPlace, OsRng};
use crypto_box::{PublicKey, SalsaBox, SecretKey};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use suaegi_core::domain::{ManagedProviderAccountSetting, RuntimeEnvironmentSetting};
use suaegi_secrets::{Secret, SecretRequest};
use tokio_tungstenite::tungstenite::Message;

const SERVICE: &str = "suaegi-remote-runtime";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeReachability {
    pub reachable: bool,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteUpdatePhase {
    Checking,
    Available,
    Current,
    Manual,
    Updating,
    Updated,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemoteServerUpdateState {
    pub phase: RemoteUpdatePhase,
    pub current_version: Option<String>,
    pub target_version: Option<String>,
    pub progress: Option<f64>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RemoteProviderAccounts {
    pub claude: Vec<ManagedProviderAccountSetting>,
    pub active_claude: Option<String>,
    pub codex: Vec<ManagedProviderAccountSetting>,
    pub active_codex: Option<String>,
    pub claude_limits: Option<crate::rate_limits::ProviderRateLimits>,
    pub codex_limits: Option<crate::rate_limits::ProviderRateLimits>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PairingOffer {
    v: u8,
    endpoint: String,
    #[serde(rename = "deviceToken")]
    device_token: String,
    #[serde(rename = "publicKeyB64")]
    public_key_b64: String,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    relay: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize)]
struct StoredCredentials {
    device_token: String,
    public_key_b64: String,
}

fn pairing_payload(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Pairing code is required.".into());
    }
    if trimmed.to_ascii_lowercase().starts_with("orca://") {
        let url = url::Url::parse(trimmed).map_err(|_| "Invalid pairing URL.".to_string())?;
        if url.scheme() != "orca"
            || url.host_str() != Some("pair")
            || !["", "/"].contains(&url.path())
        {
            return Err("Pairing URL must use orca://pair.".into());
        }
        return url
            .query_pairs()
            .find(|(name, _)| name == "code")
            .map(|(_, value)| value.into_owned())
            .ok_or_else(|| "Pairing URL is missing its code.".to_string());
    }
    Ok(trimmed.to_string())
}

fn parse_pairing_offer(input: &str) -> Result<PairingOffer, String> {
    let payload = pairing_payload(input)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&payload)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(&payload))
        .map_err(|_| "Pairing code is not valid base64url.".to_string())?;
    let offer: PairingOffer = serde_json::from_slice(&bytes)
        .map_err(|_| "Pairing code is not valid JSON.".to_string())?;
    if offer.v != 2 {
        return Err("Unsupported pairing-code version.".into());
    }
    if offer.scope.as_deref() == Some("mobile") {
        return Err("This is a mobile pairing code, not a remote runtime code.".into());
    }
    if offer.relay.is_some() {
        return Err("Hosted mobile relay pairing is not supported.".into());
    }
    let endpoint =
        url::Url::parse(&offer.endpoint).map_err(|_| "Pairing endpoint is invalid.".to_string())?;
    if !matches!(endpoint.scheme(), "ws" | "wss") || endpoint.host_str().is_none() {
        return Err("Remote runtime endpoint must use ws:// or wss://.".into());
    }
    if offer.device_token.is_empty()
        || offer.device_token.len() > 4096
        || offer.public_key_b64.is_empty()
        || offer.public_key_b64.len() > 4096
    {
        return Err("Pairing credentials are invalid.".into());
    }
    decode_public_key(&offer.public_key_b64)?;
    Ok(offer)
}

pub(crate) fn validate_runtime_pairing_code(input: &str) -> Result<(), String> {
    parse_pairing_offer(input).map(|_| ())
}

fn environment_id(name: &str, endpoint: &str, now: u64) -> String {
    let mut digest = Sha256::new();
    digest.update(name.as_bytes());
    digest.update([0]);
    digest.update(endpoint.as_bytes());
    digest.update(now.to_le_bytes());
    let hash = digest.finalize();
    format!(
        "runtime-{}",
        hash[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

pub async fn save_environment(
    name: String,
    pairing_code: String,
) -> Result<RuntimeEnvironmentSetting, String> {
    tokio::task::spawn_blocking(move || {
        let name = name.trim();
        if name.is_empty() {
            return Err("Server name is required.".to_string());
        }
        let offer = parse_pairing_offer(&pairing_code)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        let id = environment_id(name, &offer.endpoint, now);
        let credentials = StoredCredentials {
            device_token: offer.device_token,
            public_key_b64: offer.public_key_b64,
        };
        let encoded = serde_json::to_string(&credentials)
            .map_err(|_| "Could not encode runtime credentials.".to_string())?;
        suaegi_secrets::store(SERVICE, &id, &Secret::new(encoded))
            .map_err(|error| format!("Could not save runtime credentials: {error}"))?;
        Ok(RuntimeEnvironmentSetting {
            id,
            name: name.chars().take(100).collect(),
            endpoint: offer.endpoint,
            credentials_configured: true,
            created_at_unix_ms: now,
        })
    })
    .await
    .map_err(|error| format!("Runtime pairing task failed: {error}"))?
}

/// Replaces the endpoint and credentials for a durable runtime environment
/// while preserving its stable id, display name, and creation timestamp.
pub async fn update_environment(
    environment: RuntimeEnvironmentSetting,
    pairing_code: String,
) -> Result<RuntimeEnvironmentSetting, String> {
    tokio::task::spawn_blocking(move || {
        let offer = parse_pairing_offer(&pairing_code)?;
        let credentials = StoredCredentials {
            device_token: offer.device_token,
            public_key_b64: offer.public_key_b64,
        };
        let encoded = serde_json::to_string(&credentials)
            .map_err(|_| "Could not encode runtime credentials.".to_string())?;
        suaegi_secrets::store(SERVICE, &environment.id, &Secret::new(encoded))
            .map_err(|error| format!("Could not update runtime credentials: {error}"))?;
        Ok(RuntimeEnvironmentSetting {
            endpoint: offer.endpoint,
            credentials_configured: true,
            ..environment
        })
    })
    .await
    .map_err(|error| format!("Runtime pairing update task failed: {error}"))?
}

pub async fn remove_environment(id: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        suaegi_secrets::delete(SERVICE, &id)
            .map_err(|error| format!("Could not delete runtime credentials: {error}"))
    })
    .await
    .map_err(|error| format!("Runtime removal task failed: {error}"))?
}

fn load_credentials(id: &str) -> Result<StoredCredentials, String> {
    let resolved = suaegi_secrets::load(&SecretRequest::new(SERVICE, id));
    let secret = resolved
        .secret
        .ok_or_else(|| "Pairing credentials are missing from the Keychain.".to_string())?;
    serde_json::from_str(secret.expose())
        .map_err(|_| "Saved runtime credentials are invalid.".to_string())
}

fn decode_public_key(value: &str) -> Result<PublicKey, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|_| "Saved runtime public key is invalid.".to_string())?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "Saved runtime public key must be 32 bytes.".to_string())?;
    Ok(PublicKey::from(bytes))
}

fn encrypt_text(cipher: &SalsaBox, plaintext: &str) -> Result<String, String> {
    let nonce = SalsaBox::generate_nonce(&mut OsRng);
    let mut ciphertext = plaintext.as_bytes().to_vec();
    let tag = cipher
        .encrypt_in_place_detached(&nonce, b"", &mut ciphertext)
        .map_err(|_| "Could not encrypt the runtime request.".to_string())?;
    let mut frame = Vec::with_capacity(nonce.len() + tag.len() + ciphertext.len());
    frame.extend_from_slice(&nonce);
    // tweetnacl's box.after wire format is nonce || authenticator || ciphertext.
    frame.extend_from_slice(&tag);
    frame.extend_from_slice(&ciphertext);
    Ok(base64::engine::general_purpose::STANDARD.encode(frame))
}

fn decrypt_text(cipher: &SalsaBox, frame: &str) -> Result<String, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(frame)
        .map_err(|_| "Remote runtime returned invalid encrypted data.".to_string())?;
    if bytes.len() < 24 + 16 {
        return Err("Remote runtime returned a truncated encrypted frame.".into());
    }
    let nonce = crypto_box::Nonce::from_slice(&bytes[..24]);
    let tag = crypto_box::Tag::from_slice(&bytes[24..40]);
    let mut plaintext = bytes[40..].to_vec();
    cipher
        .decrypt_in_place_detached(nonce, b"", &mut plaintext, tag)
        .map_err(|_| "Remote runtime returned an undecryptable frame.".to_string())?;
    String::from_utf8(plaintext).map_err(|_| "Remote runtime returned non-UTF-8 data.".to_string())
}

async fn receive_text<S>(socket: &mut S) -> Result<String, String>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => return Ok(text.to_string()),
            Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
            Some(Ok(Message::Close(_))) | None => {
                return Err("Remote runtime closed the connection.".into());
            }
            Some(Ok(_)) => return Err("Remote runtime returned an unexpected frame.".into()),
            Some(Err(_)) => return Err("Remote runtime connection failed.".into()),
        }
    }
}

async fn request_with_credentials(
    environment: &RuntimeEnvironmentSetting,
    credentials: StoredCredentials,
    method: &str,
    params: serde_json::Value,
    orchestration_request_id: Option<&str>,
) -> Result<serde_json::Value, String> {
    let server_public_key = decode_public_key(&credentials.public_key_b64)?;
    let client_secret = SecretKey::generate(&mut OsRng);
    let client_public = client_secret.public_key();
    let cipher = SalsaBox::new(&server_public_key, &client_secret);
    let (mut socket, _) = tokio_tungstenite::connect_async(&environment.endpoint)
        .await
        .map_err(|_| "Could not connect to the remote runtime.".to_string())?;

    socket
        .send(Message::Text(
            serde_json::json!({
                "type": "e2ee_hello",
                "publicKeyB64": base64::engine::general_purpose::STANDARD
                    .encode(client_public.as_bytes())
            })
            .to_string()
            .into(),
        ))
        .await
        .map_err(|_| "Could not start the E2EE handshake.".to_string())?;
    let ready = receive_text(&mut socket).await?;
    let ready: serde_json::Value = serde_json::from_str(&ready)
        .map_err(|_| "Remote runtime returned an invalid handshake frame.".to_string())?;
    if ready.get("type").and_then(serde_json::Value::as_str) != Some("e2ee_ready") {
        return Err("Remote runtime rejected the E2EE handshake.".into());
    }

    let auth = encrypt_text(
        &cipher,
        &serde_json::json!({
            "type": "e2ee_auth",
            "deviceToken": credentials.device_token
        })
        .to_string(),
    )?;
    socket
        .send(Message::Text(auth.into()))
        .await
        .map_err(|_| "Could not authenticate with the remote runtime.".to_string())?;
    let authenticated = decrypt_text(&cipher, &receive_text(&mut socket).await?)?;
    let authenticated: serde_json::Value = serde_json::from_str(&authenticated)
        .map_err(|_| "Remote runtime returned an invalid authentication frame.".to_string())?;
    if authenticated
        .get("type")
        .and_then(serde_json::Value::as_str)
        != Some("e2ee_authenticated")
    {
        return Err("Remote runtime rejected the pairing token.".into());
    }

    let request_id = format!(
        "suaegi-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    );
    let mut request_payload = serde_json::json!({
        "id": request_id,
        "deviceToken": credentials.device_token,
        "method": method,
        "params": params
    });
    if let Some(orchestration_request_id) = orchestration_request_id {
        request_payload["orchestrationRequestId"] =
            serde_json::Value::String(orchestration_request_id.to_string());
    }
    let request = encrypt_text(&cipher, &request_payload.to_string())?;
    socket
        .send(Message::Text(request.into()))
        .await
        .map_err(|_| "Could not send the remote runtime request.".to_string())?;
    loop {
        let response = decrypt_text(&cipher, &receive_text(&mut socket).await?)?;
        let response: serde_json::Value = serde_json::from_str(&response)
            .map_err(|_| "Remote runtime returned an invalid RPC response.".to_string())?;
        if response.get("_keepalive").is_some() {
            continue;
        }
        if response.get("id").and_then(serde_json::Value::as_str) != Some(&request_id) {
            continue;
        }
        if let Some(error) = response.get("error") {
            let message = error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Remote runtime request failed.");
            return Err(message.chars().take(180).collect());
        }
        return response
            .get("result")
            .cloned()
            .ok_or_else(|| "Remote runtime RPC response has no result.".to_string());
    }
}

async fn credentials_for_environment(id: String) -> Result<StoredCredentials, String> {
    match tokio::task::spawn_blocking(move || load_credentials(&id)).await {
        Ok(result) => result,
        Err(error) => Err(format!("Could not read runtime credentials: {error}")),
    }
}

/// Call one Orca runtime RPC method over the paired E2EE WebSocket.
///
/// A fresh authenticated connection is used for each bounded request. Streaming
/// methods use a separate long-lived transport; this function intentionally
/// covers request/response operations such as `repo.list`, `files.read`, and
/// `git.status`.
pub async fn request(
    environment: RuntimeEnvironmentSetting,
    method: impl Into<String>,
    params: serde_json::Value,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let id = environment.id.clone();
    let credentials = credentials_for_environment(id).await?;
    let method = method.into();
    match tokio::time::timeout(
        timeout,
        request_with_credentials(&environment, credentials, &method, params, None),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(format!(
            "Timed out waiting for remote runtime method {method}."
        )),
    }
}

/// Sends an idempotent orchestration mutation using Orca's durable request-id
/// envelope. Retrying with the same id is safe on compatible runtimes.
pub async fn request_orchestration(
    environment: RuntimeEnvironmentSetting,
    method: impl Into<String>,
    params: serde_json::Value,
    request_id: String,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    if request_id.trim().is_empty() || request_id.len() > 2_048 {
        return Err("Orchestration request id is invalid.".into());
    }
    let credentials = credentials_for_environment(environment.id.clone()).await?;
    let method = method.into();
    match tokio::time::timeout(
        timeout,
        request_with_credentials(
            &environment,
            credentials,
            &method,
            params,
            Some(&request_id),
        ),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(format!(
            "Timed out waiting for remote orchestration method {method}."
        )),
    }
}

/// Runs the JSON terminal subscription used by desktop Orca clients.
///
/// Input is forwarded as `terminal.send` requests on the already-authenticated
/// subscription socket, avoiding a new E2EE handshake for every keystroke.
#[derive(Debug)]
pub enum TerminalStreamInput {
    Data(Vec<u8>),
    Resize { rows: u16, cols: u16 },
}

pub async fn stream_terminal(
    environment: RuntimeEnvironmentSetting,
    terminal: String,
    rows: u16,
    cols: u16,
    mut input: tokio::sync::mpsc::Receiver<TerminalStreamInput>,
) -> Result<(), String> {
    use std::io::Write as _;

    let credentials = credentials_for_environment(environment.id.clone()).await?;
    let server_public_key = decode_public_key(&credentials.public_key_b64)?;
    let client_secret = SecretKey::generate(&mut OsRng);
    let client_public = client_secret.public_key();
    let cipher = SalsaBox::new(&server_public_key, &client_secret);
    let (mut socket, _) = tokio_tungstenite::connect_async(&environment.endpoint)
        .await
        .map_err(|_| "Could not connect the remote terminal stream.".to_string())?;
    socket
        .send(Message::Text(
            serde_json::json!({
                "type": "e2ee_hello",
                "publicKeyB64": base64::engine::general_purpose::STANDARD
                    .encode(client_public.as_bytes())
            })
            .to_string()
            .into(),
        ))
        .await
        .map_err(|_| "Could not start the terminal E2EE handshake.".to_string())?;
    let ready: serde_json::Value = serde_json::from_str(&receive_text(&mut socket).await?)
        .map_err(|_| "Remote terminal returned an invalid handshake.".to_string())?;
    if ready.get("type").and_then(serde_json::Value::as_str) != Some("e2ee_ready") {
        return Err("Remote terminal rejected the E2EE handshake.".to_string());
    }
    let auth = encrypt_text(
        &cipher,
        &serde_json::json!({
            "type": "e2ee_auth",
            "deviceToken": credentials.device_token,
            "clientCapabilities": []
        })
        .to_string(),
    )?;
    socket
        .send(Message::Text(auth.into()))
        .await
        .map_err(|_| "Could not authenticate the remote terminal stream.".to_string())?;
    let authenticated = decrypt_text(&cipher, &receive_text(&mut socket).await?)?;
    let authenticated: serde_json::Value = serde_json::from_str(&authenticated)
        .map_err(|_| "Remote terminal returned invalid authentication.".to_string())?;
    if authenticated
        .get("type")
        .and_then(serde_json::Value::as_str)
        != Some("e2ee_authenticated")
    {
        return Err("Remote terminal rejected the pairing token.".to_string());
    }

    let stream_id = format!(
        "suaegi-terminal-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    );
    let subscribe = encrypt_text(
        &cipher,
        &serde_json::json!({
            "id": stream_id,
            "deviceToken": credentials.device_token,
            "method": "terminal.subscribe",
            "params": {
                "terminal": terminal,
                "client": {"id": format!("suaegi-{}", std::process::id()), "type": "desktop"},
                "viewport": {"rows": rows, "cols": cols}
            }
        })
        .to_string(),
    )?;
    socket
        .send(Message::Text(subscribe.into()))
        .await
        .map_err(|_| "Could not subscribe to remote terminal output.".to_string())?;

    let mut request_sequence = 0_u64;
    loop {
        tokio::select! {
            input_event = input.recv() => {
                let Some(input_event) = input_event else {
                    let close_id = format!("{stream_id}-close");
                    let close = encrypt_text(
                        &cipher,
                        &serde_json::json!({
                            "id": close_id,
                            "deviceToken": credentials.device_token,
                            "method": "terminal.close",
                            "params": {"terminal": terminal}
                        }).to_string(),
                    )?;
                    let _ = socket.send(Message::Text(close.into())).await;
                    return Ok(());
                };
                request_sequence = request_sequence.saturating_add(1);
                let (text, viewport, claim_viewport) = match input_event {
                    TerminalStreamInput::Data(input_bytes) => {
                        if input_bytes.is_empty() {
                            continue;
                        }
                        (
                            String::from_utf8_lossy(&input_bytes).into_owned(),
                            serde_json::Value::Null,
                            false,
                        )
                    }
                    TerminalStreamInput::Resize { rows, cols } => (
                        String::new(),
                        serde_json::json!({"rows": rows, "cols": cols}),
                        true,
                    ),
                };
                let send = encrypt_text(
                    &cipher,
                    &serde_json::json!({
                        "id": format!("{stream_id}-send-{request_sequence}"),
                        "deviceToken": credentials.device_token,
                        "method": "terminal.send",
                        "params": {
                            "terminal": terminal,
                            "text": text,
                            "client": {"id": format!("suaegi-{}", std::process::id()), "type": "desktop"},
                            "viewport": viewport,
                            "claimViewport": claim_viewport
                        }
                    }).to_string(),
                )?;
                socket.send(Message::Text(send.into())).await
                    .map_err(|_| "Could not send remote terminal input.".to_string())?;
            }
            frame = socket.next() => {
                let frame = match frame {
                    Some(Ok(Message::Text(frame))) => frame.to_string(),
                    Some(Ok(Message::Ping(payload))) => {
                        socket.send(Message::Pong(payload)).await
                            .map_err(|_| "Could not answer remote terminal keepalive.".to_string())?;
                        continue;
                    }
                    Some(Ok(Message::Pong(_))) => continue,
                    Some(Ok(Message::Close(_))) | None => {
                        return Err("Remote terminal stream closed.".to_string());
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(_)) => return Err("Remote terminal stream failed.".to_string()),
                };
                let plaintext = decrypt_text(&cipher, &frame)?;
                let response: serde_json::Value = serde_json::from_str(&plaintext)
                    .map_err(|_| "Remote terminal returned an invalid stream frame.".to_string())?;
                if response.get("_keepalive").is_some() {
                    continue;
                }
                if response.get("id").and_then(serde_json::Value::as_str) != Some(&stream_id) {
                    continue;
                }
                if let Some(error) = response.get("error") {
                    return Err(error
                        .get("message")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("Remote terminal subscription failed.")
                        .to_string());
                }
                let Some(event) = response.get("result") else {
                    continue;
                };
                match event.get("type").and_then(serde_json::Value::as_str) {
                    Some("scrollback") => {
                        if let Some(serialized) = event.get("serialized").and_then(serde_json::Value::as_str) {
                            std::io::stdout().write_all(serialized.as_bytes())
                                .map_err(|error| error.to_string())?;
                        } else if let Some(lines) = event.get("lines").and_then(serde_json::Value::as_array) {
                            for line in lines.iter().filter_map(serde_json::Value::as_str) {
                                std::io::stdout().write_all(line.as_bytes())
                                    .and_then(|()| std::io::stdout().write_all(b"\r\n"))
                                    .map_err(|error| error.to_string())?;
                            }
                        }
                        std::io::stdout().flush().map_err(|error| error.to_string())?;
                    }
                    Some("subscribed") => {
                        if let Some(lines) = event.get("lines").and_then(serde_json::Value::as_array) {
                            for line in lines.iter().filter_map(serde_json::Value::as_str) {
                                std::io::stdout().write_all(line.as_bytes())
                                    .and_then(|()| std::io::stdout().write_all(b"\r\n"))
                                    .map_err(|error| error.to_string())?;
                            }
                            std::io::stdout().flush().map_err(|error| error.to_string())?;
                        }
                    }
                    Some("data") => {
                        if let Some(chunk) = event.get("chunk").and_then(serde_json::Value::as_str) {
                            std::io::stdout().write_all(chunk.as_bytes())
                                .and_then(|()| std::io::stdout().flush())
                                .map_err(|error| error.to_string())?;
                        }
                    }
                    Some("end") => return Ok(()),
                    _ => {}
                }
            }
        }
    }
}

pub async fn check_reachability(environment: RuntimeEnvironmentSetting) -> RuntimeReachability {
    match request(
        environment,
        "status.get",
        serde_json::Value::Null,
        Duration::from_secs(15),
    )
    .await
    {
        Ok(result) => {
            let version = result
                .get("version")
                .or_else(|| result.get("appVersion"))
                .and_then(serde_json::Value::as_str);
            RuntimeReachability {
                reachable: true,
                message: version
                    .map(|version| format!("Connected · runtime {version}"))
                    .unwrap_or_else(|| "Connected · E2EE status verified".into()),
            }
        }
        Err(message) => RuntimeReachability {
            reachable: false,
            message,
        },
    }
}

fn updater_state_from_snapshot(
    snapshot: &serde_json::Value,
    current_version: Option<String>,
) -> RemoteServerUpdateState {
    let status = snapshot.get("status").unwrap_or(snapshot);
    let state = status
        .get("state")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("error");
    let target_version = status
        .get("version")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let progress = status.get("percent").and_then(serde_json::Value::as_f64);
    let (phase, message) = match state {
        "available" => (
            RemoteUpdatePhase::Available,
            target_version
                .as_deref()
                .map(|version| format!("Update {version} is available"))
                .unwrap_or_else(|| "A server update is available".into()),
        ),
        "not-available" | "idle" => (RemoteUpdatePhase::Current, "Server is up to date".into()),
        "checking" => (RemoteUpdatePhase::Checking, "Checking for updates…".into()),
        "downloading" => (
            RemoteUpdatePhase::Updating,
            progress.map_or_else(
                || "Downloading server update…".into(),
                |percent| format!("Downloading server update… {percent:.0}%"),
            ),
        ),
        "downloaded" => (
            RemoteUpdatePhase::Updating,
            "Server update downloaded".into(),
        ),
        "error" => (
            RemoteUpdatePhase::Failed,
            status
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Remote server updater failed")
                .to_string(),
        ),
        _ => (
            RemoteUpdatePhase::Failed,
            "Remote server returned an unknown updater state".into(),
        ),
    };
    RemoteServerUpdateState {
        phase,
        current_version,
        target_version,
        progress,
        message,
    }
}

async fn wait_for_updater_state(
    environment: RuntimeEnvironmentSetting,
    accepted: &[&str],
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let snapshot = request(
            environment.clone(),
            "updater.getStatus",
            serde_json::Value::Null,
            Duration::from_secs(15),
        )
        .await?;
        let state = snapshot
            .get("status")
            .and_then(|status| status.get("state"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if state == "error" || accepted.contains(&state) {
            return Ok(snapshot);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("Timed out waiting for the server updater.".into());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

pub async fn inspect_server_update(
    environment: RuntimeEnvironmentSetting,
) -> RemoteServerUpdateState {
    let status = match request(
        environment.clone(),
        "status.get",
        serde_json::Value::Null,
        Duration::from_secs(15),
    )
    .await
    {
        Ok(status) => status,
        Err(error) => {
            return RemoteServerUpdateState {
                phase: RemoteUpdatePhase::Failed,
                current_version: None,
                target_version: None,
                progress: None,
                message: error,
            };
        }
    };
    let current_version = status
        .get("appVersion")
        .or_else(|| status.get("version"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let supports_update = status
        .get("capabilities")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|capabilities| {
            capabilities
                .iter()
                .any(|capability| capability.as_str() == Some("updater.remote-control.v1"))
        });
    let automatic = status
        .get("remoteUpdateSupport")
        .and_then(|support| support.get("automatic"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !supports_update || !automatic {
        return RemoteServerUpdateState {
            phase: RemoteUpdatePhase::Manual,
            current_version,
            target_version: None,
            progress: None,
            message: "Update this server manually once to enable remote updates.".into(),
        };
    }

    let first = match request(
        environment.clone(),
        "updater.check",
        serde_json::json!({
            "includePrerelease": false,
            "includePerfPrerelease": false
        }),
        Duration::from_secs(30),
    )
    .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return RemoteServerUpdateState {
                phase: RemoteUpdatePhase::Failed,
                current_version,
                target_version: None,
                progress: None,
                message: error,
            };
        }
    };
    let first_state = first
        .get("status")
        .and_then(|status| status.get("state"))
        .and_then(serde_json::Value::as_str);
    let snapshot = if matches!(first_state, Some("available" | "not-available" | "error")) {
        Ok(first)
    } else {
        wait_for_updater_state(
            environment,
            &["available", "not-available"],
            Duration::from_secs(120),
        )
        .await
    };
    match snapshot {
        Ok(snapshot) => updater_state_from_snapshot(&snapshot, current_version),
        Err(error) => RemoteServerUpdateState {
            phase: RemoteUpdatePhase::Failed,
            current_version,
            target_version: None,
            progress: None,
            message: error,
        },
    }
}

pub async fn update_server(
    environment: RuntimeEnvironmentSetting,
    previous: RemoteServerUpdateState,
) -> RemoteServerUpdateState {
    let available = inspect_server_update(environment.clone()).await;
    if available.phase != RemoteUpdatePhase::Available {
        return available;
    }
    let target_version = available.target_version.clone();
    if let Err(error) = request(
        environment.clone(),
        "updater.download",
        serde_json::Value::Null,
        Duration::from_secs(30),
    )
    .await
    {
        return RemoteServerUpdateState {
            phase: RemoteUpdatePhase::Failed,
            message: error,
            ..available
        };
    }
    let downloaded = wait_for_updater_state(
        environment.clone(),
        &["downloaded"],
        Duration::from_secs(10 * 60),
    )
    .await;
    if let Err(error) = downloaded {
        return RemoteServerUpdateState {
            phase: RemoteUpdatePhase::Failed,
            message: error,
            ..available
        };
    }
    let install = match request(
        environment.clone(),
        "updater.install",
        serde_json::Value::Null,
        Duration::from_secs(30),
    )
    .await
    {
        Ok(install) => install,
        Err(error) => {
            return RemoteServerUpdateState {
                phase: RemoteUpdatePhase::Failed,
                message: error,
                ..available
            };
        }
    };
    let old_runtime_id = install
        .get("runtimeId")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let target_version = install
        .get("targetVersion")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or(target_version);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3 * 60);
    while tokio::time::Instant::now() < deadline {
        if let Ok(status) = request(
            environment.clone(),
            "status.get",
            serde_json::Value::Null,
            Duration::from_secs(10),
        )
        .await
        {
            let runtime_id = status.get("runtimeId").and_then(serde_json::Value::as_str);
            let version = status
                .get("appVersion")
                .or_else(|| status.get("version"))
                .and_then(serde_json::Value::as_str);
            if runtime_id.is_some()
                && old_runtime_id.as_deref() != runtime_id
                && target_version.as_deref() == version
            {
                return RemoteServerUpdateState {
                    phase: RemoteUpdatePhase::Updated,
                    current_version: version.map(str::to_string),
                    target_version,
                    progress: Some(100.0),
                    message: "Server updated and reconnected".into(),
                };
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    RemoteServerUpdateState {
        phase: RemoteUpdatePhase::Failed,
        current_version: previous.current_version,
        target_version,
        progress: None,
        message: "The server did not reconnect on the updated version.".into(),
    }
}

fn parse_remote_account(value: &serde_json::Value) -> Option<ManagedProviderAccountSetting> {
    Some(ManagedProviderAccountSetting {
        id: value.get("id")?.as_str()?.to_string(),
        email: value
            .get("email")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Provider account")
            .to_string(),
        // Remote summaries intentionally do not expose credential paths.
        config_dir: String::new(),
        created_at_unix_ms: value
            .get("createdAt")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        updated_at_unix_ms: value
            .get("updatedAt")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        last_authenticated_at_unix_ms: value
            .get("lastAuthenticatedAt")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    })
}

fn parse_provider_accounts(value: &serde_json::Value) -> Result<RemoteProviderAccounts, String> {
    let provider = |name: &str| {
        value
            .get(name)
            .ok_or_else(|| format!("Remote account snapshot is missing {name}."))
    };
    let claude = provider("claude")?;
    let codex = provider("codex")?;
    let parse_accounts = |value: &serde_json::Value| {
        value
            .get("accounts")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "Remote account snapshot has invalid accounts.".to_string())
            .map(|items| items.iter().filter_map(parse_remote_account).collect())
    };
    let rate_limits = value.get("rateLimits");
    Ok(RemoteProviderAccounts {
        claude: parse_accounts(claude)?,
        active_claude: claude
            .get("activeAccountId")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        codex: parse_accounts(codex)?,
        active_codex: codex
            .get("activeAccountId")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        claude_limits: rate_limits
            .and_then(|limits| limits.get("claude"))
            .filter(|value| !value.is_null())
            .map(|value| {
                crate::rate_limits::ProviderRateLimits::from_runtime_value(
                    crate::rate_limits::RateLimitProvider::Claude,
                    value,
                )
            }),
        codex_limits: rate_limits
            .and_then(|limits| limits.get("codex"))
            .filter(|value| !value.is_null())
            .map(|value| {
                crate::rate_limits::ProviderRateLimits::from_runtime_value(
                    crate::rate_limits::RateLimitProvider::Codex,
                    value,
                )
            }),
    })
}

pub async fn list_provider_accounts(
    environment: RuntimeEnvironmentSetting,
) -> Result<RemoteProviderAccounts, String> {
    let value = request(
        environment,
        "accounts.list",
        serde_json::Value::Null,
        Duration::from_secs(30),
    )
    .await?;
    parse_provider_accounts(&value)
}

pub async fn mutate_provider_account(
    environment: RuntimeEnvironmentSetting,
    provider: crate::managed_accounts::Provider,
    account_id: Option<String>,
    remove: bool,
) -> Result<RemoteProviderAccounts, String> {
    let method = match (provider, remove) {
        (crate::managed_accounts::Provider::Claude, false) => "accounts.selectClaude",
        (crate::managed_accounts::Provider::Codex, false) => "accounts.selectCodex",
        (crate::managed_accounts::Provider::Claude, true) => "accounts.removeClaude",
        (crate::managed_accounts::Provider::Codex, true) => "accounts.removeCodex",
    };
    if remove && account_id.is_none() {
        return Err("A remote account is required for removal.".into());
    }
    request(
        environment.clone(),
        method,
        serde_json::json!({"accountId": account_id}),
        Duration::from_secs(30),
    )
    .await?;
    list_provider_accounts(environment).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(scope: Option<&str>, relay: bool) -> String {
        let mut value = serde_json::json!({
            "v": 2,
            "endpoint": "wss://runtime.example.com",
            "deviceToken": "device-secret",
            "publicKeyB64": base64::engine::general_purpose::STANDARD.encode([7_u8; 32])
        });
        if let Some(scope) = scope {
            value["scope"] = serde_json::Value::String(scope.into());
        }
        if relay {
            value["relay"] = serde_json::json!({"v": 1});
        }
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string())
    }

    #[test]
    fn runtime_pairing_accepts_bare_and_orca_url_forms() {
        let bare = code(Some("runtime"), false);
        assert_eq!(
            parse_pairing_offer(&bare).unwrap().endpoint,
            "wss://runtime.example.com"
        );
        assert_eq!(
            parse_pairing_offer(&format!("orca://pair?code={bare}"))
                .unwrap()
                .scope
                .as_deref(),
            Some("runtime")
        );
    }

    #[test]
    fn mobile_and_relay_pairing_are_rejected() {
        assert!(parse_pairing_offer(&code(Some("mobile"), false)).is_err());
        assert!(parse_pairing_offer(&code(None, true)).is_err());
    }

    #[test]
    fn provider_account_snapshot_keeps_remote_roster_and_usage_without_paths() {
        let snapshot = parse_provider_accounts(&serde_json::json!({
            "claude": {
                "accounts": [{
                    "id": "claude-one",
                    "email": "claude@example.com",
                    "createdAt": 1,
                    "updatedAt": 2,
                    "lastAuthenticatedAt": 3
                }],
                "activeAccountId": "claude-one"
            },
            "codex": {
                "accounts": [{
                    "id": "codex-one",
                    "email": "codex@example.com",
                    "createdAt": 4,
                    "updatedAt": 5,
                    "lastAuthenticatedAt": 6
                }],
                "activeAccountId": null
            },
            "rateLimits": {
                "claude": {
                    "session": {"usedPercent": 27, "resetsAt": 2_000_000_000_000_u64},
                    "weekly": null,
                    "updatedAt": 7,
                    "error": null,
                    "status": "ok"
                },
                "codex": null
            }
        }))
        .unwrap();

        assert_eq!(snapshot.claude[0].email, "claude@example.com");
        assert!(snapshot.claude[0].config_dir.is_empty());
        assert_eq!(snapshot.active_claude.as_deref(), Some("claude-one"));
        assert_eq!(snapshot.claude_limits.unwrap().buckets[0].used_percent, 27);
        assert_eq!(snapshot.codex[0].id, "codex-one");
        assert_eq!(snapshot.active_codex, None);
        assert_eq!(snapshot.codex_limits, None);
    }

    #[test]
    fn updater_snapshot_maps_available_and_download_progress() {
        let available = updater_state_from_snapshot(
            &serde_json::json!({
                "status": {"state": "available", "version": "1.4.162"}
            }),
            Some("1.4.160".into()),
        );
        assert_eq!(available.phase, RemoteUpdatePhase::Available);
        assert_eq!(available.target_version.as_deref(), Some("1.4.162"));

        let downloading = updater_state_from_snapshot(
            &serde_json::json!({
                "status": {"state": "downloading", "version": "1.4.162", "percent": 42.5}
            }),
            Some("1.4.160".into()),
        );
        assert_eq!(downloading.phase, RemoteUpdatePhase::Updating);
        assert_eq!(downloading.progress, Some(42.5));
    }
}
