//! Foreground Orca-compatible runtime server.
//!
//! The server exposes the running desktop authority over the same legacy E2EE
//! WebSocket handshake used by Orca runtime clients. Mobile-scoped offers and
//! hosted relay transport are intentionally outside Suaegi's supported scope.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use crypto_box::aead::{AeadCore, AeadInPlace, OsRng};
use crypto_box::{PublicKey, SalsaBox, SecretKey};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

const MAX_FRAME_BYTES: usize = 1024 * 1024;
static PROJECT_ROOT: OnceLock<PathBuf> = OnceLock::new();
static WORKTREE_CREATE_LOCKS: OnceLock<
    tokio::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
> = OnceLock::new();

#[derive(Debug, Clone)]
struct RuntimeRequestContext {
    caller_fingerprint: String,
    orchestration_request_id: Option<String>,
}

pub fn run(args: &[String], json_output: bool) -> Result<i32, String> {
    if args.iter().any(|argument| argument == "--mobile-pairing") {
        return Err(
            "Mobile pairing is intentionally not supported by this Suaegi port; use runtime pairing."
                .into(),
        );
    }
    let no_pairing = args.iter().any(|argument| argument == "--no-pairing");
    let recipe_json = args.iter().any(|argument| argument == "--recipe-json");
    if no_pairing && recipe_json {
        return Err("Recipe JSON output requires runtime pairing; remove --no-pairing.".into());
    }
    let project_root = crate::cli::option_value(args, "--project-root")?;
    if recipe_json && project_root.is_none() {
        return Err("Recipe JSON output requires --project-root.".into());
    }
    if let Some(root) = project_root.as_deref() {
        let path = std::path::Path::new(root);
        if !path.is_absolute() || !path.is_dir() {
            return Err("--project-root must be an existing absolute directory.".into());
        }
    }
    let port = crate::cli::option_value(args, "--port")?
        .map(|value| {
            value
                .parse::<u16>()
                .map_err(|_| "--port must be an integer from 0 to 65535.".to_string())
        })
        .transpose()?
        .unwrap_or(6768);
    let pairing_address = crate::cli::option_value(args, "--pairing-address")?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("Could not start the runtime server: {error}"))?;
    runtime.block_on(serve(
        port,
        pairing_address.as_deref(),
        no_pairing,
        recipe_json,
        project_root.as_deref(),
        json_output,
    ))
}

async fn serve(
    port: u16,
    pairing_address: Option<&str>,
    no_pairing: bool,
    recipe_json: bool,
    project_root: Option<&str>,
    json_output: bool,
) -> Result<i32, String> {
    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port))
        .await
        .map_err(|error| format!("Could not bind the runtime server: {error}"))?;
    if let Some(project_root) = project_root {
        let root = std::fs::canonicalize(project_root)
            .map_err(|error| format!("Could not resolve --project-root: {error}"))?;
        let _ = PROJECT_ROOT.set(root);
    }
    let bound_port = listener
        .local_addr()
        .map_err(|error| format!("Could not inspect the runtime server: {error}"))?
        .port();
    let bound_endpoint = format!("ws://0.0.0.0:{bound_port}");
    let advertised_endpoint = advertised_endpoint(pairing_address, bound_port)?;
    let server_secret = SecretKey::generate(&mut OsRng);
    let device_token = random_token()?;
    let pairing_code = (!no_pairing).then(|| {
        encode_pairing_code(
            &advertised_endpoint,
            &device_token,
            &server_secret.public_key(),
        )
    });

    if recipe_json {
        println!(
            "{}",
            json!({
                "schemaVersion": 1,
                "connection": {
                    "type": "orca-server",
                    "pairingCode": pairing_code,
                    "projectRoot": project_root,
                }
            })
        );
    } else {
        let readiness = json!({
            "runtimeId": format!("suaegi-{}", std::process::id()),
            "boundEndpoint": bound_endpoint,
            "advertisedEndpoint": advertised_endpoint,
            "pairing": pairing_code.as_ref().map(|code| json!({
                "available": true,
                "scope": "runtime",
                "endpoint": advertised_endpoint,
                "code": code,
                "url": format!("orca://pair?code={code}"),
            })).unwrap_or_else(|| json!({
                "available": false,
                "reason": "disabled_by_operator",
                "guidance": "Restart without --no-pairing to create a client pairing offer.",
            })),
        });
        if json_output {
            println!("{readiness}");
        } else {
            println!("Suaegi runtime server: {advertised_endpoint}");
            if let Some(code) = pairing_code.as_deref() {
                println!("Pairing URL: orca://pair?code={code}");
            } else {
                println!("Runtime pairing is disabled.");
            }
            println!("Press Ctrl+C to stop.");
        }
    }

    loop {
        let (stream, _) = listener
            .accept()
            .await
            .map_err(|error| format!("Runtime server accept failed: {error}"))?;
        let secret = server_secret.clone();
        let token = device_token.clone();
        tokio::spawn(async move {
            let _ = handle_connection(stream, secret, token).await;
        });
    }
}

fn advertised_endpoint(value: Option<&str>, port: u16) -> Result<String, String> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(format!("ws://127.0.0.1:{port}"));
    };
    if value.starts_with("ws://") || value.starts_with("wss://") {
        let mut endpoint =
            url::Url::parse(value).map_err(|_| "--pairing-address is not a valid URL.")?;
        if endpoint.port().is_none() {
            endpoint
                .set_port(Some(port))
                .map_err(|_| "--pairing-address cannot use that port.")?;
        }
        return Ok(endpoint.to_string().trim_end_matches('/').to_string());
    }
    let host = value.trim_matches(['[', ']']);
    if host.is_empty() || host.contains('/') {
        return Err("--pairing-address must be a host or ws(s) URL.".into());
    }
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    Ok(format!("ws://{host}:{port}"))
}

fn random_token() -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("Could not generate a pairing token: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn encode_pairing_code(endpoint: &str, token: &str, public_key: &PublicKey) -> String {
    let payload = json!({
        "v": 2,
        "endpoint": endpoint,
        "deviceToken": token,
        "publicKeyB64": base64::engine::general_purpose::STANDARD.encode(public_key.as_bytes()),
        "scope": "runtime",
    });
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string())
}

async fn handle_connection(
    stream: TcpStream,
    server_secret: SecretKey,
    device_token: String,
) -> Result<(), String> {
    let mut socket = tokio_tungstenite::accept_async(stream)
        .await
        .map_err(|_| "Runtime WebSocket handshake failed.".to_string())?;
    let hello = receive_text(&mut socket).await?;
    let hello: Value =
        serde_json::from_str(&hello).map_err(|_| "Invalid E2EE hello frame.".to_string())?;
    if hello.get("type").and_then(Value::as_str) != Some("e2ee_hello") {
        return Err("Invalid E2EE hello frame.".into());
    }
    let client_key = hello
        .get("publicKeyB64")
        .and_then(Value::as_str)
        .ok_or_else(|| "E2EE hello is missing publicKeyB64.".to_string())?;
    let client_key = decode_public_key(client_key)?;
    let cipher = SalsaBox::new(&client_key, &server_secret);
    socket
        .send(Message::Text(
            json!({"type":"e2ee_ready"}).to_string().into(),
        ))
        .await
        .map_err(|_| "Could not send E2EE readiness.".to_string())?;

    let auth = decrypt_text(&cipher, &receive_text(&mut socket).await?)?;
    let auth: Value =
        serde_json::from_str(&auth).map_err(|_| "Invalid E2EE authentication.".to_string())?;
    if auth.get("type").and_then(Value::as_str) != Some("e2ee_auth")
        || auth.get("deviceToken").and_then(Value::as_str) != Some(&device_token)
    {
        let error = encrypt_text(
            &cipher,
            &json!({"type":"e2ee_error","error":{"code":"bad_auth"}}).to_string(),
        )?;
        let _ = socket.send(Message::Text(error.into())).await;
        return Err("Runtime authentication failed.".into());
    }
    let authenticated = encrypt_text(&cipher, &json!({"type":"e2ee_authenticated"}).to_string())?;
    socket
        .send(Message::Text(authenticated.into()))
        .await
        .map_err(|_| "Could not confirm runtime authentication.".to_string())?;

    struct TerminalSubscription {
        request_id: String,
        terminal: String,
        output: String,
    }
    let mut subscription: Option<TerminalSubscription> = None;
    loop {
        let frame = if subscription.is_some() {
            tokio::select! {
                frame = receive_text(&mut socket) => Some(frame?),
                () = tokio::time::sleep(Duration::from_millis(300)) => {
                    let terminal = subscription.as_ref().expect("checked").terminal.clone();
                    let snapshot = tokio::task::spawn_blocking({
                        let terminal = terminal.clone();
                        move || crate::cli::read_daemon_output(&terminal, Some("100000".into()))
                    }).await.map_err(|error| format!("Terminal stream poll failed: {error}"))??;
                    let info = crate::cli::daemon_session_info(&terminal)?;
                    let active = subscription.as_mut().expect("checked");
                    if snapshot != active.output {
                        let event = if snapshot.starts_with(&active.output) {
                            json!({
                                "type": "data",
                                "chunk": &snapshot[active.output.len()..],
                            })
                        } else {
                            json!({"type": "scrollback", "serialized": snapshot})
                        };
                        active.output = snapshot;
                        send_runtime_payload(
                            &mut socket,
                            &cipher,
                            json!({"id": active.request_id, "result": event}),
                        ).await?;
                    }
                    if !info.running {
                        send_runtime_payload(
                            &mut socket,
                            &cipher,
                            json!({
                                "id": active.request_id,
                                "result": {"type": "end", "exitCode": info.exit_code},
                            }),
                        ).await?;
                        subscription = None;
                    }
                    None
                }
            }
        } else {
            Some(receive_text(&mut socket).await?)
        };
        let Some(frame) = frame else {
            continue;
        };
        let request = match decrypt_text(&cipher, &frame).and_then(|plain| {
            serde_json::from_str::<Value>(&plain).map_err(|_| "Invalid RPC JSON.".into())
        }) {
            Ok(request) => request,
            Err(_) => continue,
        };
        if request.get("deviceToken").and_then(Value::as_str) != Some(&device_token) {
            continue;
        }
        let Some(id) = request
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            continue;
        };
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        if method == "terminal.subscribe" {
            let terminal = runtime_string_param(&params, "terminal")?.to_string();
            crate::cli::daemon_session_info(&terminal)?;
            let snapshot = tokio::task::spawn_blocking({
                let terminal = terminal.clone();
                move || crate::cli::read_daemon_output(&terminal, Some("100000".into()))
            })
            .await
            .map_err(|error| format!("Terminal subscription failed: {error}"))??;
            let lines = snapshot.lines().map(str::to_string).collect::<Vec<_>>();
            subscription = Some(TerminalSubscription {
                request_id: id.clone(),
                terminal,
                output: snapshot,
            });
            send_runtime_payload(
                &mut socket,
                &cipher,
                json!({"id": id, "result": {"type": "subscribed", "lines": lines}}),
            )
            .await?;
            continue;
        }
        let context = RuntimeRequestContext {
            caller_fingerprint: token_fingerprint(&device_token),
            orchestration_request_id: request
                .get("orchestrationRequestId")
                .and_then(Value::as_str)
                .map(str::to_string),
        };
        let response = dispatch(method, params, &context).await;
        let payload = match response {
            Ok(result) => json!({"id": id, "result": result}),
            Err(message) => json!({
                "id": id,
                "error": {"code": "runtime_request_failed", "message": message}
            }),
        };
        send_runtime_payload(&mut socket, &cipher, payload).await?;
    }
}

async fn send_runtime_payload(
    socket: &mut tokio_tungstenite::WebSocketStream<TcpStream>,
    cipher: &SalsaBox,
    payload: Value,
) -> Result<(), String> {
    let encrypted = encrypt_text(cipher, &payload.to_string())?;
    socket
        .send(Message::Text(encrypted.into()))
        .await
        .map_err(|_| "Could not send the runtime response.".to_string())
}

async fn dispatch(
    method: String,
    params: Value,
    context: &RuntimeRequestContext,
) -> Result<Value, String> {
    if matches!(method.as_str(), "status" | "status.get") {
        let forwarded =
            tokio::task::spawn_blocking(|| crate::local_rpc::call("status", Value::Null))
                .await
                .map_err(|error| format!("Runtime status dispatch failed: {error}"))??;
        return Ok(runtime_status(forwarded));
    }
    let forwarded = tokio::task::spawn_blocking({
        let method = method.clone();
        let params = params.clone();
        move || crate::local_rpc::call(&method, params)
    })
    .await
    .map_err(|error| format!("Runtime dispatch failed: {error}"))?;
    match forwarded {
        Ok(Some(result)) => return Ok(result),
        Ok(None) => {}
        Err(error) if !direct_runtime_method(&method) => return Err(error),
        Err(_) => {}
    }
    if let Some(result) = dispatch_direct(&method, &params, context).await? {
        return Ok(result);
    }
    Err(format!(
        "Runtime method {method} requires the Suaegi desktop authority to be running."
    ))
}

fn direct_runtime_method(method: &str) -> bool {
    matches!(
        method,
        "repo.list"
            | "worktree.list"
            | "worktree.show"
            | "worktree.create"
            | "worktree.rm"
            | "worktree.remove"
            | "terminal.list"
            | "terminal.show"
            | "terminal.read"
            | "terminal.create"
            | "terminal.send"
            | "terminal.wait"
            | "terminal.close"
            | "files.stat"
            | "files.readDir"
            | "files.readPreview"
            | "files.write"
            | "files.listAll"
            | "files.search"
            | "git.status"
            | "git.stage"
            | "git.unstage"
            | "git.discard"
            | "git.commit"
            | "git.fetch"
            | "git.pull"
            | "git.push"
            | "git.branchCompare"
            | "git.branchDiff"
            | "orchestration.federationAttachStart"
            | "orchestration.federationShow"
            | "orchestration.federationRead"
            | "orchestration.federationReadOutput"
            | "orchestration.federationStop"
            | "orchestration.federationPull"
            | "orchestration.federationAck"
            | "orchestration.federationImport"
    )
}

async fn dispatch_direct(
    method: &str,
    params: &Value,
    context: &RuntimeRequestContext,
) -> Result<Option<Value>, String> {
    if !direct_runtime_method(method) {
        return Ok(None);
    }
    if matches!(method, "repo.list" | "worktree.list" | "worktree.show") {
        return runtime_topology(method, params).map(Some);
    }
    if matches!(
        method,
        "worktree.create" | "worktree.rm" | "worktree.remove"
    ) {
        return runtime_worktree_mutation(method, params, context)
            .await
            .map(Some);
    }
    if method.starts_with("terminal.") {
        if method == "terminal.create" {
            let mut effective = params.clone();
            if let Some(mutation_id) = runtime_mutation_id(params)? {
                let worktree = runtime_worktree_path(params)?;
                let canonical = std::fs::canonicalize(&worktree).unwrap_or(worktree);
                let handle = runtime_terminal_create_handle(
                    &context.caller_fingerprint,
                    &canonical,
                    mutation_id,
                );
                effective
                    .as_object_mut()
                    .ok_or_else(|| "Remote terminal parameters must be an object.".to_string())?
                    .insert("preallocatedHandle".into(), Value::String(handle));
            } else if params
                .get("reconcileExisting")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return Err(
                    "Remote terminal reconciliation requires clientMutationId and worktree.".into(),
                );
            }
            return runtime_terminal(method, &effective).await.map(Some);
        }
        return runtime_terminal(method, params).await.map(Some);
    }
    if method.starts_with("orchestration.federation") {
        return runtime_federation(method, params, context).await.map(Some);
    }
    let worktree = runtime_worktree_path(params)?;
    let result = match method {
        "files.stat" => {
            let relative = runtime_string_param(params, "relativePath")?;
            let signature = suaegi_git::fs::file_signature(&worktree, relative)
                .map_err(|error| format!("Could not stat remote file: {error}"))?;
            Ok(json!({
                "size": signature.size,
                "mtime": system_time_millis(signature.mtime)?,
            }))
        }
        "files.readDir" => {
            let relative = params
                .get("relativePath")
                .and_then(Value::as_str)
                .unwrap_or("");
            let entries = suaegi_git::fs::list_dir(&worktree, relative)
                .map_err(|error| format!("Could not list remote directory: {error}"))?
                .into_iter()
                .map(|entry| {
                    json!({
                        "name": entry.name,
                        "isDirectory": entry.is_dir,
                        "isSymlink": entry.is_symlink,
                    })
                })
                .collect::<Vec<_>>();
            Ok(Value::Array(entries))
        }
        "files.readPreview" => {
            let relative = runtime_string_param(params, "relativePath")?;
            match suaegi_git::fs::read_file(&worktree, relative)
                .map_err(|error| format!("Could not read remote file: {error}"))?
            {
                suaegi_git::fs::FileRead::Ready {
                    content: suaegi_git::fs::FileContent::Text(content),
                    size,
                } => Ok(json!({
                    "content": content,
                    "size": size,
                    "isBinary": false,
                    "tooLarge": false,
                })),
                suaegi_git::fs::FileRead::Ready {
                    content: suaegi_git::fs::FileContent::Binary,
                    size,
                } => {
                    let encoded = match suaegi_git::fs::read_editable_file(&worktree, relative)
                        .map_err(|error| format!("Could not read remote binary file: {error}"))?
                    {
                        suaegi_git::fs::EditableFileRead::Binary { bytes, .. } => {
                            base64::engine::general_purpose::STANDARD.encode(bytes)
                        }
                        _ => String::new(),
                    };
                    Ok(json!({
                        "content": encoded,
                        "size": size,
                        "isBinary": true,
                        "encoding": "base64",
                        "tooLarge": false,
                    }))
                }
                suaegi_git::fs::FileRead::TooLarge { limit } => Ok(json!({
                    "content": "",
                    "size": Value::Null,
                    "isBinary": false,
                    "tooLarge": true,
                    "limit": limit,
                })),
            }
        }
        "files.write" => {
            let relative = runtime_string_param(params, "relativePath")?;
            let content = runtime_string_param(params, "content")?;
            let outcome = suaegi_git::fs::write_file(&worktree, relative, content.as_bytes(), None)
                .map_err(|error| format!("Could not write remote file: {error}"))?;
            match outcome {
                suaegi_git::fs::WriteOutcome::Written { signature } => Ok(json!({
                    "written": true,
                    "size": signature.size,
                    "mtime": system_time_millis(signature.mtime)?,
                })),
                suaegi_git::fs::WriteOutcome::StaleConflict { .. } => {
                    Err("Remote file changed before it could be written.".into())
                }
            }
        }
        "files.listAll" => {
            let files = suaegi_git::quick_open::list_quick_open_files(&worktree, &[])
                .await
                .map_err(|error| format!("Could not list remote files: {error}"))?;
            Ok(json!(files))
        }
        "files.search" => {
            let options = suaegi_search::SearchOptions {
                query: runtime_string_param(params, "query")?.to_string(),
                root_path: worktree.to_string_lossy().into_owned(),
                case_sensitive: params.get("caseSensitive").and_then(Value::as_bool),
                whole_word: params.get("wholeWord").and_then(Value::as_bool),
                use_regex: params.get("useRegex").and_then(Value::as_bool),
                include_pattern: params
                    .get("includePattern")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                exclude_pattern: params
                    .get("excludePattern")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                max_results: params
                    .get("maxResults")
                    .and_then(Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok()),
            };
            let result = suaegi_search::run_search(&options)
                .await
                .map_err(|error| format!("Remote search failed: {error}"))?;
            serde_json::to_value(result)
                .map_err(|error| format!("Could not encode remote search: {error}"))
        }
        "git.status" => runtime_git_status(&worktree).await,
        "git.stage" | "git.unstage" | "git.discard" => {
            let file = runtime_string_param(params, "filePath")?;
            let runner = suaegi_git::runner::GitRunner::new();
            match method {
                "git.stage" => suaegi_git::write_ops::stage(&runner, &worktree, file)
                    .await
                    .map_err(|error| error.to_string())?,
                "git.unstage" => suaegi_git::write_ops::unstage(&runner, &worktree, file)
                    .await
                    .map_err(|error| error.to_string())?,
                _ => {
                    suaegi_git::write_ops::discard(&runner, &worktree, file)
                        .await
                        .map_err(|error| error.to_string())?;
                }
            }
            Ok(json!({"ok": true, "filePath": file}))
        }
        "git.commit" => {
            let message = runtime_string_param(params, "message")?;
            let runner = suaegi_git::runner::GitRunner::new();
            let outcome = suaegi_git::write_ops::commit_changes(&runner, &worktree, message)
                .await
                .map_err(|error| error.to_string())?;
            Ok(json!({"status": format!("{outcome:?}")}))
        }
        "git.fetch" => {
            let runner = suaegi_git::runner::GitRunner::new();
            suaegi_git::remote::fetch(&runner, &worktree)
                .await
                .map_err(|error| error.to_string())?;
            Ok(json!({"ok": true}))
        }
        "git.pull" => {
            let runner = suaegi_git::runner::GitRunner::new();
            let outcome = suaegi_git::remote::pull(&runner, &worktree)
                .await
                .map_err(|error| error.to_string())?;
            Ok(json!({"status": format!("{outcome:?}")}))
        }
        "git.push" => {
            let runner = suaegi_git::runner::GitRunner::new();
            let branch = runner
                .run(&worktree, &["branch", "--show-current"])
                .await
                .map_err(|error| error.to_string())?
                .stdout
                .trim()
                .to_string();
            if branch.is_empty() {
                return Err("Cannot push a detached remote worktree.".into());
            }
            let outcome = suaegi_git::remote::push(&runner, &worktree, &branch, true)
                .await
                .map_err(|error| error.to_string())?;
            Ok(json!({"status": format!("{outcome:?}"), "branch": branch}))
        }
        "git.branchCompare" => {
            let base_ref = runtime_string_param(params, "baseRef")?;
            runtime_branch_compare(&worktree, base_ref).await
        }
        "git.branchDiff" => runtime_branch_diff(&worktree, params).await,
        _ => unreachable!("direct_runtime_method checked the method"),
    }?;
    Ok(Some(result))
}

fn token_fingerprint(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn federation_param<'a>(params: &'a Value, name: &str) -> Result<&'a str, String> {
    params
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("Federation parameter {name} is required."))
}

async fn runtime_federation(
    method: &str,
    params: &Value,
    context: &RuntimeRequestContext,
) -> Result<Value, String> {
    let dispatch_id = federation_param(params, "dispatchId")?;
    if method == "orchestration.federationAttachStart" {
        let request_id = context.orchestration_request_id.as_deref().ok_or_else(|| {
            "Federated worker attachment requires a durable retry request.".to_string()
        })?;
        let task_id = federation_param(params, "taskId")?;
        let task_spec = federation_param(params, "taskSpec")?;
        let worktree_selector = federation_param(params, "worktree")?;
        if matches!(worktree_selector, "current" | "new-child") {
            return Err(
                "A remote worker requires an exact existing worktree or new-top-level.".into(),
            );
        }
        let protocol_version = params
            .get("protocolVersion")
            .and_then(Value::as_u64)
            .unwrap_or(1);
        let runtime_epoch = format!("suaegi-{}", std::process::id());
        let payload_hash = token_fingerprint(&params.to_string());
        if let Some(receipt) = crate::orchestration::begin_federation_attachment(
            dispatch_id,
            task_id,
            &context.caller_fingerprint,
            request_id,
            &payload_hash,
            protocol_version,
            &runtime_epoch,
        )? {
            return Ok(receipt);
        }

        let mut effects = Vec::new();
        let mut residual = Vec::new();
        let start = async {
            let creates_worktree = worktree_selector == "new-top-level";
            let requested_terminal = params.get("terminal").and_then(Value::as_str);
            let agent = params.get("agent").and_then(Value::as_str);
            if creates_worktree {
                if params
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .is_none()
                    || params
                        .get("repo")
                        .and_then(Value::as_str)
                        .filter(|value| !value.trim().is_empty())
                        .is_none()
                {
                    return Err(
                        "A remote new-top-level worktree requires --name and an explicit --repo."
                            .to_string(),
                    );
                }
                if requested_terminal.is_some() {
                    return Err(
                        "--terminal cannot combine with remote new-worktree creation.".into(),
                    );
                }
            } else if ["name", "repo", "baseBranch", "setup", "setupSource"]
                .iter()
                .any(|name| params.get(*name).is_some_and(|value| !value.is_null()))
            {
                return Err(
                    "Creation and setup options apply only to remote new-top-level worktrees."
                        .into(),
                );
            }
            if requested_terminal.is_some() && agent.is_some() {
                return Err("--terminal cannot combine with --agent.".into());
            }
            if requested_terminal.is_none() {
                let agent = agent.ok_or_else(|| {
                    "A configured --agent is required when a federated worker creates a terminal."
                        .to_string()
                })?;
                if suaegi_term::agent::agent_def_by_id(agent).is_none() {
                    return Err(format!("Unknown remote agent: {agent}"));
                }
            }

            let (worktree_id, mut terminal) = if creates_worktree {
                let created = runtime_worktree_mutation(
                    "worktree.create",
                    &json!({
                        "name": params.get("name"),
                        "repo": params.get("repo"),
                        "baseBranch": params.get("baseBranch"),
                        "displayName": params.get("displayName"),
                        "comment": params.get("comment"),
                        "setup": params.get("setup").and_then(Value::as_str).unwrap_or("run"),
                        "agent": agent,
                        "noParent": true,
                    }),
                    context,
                )
                .await?;
                let worktree_id = created["worktree"]["id"]
                    .as_str()
                    .ok_or_else(|| "Federated worktree creation returned no id.".to_string())?
                    .to_string();
                effects
                    .push(json!({"kind":"worktree","action":"created_top_level","id":worktree_id}));
                residual.push(json!({"kind":"worktree","id":worktree_id}));
                if let Some(setup_terminal) =
                    created["setupTerminalHandle"].as_str().map(str::to_string)
                {
                    effects.push(json!({
                        "kind":"terminal","role":"setup","action":"created","id":setup_terminal
                    }));
                }
                let terminal = created["agentTerminalHandle"].as_str().map(str::to_string);
                (worktree_id, terminal)
            } else {
                let shown =
                    runtime_topology("worktree.show", &json!({"worktree": worktree_selector}))?;
                let worktree_id = shown["worktree"]["id"]
                    .as_str()
                    .ok_or_else(|| "Federated worktree lookup returned no id.".to_string())?
                    .to_string();
                effects.push(json!({"kind":"worktree","action":"reused","id":worktree_id}));
                (worktree_id, requested_terminal.map(str::to_string))
            };

            if terminal.is_none() {
                let created = runtime_terminal(
                    "terminal.create",
                    &json!({
                        "worktree": &worktree_id,
                        "agent": agent,
                    }),
                )
                .await?;
                terminal = created["terminal"]["handle"].as_str().map(str::to_string);
            } else if let Some(handle) = terminal.as_deref() {
                let info = runtime_terminal("terminal.show", &json!({"terminal": handle})).await?;
                if info["terminal"]["running"].as_bool() != Some(true) {
                    return Err(format!("Terminal {handle} is not running."));
                }
            }
            let terminal =
                terminal.ok_or_else(|| "Federated worker terminal was not created.".to_string())?;
            if !effects.iter().any(|effect| {
                effect["kind"].as_str() == Some("terminal")
                    && effect["id"].as_str() == Some(&terminal)
            }) {
                effects.push(json!({
                    "kind":"terminal",
                    "role":"agent",
                    "action": if requested_terminal.is_some() {"reused"} else {"created"},
                    "id":terminal,
                }));
            }
            let timeout_ms = params
                .get("timeoutMs")
                .and_then(Value::as_u64)
                .unwrap_or(60_000)
                .clamp(1, 3_600_000);
            let ready = runtime_terminal(
                "terminal.wait",
                &json!({"terminal": &terminal, "for":"tui-idle", "timeoutMs":timeout_ms}),
            )
            .await?;
            if ready["wait"]["satisfied"].as_bool() != Some(true) {
                return Err(format!("Agent terminal {terminal} did not become ready."));
            }
            let preamble = format!(
                "ORCHESTRATION DISPATCH\n\
                 task: {task_id}\n\
                 dispatch: {dispatch_id}\n\
                 coordinator: Run home (relayed by Suaegi)\n\
                 worker: {terminal}\n\n\
                 TASK\n{task_spec}\n\n\
                 Check coordinator control mail with:\n\
                 suaegi orchestration check --terminal {terminal} --wait --json\n\n\
                 When done, report the outcome to the Run home with dispatch {dispatch_id}."
            );
            runtime_terminal(
                "terminal.send",
                &json!({"terminal": &terminal, "text": format!("{preamble}\n")}),
            )
            .await?;
            effects.push(json!({
                "kind":"dispatch_input","role":"agent","id":terminal,"state":"accepted"
            }));
            Ok::<_, String>((worktree_id, terminal))
        }
        .await;

        return match start {
            Ok((worktree_id, terminal)) => crate::orchestration::update_federation_attachment(
                dispatch_id,
                Some(worktree_id),
                Some(terminal),
                "ready",
                effects,
                Vec::new(),
                None,
            ),
            Err(error) => crate::orchestration::update_federation_attachment(
                dispatch_id,
                None,
                None,
                "failed",
                effects,
                residual,
                Some(error),
            ),
        };
    }

    let attachment =
        crate::orchestration::federation_attachment(dispatch_id, &context.caller_fingerprint)?;
    if method == "orchestration.federationPull" {
        let items = crate::orchestration::pull_federation_relay(
            dispatch_id,
            &context.caller_fingerprint,
            params
                .get("afterSequence")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            params
                .get("limit")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(50),
        )?;
        return Ok(json!({
            "dispatchId":dispatch_id,
            "runtimeEpoch":attachment.runtime_epoch,
            "items":items,
        }));
    }
    if method == "orchestration.federationAck" {
        let through = params
            .get("throughSequence")
            .and_then(Value::as_u64)
            .ok_or_else(|| "Federation parameter throughSequence is required.".to_string())?;
        crate::orchestration::acknowledge_federation_relay(
            dispatch_id,
            &context.caller_fingerprint,
            through,
        )?;
        return Ok(json!({"dispatchId":dispatch_id,"acknowledgedThrough":through}));
    }
    if method == "orchestration.federationImport" {
        if context.orchestration_request_id.is_none() {
            return Err("Federation import requires a durable retry request.".into());
        }
        let items = params
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| "Federation parameter items is required.".to_string())?;
        return crate::orchestration::import_federation_relay(
            dispatch_id,
            &context.caller_fingerprint,
            items,
        );
    }
    let terminal = attachment
        .terminal
        .as_deref()
        .ok_or_else(|| format!("Remote Dispatch {dispatch_id} has no terminal."))?;
    match method {
        "orchestration.federationShow" => {
            let shown = runtime_terminal("terminal.show", &json!({"terminal": terminal}))
                .await
                .ok();
            Ok(json!({
                "dispatchId": dispatch_id,
                "runtimeEpoch": attachment.runtime_epoch,
                "attachment": attachment,
                "terminal": shown.and_then(|value| value.get("terminal").cloned()),
                "observation": {
                    "status": if crate::cli::daemon_session_info(terminal).is_ok() {"running"} else {"missing"},
                    "exactWorker": crate::cli::daemon_session_info(terminal).is_ok(),
                }
            }))
        }
        "orchestration.federationRead" | "orchestration.federationReadOutput" => {
            let read = runtime_terminal(
                "terminal.read",
                &json!({
                    "terminal": terminal,
                    "cursor": params.get("cursor"),
                    "limit": params.get("limit"),
                }),
            )
            .await?;
            if method.ends_with("ReadOutput") {
                Ok(json!({
                    "dispatchId": dispatch_id,
                    "runtimeEpoch": attachment.runtime_epoch,
                    "output": {
                        "source":"terminal",
                        "text":read["terminal"]["output"],
                        "nextCursor":read["terminal"]["nextCursor"],
                        "terminalStatus": if read["terminal"]["running"].as_bool() == Some(true) {"running"} else {"exited"},
                    }
                }))
            } else {
                Ok(json!({
                    "dispatchId": dispatch_id,
                    "runtimeEpoch": attachment.runtime_epoch,
                    "terminal": read["terminal"],
                }))
            }
        }
        "orchestration.federationStop" => {
            if matches!(
                attachment.status.as_str(),
                "stopped" | "succeeded" | "failed" | "abandoned"
            ) {
                return Ok(json!({
                    "dispatchId":dispatch_id,
                    "state":attachment.status,
                    "alreadySettled":true,
                    "processAction":"none",
                }));
            }
            let close = runtime_terminal("terminal.close", &json!({"terminal": terminal})).await?;
            let _ = crate::orchestration::update_federation_attachment(
                dispatch_id,
                attachment.worktree_id,
                Some(terminal.to_string()),
                "stopped",
                attachment.effects,
                attachment.residual_resources,
                None,
            )?;
            Ok(json!({
                "dispatchId":dispatch_id,
                "state":"stopped",
                "alreadySettled":false,
                "processAction":"closed_agent_terminal",
                "close":close,
            }))
        }
        _ => Err(format!("Unsupported federation method: {method}")),
    }
}

async fn runtime_worktree_mutation(
    method: &str,
    params: &Value,
    context: &RuntimeRequestContext,
) -> Result<Value, String> {
    runtime_worktree_mutation_at(
        method,
        params,
        context,
        crate::persistence_thread::default_data_file(),
        PROJECT_ROOT.get().map(PathBuf::as_path),
    )
    .await
}

async fn runtime_worktree_mutation_at(
    method: &str,
    params: &Value,
    context: &RuntimeRequestContext,
    data_file: PathBuf,
    project_root: Option<&std::path::Path>,
) -> Result<Value, String> {
    let create_key = if method == "worktree.create" {
        runtime_mutation_id(params)?.map(|mutation_id| {
            let repo = params
                .get("repo")
                .and_then(Value::as_str)
                .unwrap_or("single-repository");
            runtime_worktree_create_key(&context.caller_fingerprint, repo, mutation_id)
        })
    } else {
        None
    };
    let create_lock = if let Some(key) = create_key.as_ref() {
        let locks = WORKTREE_CREATE_LOCKS
            .get_or_init(|| tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let mut locks = locks.lock().await;
        Some(
            locks
                .entry(key.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone(),
        )
    } else {
        None
    };
    let _create_guard = match create_lock.as_ref() {
        Some(lock) => Some(lock.lock().await),
        None => None,
    };
    let mut store = suaegi_core::persistence::Store::new(data_file);
    let mut state = store.load().state;
    if method == "worktree.create" {
        if let Some(key) = create_key.as_ref() {
            if let Some(receipt) = state
                .settings
                .ui
                .runtime_worktree_create_receipts
                .get(key)
                .and_then(|receipt| serde_json::from_str::<Value>(receipt).ok())
            {
                let path = receipt
                    .get("path")
                    .and_then(Value::as_str)
                    .map(std::path::Path::new);
                if path.is_some_and(std::path::Path::exists) {
                    let mut replay = receipt;
                    if let Some(object) = replay.as_object_mut() {
                        object.insert("idempotentReplay".into(), Value::Bool(true));
                    }
                    return Ok(replay);
                }
                state
                    .settings
                    .ui
                    .runtime_worktree_create_receipts
                    .remove(key);
            }
        }
        let name = runtime_string_param(params, "name")?.trim().to_string();
        if name.is_empty() {
            return Err("Remote worktree name cannot be empty.".into());
        }
        if let Some(root) = project_root {
            let repo = suaegi_core::domain::Repo::from_path(root)
                .map_err(|error| format!("Could not resolve --project-root: {error}"))?;
            if !state.repos.iter().any(|candidate| candidate.id == repo.id) {
                state.repos.push(repo);
            }
        }
        let repo_selector = params.get("repo").and_then(Value::as_str);
        let repo = match repo_selector {
            Some(selector) => {
                let selector = selector
                    .strip_prefix("id:")
                    .or_else(|| selector.strip_prefix("path:"))
                    .or_else(|| selector.strip_prefix("name:"))
                    .unwrap_or(selector);
                let canonical_selector = std::path::Path::new(selector).canonicalize().ok();
                state
                    .repos
                    .iter()
                    .find(|repo| {
                        repo.id.0 == selector
                            || repo.display_name == selector
                            || repo.path == std::path::Path::new(selector)
                            || canonical_selector.as_ref() == Some(&repo.path)
                    })
                    .cloned()
                    .ok_or_else(|| format!("Remote repository not found: {selector}"))?
            }
            None if state.repos.len() == 1 => state.repos[0].clone(),
            None => return Err("Remote worktree create requires an explicit repo.".into()),
        };
        let base = params
            .get("baseBranch")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| repo.worktree_base_ref.clone())
            .unwrap_or_else(|| "HEAD".into());
        let configured_root = state
            .settings
            .ui
            .repo_worktree_base_paths
            .get(&repo.id.0)
            .map(String::as_str)
            .filter(|value| !value.trim().is_empty());
        let workspace_root = configured_root.map_or_else(
            || state.settings.workspace_root.clone(),
            |value| {
                let path = std::path::PathBuf::from(value);
                if path.is_absolute() {
                    path
                } else {
                    repo.path.join(path)
                }
            },
        );
        let nest = configured_root
            .map(|_| false)
            .unwrap_or(state.settings.ui.nest_workspaces);
        let created = crate::git_tasks::create_worktree_with_layout_now(
            repo.clone(),
            name,
            base,
            workspace_root,
            nest,
            None,
            state.settings.ui.refresh_local_base_ref,
        )
        .await?;
        let persisted = suaegi_core::domain::Worktree {
            id: suaegi_core::domain::WorktreeId(created.path.to_string_lossy().into_owned()),
            repo_id: repo.id.clone(),
            path: created.path.clone(),
            branch: created.branch.clone(),
            display_name: params
                .get("displayName")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(|value| value.chars().take(100).collect())
                .unwrap_or_else(|| created.display_name.clone()),
            created_with_agent: params
                .get("agent")
                .and_then(Value::as_str)
                .map(str::to_string),
            created_at_unix_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            linked_github_pr: None,
            linked_linear_issue: None,
            linked_linear_issue_workspace_id: None,
            linked_linear_issue_organization_url_key: None,
            linked_jira_issue: None,
            linked_jira_site: None,
        };
        state
            .worktrees
            .retain(|worktree| worktree.id != persisted.id);
        state.worktrees.push(persisted.clone());
        if let Some(comment) = params
            .get("comment")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            state.settings.ui.worktree_comments.insert(
                persisted.id.0.clone(),
                comment.chars().take(1_000).collect(),
            );
        }
        if params.get("noParent").and_then(Value::as_bool) == Some(true) {
            state.settings.ui.worktree_parents.remove(&persisted.id.0);
        } else if let Some(parent) = params
            .get("parentWorktree")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            state
                .settings
                .ui
                .worktree_parents
                .insert(persisted.id.0.clone(), parent.to_string());
        }
        store
            .save(&state)
            .map_err(|error| format!("Could not persist remote worktree: {error}"))?;

        let setup_requested = params
            .get("setup")
            .and_then(Value::as_str)
            .map(|value| value == "run")
            .or_else(|| params.get("runSetup").and_then(Value::as_bool))
            .unwrap_or(false);
        let setup_terminal = if setup_requested {
            let setting = state
                .settings
                .ui
                .repo_hook_settings
                .get(&repo.id.0)
                .cloned()
                .unwrap_or_default();
            match crate::repo_hooks::effective_setup_script(&setting, &persisted.path)? {
                Some(script) => runtime_terminal(
                    "terminal.create",
                    &json!({"worktree": &persisted.path, "command": script}),
                )
                .await?
                .pointer("/terminal/handle")
                .cloned(),
                None => None,
            }
        } else {
            None
        };
        let agent_terminal = if let Some(agent) = params.get("agent").and_then(Value::as_str) {
            runtime_terminal(
                "terminal.create",
                &json!({
                    "worktree": &persisted.path,
                    "agent": agent,
                    "viewport": params.get("viewport"),
                }),
            )
            .await?
            .pointer("/terminal/handle")
            .cloned()
        } else {
            None
        };
        let worktree = json!({
            "id": persisted.id.0,
            "fullWorktreeId": persisted.id.0,
            "repoId": persisted.repo_id.0,
            "path": persisted.path,
            "branch": persisted.branch,
            "displayName": persisted.display_name,
        });
        let response = json!({
            "worktree": worktree,
            "path": worktree["path"],
            "branch": worktree["branch"],
            "agentTerminalHandle": agent_terminal,
            "setupTerminalHandle": setup_terminal,
        });
        if let Some(key) = create_key {
            let receipts = &mut state.settings.ui.runtime_worktree_create_receipts;
            receipts.insert(
                key,
                serde_json::to_string(&response)
                    .map_err(|error| format!("Could not encode worktree receipt: {error}"))?,
            );
            if receipts.len() > 4_096 {
                receipts.retain(|_, receipt| {
                    serde_json::from_str::<Value>(receipt)
                        .ok()
                        .and_then(|value| value.get("path")?.as_str().map(std::path::PathBuf::from))
                        .is_some_and(|path| path.exists())
                });
            }
            store
                .save(&state)
                .map_err(|error| format!("Could not persist worktree receipt: {error}"))?;
        }
        return Ok(response);
    }

    let path = resolve_runtime_worktree_path(params, &state, project_root)?;
    let persisted = state
        .worktrees
        .iter()
        .find(|worktree| worktree.path == path)
        .cloned()
        .ok_or_else(|| format!("Remote worktree not found: {}", path.display()))?;
    let repo = state
        .repos
        .iter()
        .find(|repo| repo.id == persisted.repo_id)
        .cloned()
        .ok_or_else(|| "Remote worktree repository is missing.".to_string())?;
    if repo.path == persisted.path {
        return Err("The primary repository checkout cannot be removed.".into());
    }
    crate::git_tasks::remove_worktree_now(
        repo,
        persisted.path.clone(),
        params
            .get("force")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        Some(persisted.branch.clone()),
        state
            .settings
            .ui
            .repo_symlink_paths
            .get(&persisted.repo_id.0)
            .cloned()
            .unwrap_or_default(),
    )
    .await?;
    state
        .worktrees
        .retain(|worktree| worktree.id != persisted.id);
    state.settings.ui.worktree_comments.remove(&persisted.id.0);
    state.settings.ui.worktree_parents.remove(&persisted.id.0);
    state
        .settings
        .ui
        .runtime_worktree_create_receipts
        .retain(|_, receipt| {
            serde_json::from_str::<Value>(receipt)
                .ok()
                .and_then(|value| value.get("path")?.as_str().map(std::path::PathBuf::from))
                .is_none_or(|path| path != persisted.path)
        });
    store
        .save(&state)
        .map_err(|error| format!("Could not persist remote worktree removal: {error}"))?;
    Ok(json!({"removed": persisted.path, "branch": persisted.branch}))
}

async fn runtime_terminal(method: &str, params: &Value) -> Result<Value, String> {
    if method == "terminal.list" {
        let terminals = suaegi_term::daemon::list_sessions()
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|session| {
                json!({
                    "handle": session.session_id,
                    "running": session.running,
                    "exitCode": session.exit_code,
                    "rows": session.rows,
                    "cols": session.cols,
                    "nextCursor": session.next_sequence,
                })
            })
            .collect::<Vec<_>>();
        return Ok(json!({"terminals": terminals}));
    }
    if method == "terminal.create" {
        let worktree = runtime_worktree_path(params)?;
        let agent = params
            .get("agent")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty());
        let command = params
            .get("command")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty());
        if agent.is_some() && command.is_some() {
            return Err("Remote terminal create cannot combine agent and command.".into());
        }
        let viewport = params.get("viewport").unwrap_or(&Value::Null);
        let rows = viewport
            .get("rows")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(50)
            .clamp(2, 500);
        let cols = viewport
            .get("cols")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(80)
            .clamp(2, 500);
        let persisted =
            suaegi_core::persistence::Store::new(crate::persistence_thread::default_data_file())
                .load()
                .state;
        let (program, args, profile_env) = if let Some(agent) = agent {
            if persisted
                .settings
                .ui
                .disabled_agents
                .iter()
                .any(|disabled| disabled == agent)
            {
                return Err(format!("Agent {agent} is disabled on this server."));
            }
            let definition = suaegi_term::agent::agent_def_by_id(agent)
                .ok_or_else(|| format!("Unknown remote agent: {agent}"))?;
            let command_override = persisted
                .settings
                .ui
                .agent_command_overrides
                .get(agent)
                .map(String::as_str)
                .map(suaegi_gen_prompt::tokenize_custom_command_template)
                .transpose()?;
            let configured_args = persisted
                .settings
                .ui
                .agent_default_args
                .get(agent)
                .map(String::as_str)
                .map(suaegi_gen_prompt::tokenize_custom_command_template)
                .transpose()?
                .unwrap_or_default();
            let spawn = suaegi_term::agent::build_spawn_by_id_with_profile(
                Some(definition.id),
                None,
                worktree.clone(),
                rows,
                cols,
                command_override.as_deref(),
                &configured_args,
            );
            let profile_env = persisted
                .settings
                .ui
                .agent_default_env
                .get(agent)
                .into_iter()
                .flat_map(|values| values.iter())
                .filter(|(name, value)| {
                    !name.starts_with("SUAEGI_")
                        && !name.starts_with("ORCA_")
                        && !name.contains('\0')
                        && !value.contains('\0')
                        && value.len() <= 16 * 1024
                })
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect::<Vec<_>>();
            (spawn.program, spawn.args, profile_env)
        } else {
            let (program, args) = command.map_or_else(
                || ("/bin/zsh".to_string(), vec!["-l".to_string()]),
                |command| {
                    (
                        "/bin/zsh".to_string(),
                        vec!["-lc".to_string(), command.to_string()],
                    )
                },
            );
            (program, args, Vec::new())
        };
        let handle = params
            .get("preallocatedHandle")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                format!(
                    "runtime:{}:{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                )
            });
        let mut env = std::env::vars().collect::<std::collections::BTreeMap<_, _>>();
        if let Some(requested) = params.get("env").and_then(Value::as_object) {
            for (name, value) in requested {
                if name.starts_with("SUAEGI_") || name.starts_with("ORCA_") || name.contains('\0') {
                    continue;
                }
                if let Some(value) = value.as_str().filter(|value| !value.contains('\0')) {
                    env.insert(name.clone(), value.to_string());
                }
            }
        }
        for (name, value) in profile_env {
            env.insert(name, value);
        }
        env.insert("SUAEGI_TERMINAL_HANDLE".into(), handle.clone());
        let spec = suaegi_term::daemon::SpawnSpec {
            program,
            args,
            cwd: Some(worktree.clone()),
            env: env.into_iter().collect(),
            rows,
            cols,
        };
        let (session, reader, _) =
            suaegi_term::daemon::DaemonClientSession::create_or_attach(handle.clone(), spec)
                .map_err(|error| error.to_string())?;
        drop(reader);
        session.disconnect();
        return Ok(json!({
            "terminal": {
                "handle": handle,
                "running": true,
                "worktreeId": worktree,
                "rows": rows,
                "cols": cols,
            }
        }));
    }
    let terminal = runtime_string_param(params, "terminal")?;
    if method == "terminal.close" {
        suaegi_term::daemon::kill_session(terminal).map_err(|error| error.to_string())?;
        return Ok(json!({"terminal": terminal, "closed": true}));
    }
    if method == "terminal.show" {
        let session = crate::cli::daemon_session_info(terminal)?;
        return Ok(json!({
            "terminal": {
                "handle": session.session_id,
                "running": session.running,
                "exitCode": session.exit_code,
                "rows": session.rows,
                "cols": session.cols,
                "nextCursor": session.next_sequence,
            }
        }));
    }
    if method == "terminal.read" {
        let session = crate::cli::daemon_session_info(terminal)?;
        let limit = params
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(1_000)
            .clamp(1, 100_000);
        let output = crate::cli::read_daemon_output(terminal, Some(limit.to_string()))?;
        return Ok(json!({
            "terminal": {
                "handle": terminal,
                "output": output,
                "nextCursor": session.next_sequence,
                "running": session.running,
                "exitCode": session.exit_code,
            }
        }));
    }
    if method == "terminal.send" {
        let (session, reader, is_new) = crate::cli::attach_daemon_session(terminal)?;
        drop(reader);
        if is_new {
            let _ = session.kill();
            return Err(format!("Remote terminal not found: {terminal}"));
        }
        if let Some(viewport) = params.get("viewport").filter(|value| !value.is_null()) {
            let rows = viewport
                .get("rows")
                .and_then(Value::as_u64)
                .and_then(|value| u16::try_from(value).ok());
            let cols = viewport
                .get("cols")
                .and_then(Value::as_u64)
                .and_then(|value| u16::try_from(value).ok());
            if let (Some(rows), Some(cols)) = (rows, cols) {
                session
                    .resize(rows.clamp(2, 500), cols.clamp(2, 500))
                    .map_err(|error| error.to_string())?;
            }
        }
        let text = params.get("text").and_then(Value::as_str).unwrap_or("");
        if !text.is_empty() {
            session
                .write(text.as_bytes())
                .map_err(|error| error.to_string())?;
        }
        session.disconnect();
        return Ok(json!({"terminal": terminal, "bytesWritten": text.len()}));
    }
    if method == "terminal.wait" {
        let condition = params.get("for").and_then(Value::as_str).unwrap_or("exit");
        if !matches!(condition, "exit" | "tui-idle") {
            return Err("Remote terminal wait condition must be exit or tui-idle.".into());
        }
        let timeout_ms = params
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .unwrap_or(300_000)
            .clamp(1, 3_600_000);
        let started = tokio::time::Instant::now();
        let mut previous_sequence = None;
        let mut stable_since = tokio::time::Instant::now();
        loop {
            let info = crate::cli::daemon_session_info(terminal)?;
            let satisfied = if condition == "exit" {
                !info.running
            } else if !info.running {
                true
            } else {
                if previous_sequence != Some(info.next_sequence) {
                    previous_sequence = Some(info.next_sequence);
                    stable_since = tokio::time::Instant::now();
                }
                stable_since.elapsed() >= Duration::from_millis(750)
            };
            if satisfied {
                return Ok(json!({
                    "wait": {
                        "terminal": terminal,
                        "for": condition,
                        "satisfied": true,
                        "exitCode": info.exit_code,
                    }
                }));
            }
            if started.elapsed() >= Duration::from_millis(timeout_ms) {
                return Ok(json!({
                    "wait": {
                        "terminal": terminal,
                        "for": condition,
                        "satisfied": false,
                        "timeoutMs": timeout_ms,
                    }
                }));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    Err(format!("Unsupported runtime terminal method: {method}"))
}

fn runtime_topology(method: &str, params: &Value) -> Result<Value, String> {
    let state =
        suaegi_core::persistence::Store::new(crate::persistence_thread::default_data_file())
            .load()
            .state;
    if method == "repo.list" {
        let mut repos = state
            .repos
            .iter()
            .map(|repo| {
                json!({
                    "id": repo.id.0,
                    "name": repo.display_name,
                    "displayName": repo.display_name,
                    "path": repo.path,
                    "baseRef": repo.worktree_base_ref,
                })
            })
            .collect::<Vec<_>>();
        if let Some(root) = PROJECT_ROOT.get() {
            if !state.repos.iter().any(|repo| repo.path == *root) {
                repos.push(json!({
                    "id": root.to_string_lossy(),
                    "name": root.file_name().and_then(|name| name.to_str()).unwrap_or("project"),
                    "displayName": root.file_name().and_then(|name| name.to_str()).unwrap_or("project"),
                    "path": root,
                    "baseRef": Value::Null,
                }));
            }
        }
        return Ok(json!({"repos": repos.clone(), "repositories": repos}));
    }
    let mut rows = state
        .worktrees
        .iter()
        .map(|worktree| {
            json!({
                "id": worktree.id.0,
                "fullWorktreeId": worktree.id.0,
                "repoId": worktree.repo_id.0,
                "path": worktree.path,
                "branch": worktree.branch,
                "displayName": worktree.display_name,
                "main": state.repos.iter().any(|repo| repo.id == worktree.repo_id && repo.path == worktree.path),
            })
        })
        .collect::<Vec<_>>();
    if let Some(root) = PROJECT_ROOT.get() {
        if !state
            .worktrees
            .iter()
            .any(|worktree| worktree.path == *root)
        {
            rows.push(json!({
                "id": root.to_string_lossy(),
                "fullWorktreeId": root.to_string_lossy(),
                "repoId": root.to_string_lossy(),
                "path": root,
                "branch": "HEAD",
                "displayName": root.file_name().and_then(|name| name.to_str()).unwrap_or("project"),
                "main": true,
            }));
        }
    }
    if method == "worktree.list" {
        let repo = params.get("repo").and_then(Value::as_str).map(|value| {
            value
                .strip_prefix("id:")
                .or_else(|| value.strip_prefix("path:"))
                .or_else(|| value.strip_prefix("name:"))
                .unwrap_or(value)
        });
        let filtered = rows
            .into_iter()
            .filter(|row| {
                repo.is_none_or(|repo| {
                    row["repoId"].as_str() == Some(repo)
                        || state.repos.iter().any(|candidate| {
                            (candidate.display_name == repo
                                || candidate.path == std::path::Path::new(repo))
                                && row["repoId"].as_str() == Some(candidate.id.0.as_str())
                        })
                })
            })
            .collect::<Vec<_>>();
        return Ok(json!({"worktrees": filtered}));
    }
    let selector = runtime_string_param(params, "worktree")?;
    let path = runtime_worktree_path(params)?;
    rows.into_iter()
        .find(|row| {
            row["id"].as_str() == Some(selector)
                || row["path"].as_str() == path.to_str()
                || row["branch"].as_str() == Some(selector)
                || row["displayName"].as_str() == Some(selector)
        })
        .map(|worktree| json!({"worktree": worktree}))
        .ok_or_else(|| format!("Remote worktree not found: {selector}"))
}

fn runtime_string_param<'a>(params: &'a Value, name: &str) -> Result<&'a str, String> {
    params
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("Runtime parameter {name} is required."))
}

fn runtime_mutation_id(params: &Value) -> Result<Option<&str>, String> {
    let Some(value) = params.get("clientMutationId") else {
        return Ok(None);
    };
    let mutation_id = value
        .as_str()
        .ok_or_else(|| "clientMutationId must be a string.".to_string())?;
    if mutation_id.is_empty() || mutation_id.chars().count() > 128 {
        return Err("clientMutationId must contain from 1 to 128 characters.".into());
    }
    Ok(Some(mutation_id))
}

fn runtime_terminal_create_handle(
    caller_fingerprint: &str,
    worktree: &std::path::Path,
    mutation_id: &str,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"orca.remote-terminal-create.v2\0");
    digest.update(caller_fingerprint.as_bytes());
    digest.update(b"\0");
    digest.update(worktree.to_string_lossy().as_bytes());
    digest.update(b"\0");
    digest.update(mutation_id.as_bytes());
    let hex = format!("{:x}", digest.finalize());
    format!("term_{}", &hex[..32])
}

fn runtime_worktree_create_key(caller: &str, repo: &str, mutation_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"orca.remote-worktree-create.v1\0");
    digest.update(caller.as_bytes());
    digest.update(b"\0");
    digest.update(repo.as_bytes());
    digest.update(b"\0");
    digest.update(mutation_id.as_bytes());
    format!("{:x}", digest.finalize())
}

fn runtime_worktree_path(params: &Value) -> Result<std::path::PathBuf, String> {
    let state =
        suaegi_core::persistence::Store::new(crate::persistence_thread::default_data_file())
            .load()
            .state;
    resolve_runtime_worktree_path(params, &state, PROJECT_ROOT.get().map(PathBuf::as_path))
}

fn resolve_runtime_worktree_path(
    params: &Value,
    state: &suaegi_core::domain::PersistedState,
    project_root: Option<&std::path::Path>,
) -> Result<std::path::PathBuf, String> {
    let selector = runtime_string_param(params, "worktree")?;
    let selector = selector
        .strip_prefix("id:")
        .or_else(|| selector.strip_prefix("path:"))
        .or_else(|| selector.strip_prefix("name:"))
        .unwrap_or(selector);
    if let Some(root) = project_root {
        if matches!(selector, "active" | "current" | "project" | "root")
            || std::path::Path::new(selector) == root
            || root
                .file_name()
                .is_some_and(|name| name.to_string_lossy() == selector)
        {
            return Ok(root.to_path_buf());
        }
    }
    if let Some(worktree) = state.worktrees.iter().find(|worktree| {
        worktree.id.0 == selector
            || worktree.path == std::path::Path::new(selector)
            || worktree.branch == selector
            || worktree.display_name == selector
            || worktree
                .path
                .file_name()
                .is_some_and(|name| name.to_string_lossy() == selector)
    }) {
        return Ok(worktree.path.clone());
    }
    if let Some(repo) = state.repos.iter().find(|repo| {
        repo.id.0 == selector
            || repo.path == std::path::Path::new(selector)
            || repo.display_name == selector
    }) {
        return Ok(repo.path.clone());
    }
    Err(format!("Remote worktree not found: {selector}"))
}

fn system_time_millis(time: std::time::SystemTime) -> Result<f64, String> {
    time.duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64() * 1_000.0)
        .map_err(|_| "Remote file timestamp predates the Unix epoch.".into())
}

async fn runtime_git_status(worktree: &std::path::Path) -> Result<Value, String> {
    let runner = suaegi_git::runner::GitRunner::new();
    let entries = suaegi_git::status::working_tree_status_detailed(&runner, worktree)
        .await
        .map_err(|error| error.to_string())?;
    let mut rows = Vec::new();
    for entry in entries {
        let (status, old_path, conflict_kind) = runtime_file_status(&entry.status);
        for area in [
            entry.staged.then_some("staged"),
            entry.unstaged.then_some("unstaged"),
        ]
        .into_iter()
        .flatten()
        {
            rows.push(json!({
                "path": entry.path,
                "status": status,
                "area": area,
                "oldPath": old_path,
                "conflictKind": conflict_kind,
            }));
        }
    }
    Ok(json!({"entries": rows}))
}

fn runtime_file_status(
    status: &suaegi_git::status::FileStatus,
) -> (&'static str, Option<&str>, Option<&'static str>) {
    use suaegi_git::status::{ConflictKind, FileStatus};
    match status {
        FileStatus::Modified => ("modified", None, None),
        FileStatus::Added => ("added", None, None),
        FileStatus::Deleted => ("deleted", None, None),
        FileStatus::Renamed { from } => ("renamed", Some(from), None),
        FileStatus::Copied { from } => ("copied", Some(from), None),
        FileStatus::Untracked => ("untracked", None, None),
        FileStatus::Conflicted(kind) => (
            "conflicted",
            None,
            Some(match kind {
                ConflictKind::BothModified => "both_modified",
                ConflictKind::BothAdded => "both_added",
                ConflictKind::BothDeleted => "both_deleted",
                ConflictKind::AddedByUs => "added_by_us",
                ConflictKind::AddedByThem => "added_by_them",
                ConflictKind::DeletedByUs => "deleted_by_us",
                ConflictKind::DeletedByThem => "deleted_by_them",
            }),
        ),
        FileStatus::Other(_) => ("modified", None, None),
    }
}

async fn runtime_branch_compare(
    worktree: &std::path::Path,
    base_ref: &str,
) -> Result<Value, String> {
    use suaegi_git::compare::{ChangeStatus, CompareHandle, CompareOutcome};
    let outcome = crate::git_tasks::compare_worktree_now(
        worktree.to_path_buf(),
        base_ref.to_string(),
        CompareHandle::new(),
    )
    .await
    .map_err(|error| format!("{error:?}"))?;
    match outcome {
        CompareOutcome::Ready(compare) => {
            let entries = compare
                .files
                .into_iter()
                .map(|file| {
                    let (status, old_path) = match file.status {
                        ChangeStatus::Modified => ("modified", None),
                        ChangeStatus::Added => ("added", None),
                        ChangeStatus::Deleted => ("deleted", None),
                        ChangeStatus::Renamed { from } => ("renamed", Some(from)),
                        ChangeStatus::Copied { from } => ("copied", Some(from)),
                        ChangeStatus::Other(_) => ("modified", None),
                    };
                    json!({
                        "path": file.path,
                        "status": status,
                        "oldPath": old_path,
                        "added": file.additions,
                        "removed": file.deletions,
                    })
                })
                .collect::<Vec<_>>();
            Ok(json!({
                "summary": {
                    "status": "ready",
                    "baseRef": base_ref,
                    "mergeBase": compare.merge_base,
                    "commitsAhead": compare.ahead_count,
                },
                "entries": entries,
            }))
        }
        CompareOutcome::NoMergeBase => Ok(json!({
            "summary": {"status": "no-merge-base", "baseRef": base_ref},
            "entries": [],
        })),
        CompareOutcome::UnbornHead => Ok(json!({
            "summary": {"status": "unborn-head", "baseRef": base_ref},
            "entries": [],
        })),
        CompareOutcome::InvalidBase => Ok(json!({
            "summary": {"status": "invalid-base", "baseRef": base_ref},
            "entries": [],
        })),
        CompareOutcome::Cancelled => Err("Remote branch comparison was cancelled.".into()),
    }
}

async fn runtime_branch_diff(worktree: &std::path::Path, params: &Value) -> Result<Value, String> {
    use suaegi_git::compare::{ChangeStatus, CompareHandle, CompareOutcome, FileSource};
    let file = runtime_string_param(params, "filePath")?;
    let compare_param = params
        .get("compare")
        .ok_or_else(|| "Runtime parameter compare is required.".to_string())?;
    let base_ref = compare_param
        .get("baseRef")
        .and_then(Value::as_str)
        .or_else(|| compare_param.get("mergeBase").and_then(Value::as_str))
        .ok_or_else(|| "Remote diff compare has no base ref.".to_string())?;
    let comparison = crate::git_tasks::compare_worktree_now(
        worktree.to_path_buf(),
        base_ref.to_string(),
        CompareHandle::new(),
    )
    .await
    .map_err(|error| format!("{error:?}"))?;
    let CompareOutcome::Ready(comparison) = comparison else {
        return Err("Remote diff comparison is not ready.".into());
    };
    let changed = comparison
        .files
        .iter()
        .find(|entry| entry.path == file)
        .ok_or_else(|| format!("Remote diff file was not found: {file}"))?;
    if matches!(changed.status, ChangeStatus::Other(_)) {
        return Ok(json!({"kind": "non-renderable"}));
    }
    let runner = suaegi_git::runner::GitRunner::new();
    let original_path = match &changed.status {
        ChangeStatus::Renamed { from } | ChangeStatus::Copied { from } => from.as_str(),
        _ => file,
    };
    let original = if matches!(changed.status, ChangeStatus::Added) {
        Vec::new()
    } else {
        suaegi_git::compare::file_head_bytes(
            &runner,
            worktree,
            FileSource::Revision(comparison.merge_base.clone()),
            original_path,
            suaegi_git::runner::MAX_DIFF_BYTES,
        )
        .await
        .map_err(|error| error.to_string())?
    };
    let modified = if matches!(changed.status, ChangeStatus::Deleted) {
        Vec::new()
    } else {
        suaegi_git::compare::file_head_bytes(
            &runner,
            worktree,
            FileSource::WorkingTree,
            file,
            suaegi_git::runner::MAX_DIFF_BYTES,
        )
        .await
        .map_err(|error| error.to_string())?
    };
    if original.contains(&0) || modified.contains(&0) {
        return Ok(json!({"kind": "binary"}));
    }
    let original = String::from_utf8(original)
        .map_err(|_| "Remote original file is not UTF-8.".to_string())?;
    let modified = String::from_utf8(modified)
        .map_err(|_| "Remote modified file is not UTF-8.".to_string())?;
    Ok(json!({
        "kind": "text",
        "originalContent": original,
        "modifiedContent": modified,
    }))
}

fn runtime_status(desktop: Option<Value>) -> Value {
    let state =
        suaegi_core::persistence::Store::new(crate::persistence_thread::default_data_file())
            .load()
            .state;
    let desktop_running = desktop.is_some();
    let mut status = desktop.unwrap_or_else(|| {
        json!({
            "app": "Suaegi",
            "running": false,
            "repositories": state.repos.len(),
            "worktrees": state.worktrees.len(),
            "surface": "headless",
        })
    });
    let Some(object) = status.as_object_mut() else {
        return status;
    };
    object.insert(
        "runtimeId".into(),
        Value::String(format!("suaegi-{}", std::process::id())),
    );
    object.insert(
        "version".into(),
        Value::String(env!("CARGO_PKG_VERSION").into()),
    );
    object.insert(
        "appVersion".into(),
        Value::String(env!("CARGO_PKG_VERSION").into()),
    );
    object.insert("protocolVersion".into(), Value::from(1));
    object.insert("runtimeProtocolVersion".into(), Value::from(1));
    object.insert("minCompatibleMobileVersion".into(), Value::from(0));
    object.insert("minCompatibleRuntimeClientVersion".into(), Value::from(0));
    object.insert("deviceScope".into(), Value::String("runtime".into()));
    object.insert(
        "capabilities".into(),
        json!([
            "runtime.status.compat.v1",
            "worktree.create-idempotency.v1",
            "terminal.create-idempotency.v2",
            "orchestration.contract.v1",
            "orchestration.federation.v1",
            "orchestration.federation-control-mail.v1",
        ]),
    );
    object.insert(
        "desktopWindowStatus".into(),
        Value::String(if desktop_running { "open" } else { "closed" }.into()),
    );
    status
}

fn decode_public_key(value: &str) -> Result<PublicKey, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|_| "Client public key is invalid.".to_string())?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "Client public key must be 32 bytes.".to_string())?;
    Ok(PublicKey::from(bytes))
}

fn encrypt_text(cipher: &SalsaBox, plaintext: &str) -> Result<String, String> {
    let nonce = SalsaBox::generate_nonce(&mut OsRng);
    let mut ciphertext = plaintext.as_bytes().to_vec();
    let tag = cipher
        .encrypt_in_place_detached(&nonce, b"", &mut ciphertext)
        .map_err(|_| "Could not encrypt a runtime response.".to_string())?;
    let mut frame = Vec::with_capacity(nonce.len() + tag.len() + ciphertext.len());
    frame.extend_from_slice(&nonce);
    frame.extend_from_slice(&tag);
    frame.extend_from_slice(&ciphertext);
    Ok(base64::engine::general_purpose::STANDARD.encode(frame))
}

fn decrypt_text(cipher: &SalsaBox, frame: &str) -> Result<String, String> {
    if frame.len() > MAX_FRAME_BYTES * 2 {
        return Err("Encrypted runtime frame is too large.".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(frame)
        .map_err(|_| "Encrypted runtime frame is invalid.".to_string())?;
    if bytes.len() < 40 || bytes.len() > MAX_FRAME_BYTES {
        return Err("Encrypted runtime frame has an invalid size.".into());
    }
    let nonce = crypto_box::Nonce::from_slice(&bytes[..24]);
    let tag = crypto_box::Tag::from_slice(&bytes[24..40]);
    let mut plaintext = bytes[40..].to_vec();
    cipher
        .decrypt_in_place_detached(nonce, b"", &mut plaintext, tag)
        .map_err(|_| "Runtime frame authentication failed.".to_string())?;
    String::from_utf8(plaintext).map_err(|_| "Runtime frame is not UTF-8.".to_string())
}

async fn receive_text(
    socket: &mut tokio_tungstenite::WebSocketStream<TcpStream>,
) -> Result<String, String> {
    loop {
        let next = tokio::time::timeout(Duration::from_secs(300), socket.next())
            .await
            .map_err(|_| "Runtime connection timed out.".to_string())?;
        match next {
            Some(Ok(Message::Text(text))) if text.len() <= MAX_FRAME_BYTES => {
                return Ok(text.to_string());
            }
            Some(Ok(Message::Ping(payload))) => {
                socket
                    .send(Message::Pong(payload))
                    .await
                    .map_err(|_| "Runtime connection failed.".to_string())?;
            }
            Some(Ok(Message::Pong(_))) => {}
            Some(Ok(Message::Close(_))) | None => return Err("Runtime connection closed.".into()),
            Some(Ok(_)) => return Err("Unexpected runtime WebSocket frame.".into()),
            Some(Err(_)) => return Err("Runtime WebSocket failed.".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn ensure_test_daemon() {
        static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        INIT.get_or_init(|| {
            if suaegi_term::daemon::configured() {
                return;
            }
            let runtime_dir = tempfile::tempdir().unwrap().keep();
            let server_dir = runtime_dir.clone();
            std::thread::spawn(move || {
                let _ = suaegi_term::daemon::run(&server_dir);
            });
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while (!runtime_dir.join("pty-v1.sock").exists()
                || !runtime_dir.join("pty-v1.token").exists())
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            suaegi_term::daemon::configure(suaegi_term::daemon::DaemonConfiguration {
                executable: "/bin/false".into(),
                runtime_dir,
            })
            .unwrap();
        });
    }

    #[test]
    fn pairing_code_is_accepted_by_the_runtime_client_parser() {
        let secret = SecretKey::generate(&mut OsRng);
        let code = encode_pairing_code("ws://127.0.0.1:6768", "token", &secret.public_key());
        assert!(crate::remote_runtime::validate_runtime_pairing_code(&code).is_ok());
    }

    #[test]
    fn advertised_addresses_match_host_and_url_forms() {
        assert_eq!(
            advertised_endpoint(None, 6768).unwrap(),
            "ws://127.0.0.1:6768"
        );
        assert_eq!(
            advertised_endpoint(Some("100.64.1.20"), 6768).unwrap(),
            "ws://100.64.1.20:6768"
        );
        assert_eq!(
            advertised_endpoint(Some("wss://server.example"), 6768).unwrap(),
            "wss://server.example:6768"
        );
    }

    #[test]
    fn status_get_shape_contains_orca_runtime_compatibility_fields() {
        let status = runtime_status(Some(json!({
            "app": "Suaegi",
            "running": true,
            "repositories": 1,
            "worktrees": 2,
            "surface": "workbench",
        })));
        assert_eq!(status["appVersion"], env!("CARGO_PKG_VERSION"));
        assert_eq!(status["runtimeProtocolVersion"], 1);
        assert_eq!(status["minCompatibleRuntimeClientVersion"], 0);
        assert_eq!(status["deviceScope"], "runtime");
        assert_eq!(status["desktopWindowStatus"], "open");
        let capabilities = status["capabilities"].as_array().unwrap();
        assert!(capabilities.contains(&json!("worktree.create-idempotency.v1")));
        assert!(capabilities.contains(&json!("terminal.create-idempotency.v2")));
    }

    #[test]
    fn terminal_create_identity_is_stable_and_caller_scoped() {
        let worktree = std::path::Path::new("/tmp/suaegi-idempotency");
        let first = runtime_terminal_create_handle("caller-a", worktree, "mutation-1");
        let retry = runtime_terminal_create_handle("caller-a", worktree, "mutation-1");
        let other_caller = runtime_terminal_create_handle("caller-b", worktree, "mutation-1");
        assert_eq!(first, retry);
        assert_ne!(first, other_caller);
        assert!(first.starts_with("term_"));
        assert_eq!(first.len(), 37);
        assert!(runtime_mutation_id(&json!({"clientMutationId": ""})).is_err());
        assert!(runtime_mutation_id(&json!({"clientMutationId": "x".repeat(129)})).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn headless_terminal_send_read_and_close_use_the_survival_daemon() {
        ensure_test_daemon();
        let handle = format!("runtime-test-{}", std::process::id());
        let spec = suaegi_term::daemon::SpawnSpec {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "printf 'ready\\n'; IFS= read -r line; printf 'got:%s\\n' \"$line\"; sleep 1"
                    .into(),
            ],
            cwd: None,
            env: std::env::vars().collect(),
            rows: 24,
            cols: 80,
        };
        let (session, reader, _) =
            suaegi_term::daemon::DaemonClientSession::create_or_attach(handle.clone(), spec)
                .unwrap();
        drop(reader);
        session.disconnect();

        runtime_terminal(
            "terminal.send",
            &json!({"terminal": handle, "text": "hello\n"}),
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let read = runtime_terminal("terminal.read", &json!({"terminal": handle, "limit": 100}))
            .await
            .unwrap();
        assert!(read["terminal"]["output"]
            .as_str()
            .unwrap()
            .contains("got:hello"));
        let closed = runtime_terminal("terminal.close", &json!({"terminal": handle}))
            .await
            .unwrap();
        assert_eq!(closed["closed"], true);
    }

    #[tokio::test]
    async fn headless_project_root_can_create_persist_and_remove_a_worktree() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("project");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {:?}: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "Suaegi Test"]);
        git(&["config", "user.email", "suaegi-test@example.invalid"]);
        std::fs::write(repo.join("README.md"), "headless runtime\n").unwrap();
        git(&["add", "README.md"]);
        git(&["commit", "-qm", "initial"]);

        let data_file = temp.path().join("state").join("data.json");
        let mut initial = suaegi_core::domain::PersistedState::default();
        initial.settings.workspace_root = temp.path().join("workspaces");
        suaegi_core::persistence::Store::new(data_file.clone())
            .save(&initial)
            .unwrap();

        let created = runtime_worktree_mutation_at(
            "worktree.create",
            &json!({
                "name": "remote-worker",
                "repo": repo,
                "baseBranch": "HEAD",
                "displayName": "Remote Worker",
                "comment": "federated QA",
                "noParent": true,
                "setup": "skip",
                "clientMutationId": "create-remote-worker",
            }),
            &RuntimeRequestContext {
                caller_fingerprint: "test-runtime".into(),
                orchestration_request_id: None,
            },
            data_file.clone(),
            Some(&repo),
        )
        .await
        .unwrap();
        let path = PathBuf::from(created["path"].as_str().unwrap());
        assert!(path.is_dir());
        assert_eq!(created["worktree"]["displayName"], "Remote Worker");

        let persisted = suaegi_core::persistence::Store::new(data_file.clone())
            .load()
            .state;
        assert_eq!(persisted.repos.len(), 1);
        assert_eq!(persisted.worktrees.len(), 1);
        assert_eq!(
            persisted
                .settings
                .ui
                .worktree_comments
                .get(path.to_str().unwrap()),
            Some(&"federated QA".to_string())
        );
        assert!(!persisted
            .settings
            .ui
            .worktree_parents
            .contains_key(path.to_str().unwrap()));
        assert_eq!(
            persisted.settings.ui.runtime_worktree_create_receipts.len(),
            1
        );

        let replayed = runtime_worktree_mutation_at(
            "worktree.create",
            &json!({
                "name": "remote-worker",
                "repo": repo,
                "baseBranch": "HEAD",
                "setup": "skip",
                "clientMutationId": "create-remote-worker",
            }),
            &RuntimeRequestContext {
                caller_fingerprint: "test-runtime".into(),
                orchestration_request_id: None,
            },
            data_file.clone(),
            Some(&repo),
        )
        .await
        .unwrap();
        assert_eq!(replayed["path"], created["path"]);
        assert_eq!(replayed["idempotentReplay"], true);
        let persisted = suaegi_core::persistence::Store::new(data_file.clone())
            .load()
            .state;
        assert_eq!(persisted.worktrees.len(), 1);

        let removed = runtime_worktree_mutation_at(
            "worktree.remove",
            &json!({"worktree": path, "force": false}),
            &RuntimeRequestContext {
                caller_fingerprint: "test-runtime".into(),
                orchestration_request_id: None,
            },
            data_file.clone(),
            Some(&repo),
        )
        .await
        .unwrap();
        assert_eq!(removed["removed"].as_str(), path.to_str());
        assert!(!path.exists());
        let persisted = suaegi_core::persistence::Store::new(data_file).load().state;
        assert!(persisted.worktrees.is_empty());
        assert!(persisted.settings.ui.worktree_comments.is_empty());
        assert!(persisted.settings.ui.worktree_parents.is_empty());
    }

    #[tokio::test]
    async fn websocket_pairing_completes_the_orca_legacy_e2ee_handshake() {
        let orchestration_state = tempfile::tempdir().unwrap();
        let orchestration_state_file = orchestration_state.path().join("orchestration.json");
        // This test owns the only orchestration persistence access in this test
        // module and restores the process environment after the server exits.
        unsafe {
            std::env::set_var("SUAEGI_ORCHESTRATION_STATE_PATH", &orchestration_state_file);
        }
        let _ = PROJECT_ROOT.set(std::env::current_dir().unwrap().canonicalize().unwrap());
        #[cfg(unix)]
        let stream_terminal = {
            ensure_test_daemon();
            let handle = format!("runtime-stream-test-{}", std::process::id());
            let spec = suaegi_term::daemon::SpawnSpec {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "printf 'stream-ready\\n'; sleep 30".into()],
                cwd: None,
                env: std::env::vars().collect(),
                rows: 24,
                cols: 80,
            };
            let (session, reader, _) =
                suaegi_term::daemon::DaemonClientSession::create_or_attach(handle.clone(), spec)
                    .unwrap();
            drop(reader);
            session.disconnect();
            handle
        };
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_secret = SecretKey::generate(&mut OsRng);
        let server_public = server_secret.public_key();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _ = handle_connection(stream, server_secret, "test-token".into()).await;
        });

        let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{address}"))
            .await
            .unwrap();
        let client_secret = SecretKey::generate(&mut OsRng);
        let cipher = SalsaBox::new(&server_public, &client_secret);
        client
            .send(Message::Text(
                json!({
                    "type": "e2ee_hello",
                    "publicKeyB64": base64::engine::general_purpose::STANDARD
                        .encode(client_secret.public_key().as_bytes())
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let ready = client.next().await.unwrap().unwrap().into_text().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&ready).unwrap()["type"],
            "e2ee_ready"
        );
        let auth = encrypt_text(
            &cipher,
            &json!({"type":"e2ee_auth","deviceToken":"test-token"}).to_string(),
        )
        .unwrap();
        client.send(Message::Text(auth.into())).await.unwrap();
        let authenticated = client.next().await.unwrap().unwrap().into_text().unwrap();
        let authenticated = decrypt_text(&cipher, &authenticated).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&authenticated).unwrap()["type"],
            "e2ee_authenticated"
        );
        let status_request = encrypt_text(
            &cipher,
            &json!({
                "id": "status-1",
                "method": "status.get",
                "deviceToken": "test-token",
            })
            .to_string(),
        )
        .unwrap();
        client
            .send(Message::Text(status_request.into()))
            .await
            .unwrap();
        let status_response = client.next().await.unwrap().unwrap().into_text().unwrap();
        let status_response: Value =
            serde_json::from_str(&decrypt_text(&cipher, &status_response).unwrap()).unwrap();
        assert_eq!(status_response["id"], "status-1");
        assert_eq!(status_response["result"]["deviceScope"], "runtime");
        assert_eq!(status_response["result"]["runtimeProtocolVersion"], 1);
        assert!(status_response["result"]["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|capability| capability == "orchestration.federation.v1"));
        #[cfg(unix)]
        {
            let attach = encrypt_text(
                &cipher,
                &json!({
                    "id": "federation-attach-1",
                    "method": "orchestration.federationAttachStart",
                    "orchestrationRequestId": "mutation-federation-attach-1",
                    "deviceToken": "test-token",
                    "params": {
                        "dispatchId": "dispatch-federation-test-1",
                        "taskId": "task-federation-test-1",
                        "taskSpec": "Verify the encrypted federation path",
                        "protocolVersion": 2,
                        "worktree": "root",
                        "terminal": stream_terminal,
                        "timeoutMs": 5_000,
                    },
                })
                .to_string(),
            )
            .unwrap();
            client.send(Message::Text(attach.into())).await.unwrap();
            let attached = client.next().await.unwrap().unwrap().into_text().unwrap();
            let attached: Value =
                serde_json::from_str(&decrypt_text(&cipher, &attached).unwrap()).unwrap();
            assert_eq!(attached["id"], "federation-attach-1");
            assert_eq!(attached["result"]["state"], "ready");
            assert_eq!(
                attached["result"]["terminalHandle"],
                stream_terminal.as_str()
            );

            let import = encrypt_text(
                &cipher,
                &json!({
                    "id": "federation-import-1",
                    "method": "orchestration.federationImport",
                    "orchestrationRequestId": "mutation-federation-import-1",
                    "deviceToken": "test-token",
                    "params": {
                        "dispatchId": "dispatch-federation-test-1",
                        "items": [{
                            "dispatch_id": "dispatch-federation-test-1",
                            "direction": "to_worker",
                            "sequence": 1,
                            "message_id": "message-control-test-1",
                            "kind": "control_message",
                            "payload": json!({
                                "from":"run-home",
                                "subject":"continue",
                                "body":"run the final verification",
                                "type":"status",
                                "priority":"normal",
                                "threadId":null,
                                "payload":null,
                            }).to_string(),
                        }],
                    },
                })
                .to_string(),
            )
            .unwrap();
            client.send(Message::Text(import.into())).await.unwrap();
            let imported = client.next().await.unwrap().unwrap().into_text().unwrap();
            let imported: Value =
                serde_json::from_str(&decrypt_text(&cipher, &imported).unwrap()).unwrap();
            assert_eq!(imported["result"]["acknowledgedThrough"], 1);
            let checked = crate::orchestration::run(
                "check",
                &[
                    "--terminal".into(),
                    stream_terminal.clone(),
                    "--peek".into(),
                ],
            )
            .unwrap();
            assert_eq!(checked["messages"][0]["subject"], "continue");

            let question_terminal = stream_terminal.clone();
            let question = std::thread::spawn(move || {
                crate::orchestration::run(
                    "ask",
                    &[
                        "--from".into(),
                        question_terminal,
                        "--question".into(),
                        "Which verification mode?".into(),
                        "--timeout-ms".into(),
                        "5000".into(),
                    ],
                )
            });
            tokio::time::sleep(Duration::from_millis(100)).await;
            let pull_question = encrypt_text(
                &cipher,
                &json!({
                    "id": "federation-pull-question-1",
                    "method": "orchestration.federationPull",
                    "deviceToken": "test-token",
                    "params": {
                        "dispatchId": "dispatch-federation-test-1",
                        "afterSequence": 0,
                        "limit": 50,
                    },
                })
                .to_string(),
            )
            .unwrap();
            client
                .send(Message::Text(pull_question.into()))
                .await
                .unwrap();
            let pulled_question = client.next().await.unwrap().unwrap().into_text().unwrap();
            let pulled_question: Value =
                serde_json::from_str(&decrypt_text(&cipher, &pulled_question).unwrap()).unwrap();
            let question_id = pulled_question["result"]["items"][0]["message_id"]
                .as_str()
                .unwrap()
                .to_string();
            let import_reply = encrypt_text(
                &cipher,
                &json!({
                    "id": "federation-import-reply-1",
                    "method": "orchestration.federationImport",
                    "orchestrationRequestId": "mutation-federation-import-reply-1",
                    "deviceToken": "test-token",
                    "params": {
                        "dispatchId": "dispatch-federation-test-1",
                        "items": [{
                            "dispatch_id": "dispatch-federation-test-1",
                            "direction": "to_worker",
                            "sequence": 2,
                            "message_id": "message-reply-test-1",
                            "kind": "control_message",
                            "payload": json!({
                                "from":"run-home",
                                "subject":"Re: Which verification mode?",
                                "body":"Run the full suite",
                                "type":"reply",
                                "priority":"normal",
                                "threadId":question_id,
                                "payload":null,
                                "replyTo":question_id,
                            }).to_string(),
                        }],
                    },
                })
                .to_string(),
            )
            .unwrap();
            client
                .send(Message::Text(import_reply.into()))
                .await
                .unwrap();
            let imported_reply = client.next().await.unwrap().unwrap().into_text().unwrap();
            let imported_reply: Value =
                serde_json::from_str(&decrypt_text(&cipher, &imported_reply).unwrap()).unwrap();
            assert_eq!(imported_reply["result"]["acknowledgedThrough"], 2);
            let answered = question.join().unwrap().unwrap();
            assert_eq!(answered["reply"]["body"], "Run the full suite");

            let relayed = crate::orchestration::run(
                "send",
                &[
                    "--from".into(),
                    stream_terminal.clone(),
                    "--type".into(),
                    "worker_done".into(),
                    "--subject".into(),
                    "federated task complete".into(),
                    "--task-id".into(),
                    "task-federation-test-1".into(),
                    "--dispatch-id".into(),
                    "dispatch-federation-test-1".into(),
                    "--outcome".into(),
                    "succeeded".into(),
                ],
            )
            .unwrap();
            assert_eq!(relayed["relay"], true);
            let pull = encrypt_text(
                &cipher,
                &json!({
                    "id": "federation-pull-1",
                    "method": "orchestration.federationPull",
                    "deviceToken": "test-token",
                    "params": {
                        "dispatchId": "dispatch-federation-test-1",
                        "afterSequence": 1,
                        "limit": 50,
                    },
                })
                .to_string(),
            )
            .unwrap();
            client.send(Message::Text(pull.into())).await.unwrap();
            let pulled = client.next().await.unwrap().unwrap().into_text().unwrap();
            let pulled: Value =
                serde_json::from_str(&decrypt_text(&cipher, &pulled).unwrap()).unwrap();
            assert_eq!(pulled["id"], "federation-pull-1");
            assert_eq!(pulled["result"]["items"][0]["sequence"], 2);
            assert_eq!(
                pulled["result"]["items"][0]["dispatch_id"],
                "dispatch-federation-test-1"
            );

            let subscribe = encrypt_text(
                &cipher,
                &json!({
                    "id": "terminal-stream-1",
                    "method": "terminal.subscribe",
                    "deviceToken": "test-token",
                    "params": {"terminal": stream_terminal},
                })
                .to_string(),
            )
            .unwrap();
            client.send(Message::Text(subscribe.into())).await.unwrap();
            let subscribed = client.next().await.unwrap().unwrap().into_text().unwrap();
            let subscribed: Value =
                serde_json::from_str(&decrypt_text(&cipher, &subscribed).unwrap()).unwrap();
            assert_eq!(subscribed["id"], "terminal-stream-1");
            assert_eq!(subscribed["result"]["type"], "subscribed");
            assert!(subscribed["result"]["lines"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .any(|line| line.contains("stream-ready")));
            let stop = encrypt_text(
                &cipher,
                &json!({
                    "id": "federation-stop-1",
                    "method": "orchestration.federationStop",
                    "deviceToken": "test-token",
                    "params": {"dispatchId": "dispatch-federation-test-1"},
                })
                .to_string(),
            )
            .unwrap();
            client.send(Message::Text(stop.into())).await.unwrap();
            loop {
                let response = client.next().await.unwrap().unwrap().into_text().unwrap();
                let response: Value =
                    serde_json::from_str(&decrypt_text(&cipher, &response).unwrap()).unwrap();
                if response["id"] == "federation-stop-1" {
                    assert_eq!(response["result"]["state"], "stopped");
                    assert_eq!(response["result"]["processAction"], "closed_agent_terminal");
                    break;
                }
            }
        }
        client.close(None).await.unwrap();
        server.await.unwrap();
        unsafe {
            std::env::remove_var("SUAEGI_ORCHESTRATION_STATE_PATH");
        }
    }
}
