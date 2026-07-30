//! Out-of-process JavaScript worker command bridge for Orca plugin API v1.
//!
//! Workers receive an allowlisted environment and communicate over bounded
//! JSON lines. Every worker-originated host call is re-gated in Rust.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures::channel::{mpsc, oneshot};
use futures::{SinkExt, Stream, StreamExt};
use iced::Subscription;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};

const READY_TIMEOUT: Duration = Duration::from_secs(10);
const INVOKE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PROTOCOL_LINE_BYTES: usize = 1024 * 1024;
const MAX_HOST_CALLS: usize = 1_024;
const MAX_ACTIVE_WORKERS: usize = 5;
const IDLE_REAP_AFTER: Duration = Duration::from_secs(5 * 60);

type HostResponse = Result<Value, String>;

#[derive(Clone)]
pub struct PluginHostRequest {
    pub plugin_key: String,
    pub method: String,
    pub params: Value,
    responder: Arc<Mutex<Option<oneshot::Sender<HostResponse>>>>,
}

impl std::fmt::Debug for PluginHostRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginHostRequest")
            .field("plugin_key", &self.plugin_key)
            .field("method", &self.method)
            .field("params", &self.params)
            .finish_non_exhaustive()
    }
}

impl PluginHostRequest {
    pub fn respond(&self, response: HostResponse) {
        if let Some(responder) = self
            .responder
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = responder.send(response);
        }
    }

    #[cfg(test)]
    pub fn for_test(plugin_key: &str, method: &str, params: Value) -> Self {
        let (responder, _response) = oneshot::channel();
        Self {
            plugin_key: plugin_key.to_string(),
            method: method.to_string(),
            params,
            responder: Arc::new(Mutex::new(Some(responder))),
        }
    }
}

struct HostChannel {
    sender: mpsc::UnboundedSender<PluginHostRequest>,
    receiver: futures::lock::Mutex<mpsc::UnboundedReceiver<PluginHostRequest>>,
}

fn host_channel() -> &'static HostChannel {
    static CHANNEL: OnceLock<HostChannel> = OnceLock::new();
    CHANNEL.get_or_init(|| {
        let (sender, receiver) = mpsc::unbounded();
        HostChannel {
            sender,
            receiver: futures::lock::Mutex::new(receiver),
        }
    })
}

async fn invoke_app_host(plugin_key: &str, method: &str, params: Value) -> Result<Value, String> {
    let (responder, response) = oneshot::channel();
    host_channel()
        .sender
        .clone()
        .send(PluginHostRequest {
            plugin_key: plugin_key.to_string(),
            method: method.to_string(),
            params,
            responder: Arc::new(Mutex::new(Some(responder))),
        })
        .await
        .map_err(|_| "Suaegi app host is unavailable.".to_string())?;
    response
        .await
        .map_err(|_| "Suaegi app host dropped the plugin request.".to_string())?
}

/// Route the deliberately smaller panel API through the same UI-owned host
/// gate used by out-of-process plugin workers.
pub async fn invoke_panel_host(
    plugin_key: &str,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    if !matches!(
        method,
        "workspace.readContext" | "terminal.sendText" | "notifications.show"
    ) {
        return Err("plugin host method is not callable from panels".into());
    }
    invoke_app_host(plugin_key, method, params).await
}

fn host_request_stream() -> impl Stream<Item = crate::state::Message> {
    futures::stream::unfold((), |_| async {
        let request = host_channel().receiver.lock().await.next().await?;
        Some((crate::state::Message::PluginHostCallRequested(request), ()))
    })
}

pub fn subscription() -> Subscription<crate::state::Message> {
    Subscription::batch([
        Subscription::run(host_request_stream),
        iced::time::every(Duration::from_secs(60))
            .map(|_| crate::state::Message::PluginWorkerReapTick),
    ])
}

const WORKER_SCRIPT: &str = r#"
import { createInterface } from 'node:readline';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

const [root, mainEntry, capabilitiesJson] = process.argv.slice(1);
const capabilities = JSON.parse(capabilitiesJson);
const handlers = new Map();
const eventHandlers = new Map();
const pending = new Map();
let callId = 0;
let queue = Promise.resolve();
const send = value => process.stdout.write(`${JSON.stringify(value)}\n`);
const output = (...values) => process.stderr.write(`${values.map(String).join(' ').slice(0, 8192)}\n`);
console.log = output;
console.info = output;
console.warn = output;
console.error = output;
const input = createInterface({ input: process.stdin, crlfDelay: Infinity });

const handle = async message => {
  if (message.type === 'invokeCommand') {
    const handler = handlers.get(message.commandId);
    if (!handler) {
      send({ type: 'commandResult', callId: message.callId, ok: false, error: `no handler registered for command ${message.commandId}` });
      return;
    }
    try {
      const value = await handler(message.args);
      send({ type: 'commandResult', callId: message.callId, ok: true, value });
    } catch (error) {
      send({ type: 'commandResult', callId: message.callId, ok: false, error: String(error?.stack || error).slice(0, 8192) });
    }
    return;
  }
  if (message.type === 'deliverEvent') {
    for (const handler of eventHandlers.get(message.event) || []) {
      try { await handler(message.payload); }
      catch (error) { output(String(error?.stack || error).slice(0, 8192)); }
    }
    send({ type: 'eventAck', eventId: message.eventId });
    return;
  }
  if (message.type === 'shutdown') {
    try {
      if (globalThis.__suaegiDeactivate) await globalThis.__suaegiDeactivate();
    } catch (error) {
      output(String(error?.stack || error).slice(0, 8192));
    }
    input.close();
    process.exitCode = 0;
  }
};

input.on('line', line => {
  try {
    const message = JSON.parse(line);
    // Host results must bypass the serialized handler queue: a command or
    // event handler may currently be awaiting this exact response.
    if (message.type === 'hostResult') {
      const waiter = pending.get(message.callId);
      if (!waiter) return;
      pending.delete(message.callId);
      if (message.ok) waiter.resolve(message.value);
      else waiter.reject(Object.assign(new Error(message.error || 'host call failed'), { code: message.errorCode }));
      return;
    }
    queue = queue.then(() => handle(message)).catch(error => {
      send({ type: 'fatal', error: String(error?.stack || error).slice(0, 8192) });
      process.exitCode = 1;
      input.close();
    });
  } catch {
    output('ignoring malformed parent message');
  }
});
const api = {
  commands: {
    register(id, handler) {
      if (typeof id !== 'string' || typeof handler !== 'function') throw new Error('invalid command registration');
      handlers.set(id, handler);
    }
  },
  events: {
    on(event, handler) {
      if (typeof event !== 'string' || typeof handler !== 'function') throw new Error('invalid event registration');
      const registered = eventHandlers.get(event) || [];
      registered.push(handler);
      eventHandlers.set(event, registered);
    }
  },
  host: {
    call(method, params = {}) {
      const current = callId++;
      return new Promise((resolve, reject) => {
        pending.set(current, { resolve, reject });
        send({ type: 'hostCall', callId: current, method, params });
      });
    }
  },
  grantedCapabilities: Object.freeze([...capabilities]),
  log(message) { output(String(message)); }
};
try {
  const specifier = pathToFileURL(join(root, ...mainEntry.split(/[\\/]/))).href;
  const module = await import(specifier);
  if (typeof module.default !== 'function') throw new Error(`plugin entry ${mainEntry} has no default-exported activate function`);
  if (module.deactivate !== undefined && typeof module.deactivate !== 'function') throw new Error('plugin has a non-function deactivate export');
  globalThis.__suaegiDeactivate = module.deactivate;
  await module.default(api);
  send({ type: 'ready', commands: [...handlers.keys()] });
} catch (error) {
  send({ type: 'fatal', error: String(error?.stack || error).slice(0, 8192) });
  input.close();
  process.exitCode = 1;
}
"#;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum WorkerMessage {
    Ready {
        commands: Vec<String>,
    },
    HostCall {
        #[serde(rename = "callId")]
        call_id: u64,
        method: String,
        #[serde(default)]
        params: Value,
    },
    CommandResult {
        #[serde(rename = "callId")]
        call_id: u64,
        ok: bool,
        #[serde(default)]
        value: Value,
        #[serde(default)]
        error: Option<String>,
    },
    EventAck {
        #[serde(rename = "eventId")]
        event_id: u64,
    },
    Fatal {
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkerSpec {
    plugin_key: String,
    plugin_root: PathBuf,
    expected_content_hash: Option<String>,
    main_entry: String,
    capabilities: Vec<String>,
    data_root: PathBuf,
}

enum WorkerCommand {
    Invoke {
        command_id: String,
        response: oneshot::Sender<Result<Value, String>>,
    },
    DeliverEvent {
        event: String,
        payload: Value,
        response: oneshot::Sender<Result<(), String>>,
    },
    Shutdown,
}

#[derive(Clone)]
struct WorkerSlot {
    spec: WorkerSpec,
    sender: mpsc::UnboundedSender<WorkerCommand>,
    in_flight: Arc<AtomicUsize>,
    last_used: Instant,
    generation: u64,
}

fn worker_slots() -> &'static Mutex<HashMap<String, WorkerSlot>> {
    static SLOTS: OnceLock<Mutex<HashMap<String, WorkerSlot>>> = OnceLock::new();
    SLOTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn dynamic_subscriptions() -> &'static Mutex<HashMap<String, HashSet<String>>> {
    static SUBSCRIPTIONS: OnceLock<Mutex<HashMap<String, HashSet<String>>>> = OnceLock::new();
    SUBSCRIPTIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn allowlisted_environment(command: &mut tokio::process::Command) {
    const KEYS: &[&str] = &[
        "PATH",
        "HOME",
        "USERPROFILE",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "TZ",
        "TMPDIR",
        "TEMP",
        "TMP",
        "SYSTEMROOT",
        "SYSTEMDRIVE",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
        "PROCESSOR_ARCHITECTURE",
        "NUMBER_OF_PROCESSORS",
    ];
    command.env_clear();
    for key in KEYS {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command.env("ELECTRON_RUN_AS_NODE", "1");
}

fn node_executable() -> PathBuf {
    if let Some(configured) = std::env::var_os("SUAEGI_PLUGIN_NODE") {
        let path = PathBuf::from(configured);
        if path.is_absolute() && path.is_file() {
            return path;
        }
    }
    for candidate in ["/opt/homebrew/bin/node", "/usr/local/bin/node"] {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from("node")
}

async fn read_protocol_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> Result<Option<String>, String> {
    let mut output = Vec::new();
    loop {
        let available = reader
            .fill_buf()
            .await
            .map_err(|error| format!("Could not read plugin worker protocol: {error}"))?;
        if available.is_empty() {
            if output.is_empty() {
                return Ok(None);
            }
            break;
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        let copied = newline.unwrap_or(available.len());
        if output.len().saturating_add(copied) > MAX_PROTOCOL_LINE_BYTES {
            return Err("Plugin worker protocol line exceeds its size limit.".into());
        }
        output.extend_from_slice(&available[..copied]);
        reader.consume(consumed);
        if newline.is_some() {
            break;
        }
    }
    if output.last() == Some(&b'\r') {
        output.pop();
    }
    String::from_utf8(output)
        .map(Some)
        .map_err(|_| "Plugin worker protocol is not UTF-8.".to_string())
}

fn plugin_spec(plugin: &crate::plugins::PluginEntry) -> Option<WorkerSpec> {
    if plugin.status != crate::plugins::PluginStatus::Idle || plugin.blocked_by_kill_list.is_some()
    {
        return None;
    }
    Some(WorkerSpec {
        plugin_key: plugin.plugin_key.clone(),
        plugin_root: plugin.root.clone(),
        expected_content_hash: plugin.content_hash.clone(),
        main_entry: plugin.main_entry.clone()?,
        capabilities: plugin.capabilities.clone(),
        data_root: crate::plugins::default_plugins_data_dir(),
    })
}

fn retire_slot(plugin_key: &str, generation: u64) {
    let mut slots = worker_slots()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if slots
        .get(plugin_key)
        .is_some_and(|slot| slot.generation == generation)
    {
        slots.remove(plugin_key);
        dynamic_subscriptions()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(plugin_key);
    }
}

fn worker_slot(spec: WorkerSpec) -> Result<WorkerSlot, String> {
    static GENERATION: AtomicU64 = AtomicU64::new(1);
    let now = Instant::now();
    let mut slots = worker_slots()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let idle = slots
        .iter()
        .filter(|(_, slot)| {
            slot.in_flight.load(Ordering::Acquire) == 0
                && now.saturating_duration_since(slot.last_used) > IDLE_REAP_AFTER
        })
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    for key in idle {
        if let Some(slot) = slots.remove(&key) {
            let _ = slot.sender.unbounded_send(WorkerCommand::Shutdown);
        }
    }
    if let Some(existing) = slots.get_mut(&spec.plugin_key) {
        if existing.spec == spec {
            existing.last_used = now;
            return Ok(existing.clone());
        }
        let stale = slots
            .remove(&spec.plugin_key)
            .expect("the worker slot was just found");
        let _ = stale.sender.unbounded_send(WorkerCommand::Shutdown);
    }
    if slots.len() >= MAX_ACTIVE_WORKERS {
        let candidate = slots
            .iter()
            .filter(|(_, slot)| slot.in_flight.load(Ordering::Acquire) == 0)
            .min_by_key(|(_, slot)| slot.last_used)
            .map(|(key, _)| key.clone());
        let Some(candidate) = candidate else {
            return Err("All five plugin worker slots are busy.".into());
        };
        if let Some(slot) = slots.remove(&candidate) {
            let _ = slot.sender.unbounded_send(WorkerCommand::Shutdown);
        }
    }
    let (sender, receiver) = mpsc::unbounded();
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
    let slot = WorkerSlot {
        spec: spec.clone(),
        sender,
        in_flight: Arc::new(AtomicUsize::new(0)),
        last_used: now,
        generation,
    };
    tokio::spawn(run_worker(spec, receiver, generation));
    slots.insert(slot.spec.plugin_key.clone(), slot.clone());
    Ok(slot)
}

pub fn reap_idle() {
    let now = Instant::now();
    let mut slots = worker_slots()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let idle = slots
        .iter()
        .filter(|(_, slot)| {
            slot.in_flight.load(Ordering::Acquire) == 0
                && now.saturating_duration_since(slot.last_used) > IDLE_REAP_AFTER
        })
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    for key in idle {
        if let Some(slot) = slots.remove(&key) {
            let _ = slot.sender.unbounded_send(WorkerCommand::Shutdown);
        }
    }
}

async fn write_frame(stdin: &mut tokio::process::ChildStdin, value: &Value) -> Result<(), String> {
    let mut encoded = serde_json::to_vec(value)
        .map_err(|error| format!("Could not encode plugin worker request: {error}"))?;
    if encoded.len() > MAX_PROTOCOL_LINE_BYTES {
        return Err("Plugin worker request exceeds its size limit.".into());
    }
    encoded.push(b'\n');
    stdin
        .write_all(&encoded)
        .await
        .map_err(|error| format!("Could not write to plugin worker: {error}"))
}

async fn answer_host_call(
    spec: &WorkerSpec,
    stdin: &mut tokio::process::ChildStdin,
    call_id: u64,
    method: String,
    params: Value,
) -> Result<(), String> {
    let result = if matches!(
        method.as_str(),
        "workspace.readContext" | "terminal.sendText" | "notifications.show" | "events.subscribe"
    ) {
        let capability = crate::plugin_host::required_capability(&method)
            .ok_or_else(|| "unknown plugin host method".to_string())?;
        if !spec
            .capabilities
            .iter()
            .any(|granted| granted == capability)
        {
            Err(format!("plugin capability {capability} was not granted"))
        } else {
            invoke_app_host(&spec.plugin_key, &method, params).await
        }
    } else {
        crate::plugin_host::invoke_private(
            &spec.data_root,
            &spec.plugin_key,
            &spec.capabilities,
            &method,
            params,
        )
    };
    let response = match result {
        Ok(value) => {
            serde_json::json!({"type":"hostResult","callId":call_id,"ok":true,"value":value})
        }
        Err(error) => {
            serde_json::json!({"type":"hostResult","callId":call_id,"ok":false,"errorCode":"denied","error":error})
        }
    };
    write_frame(stdin, &response).await
}

async fn run_worker(
    spec: WorkerSpec,
    mut requests: mpsc::UnboundedReceiver<WorkerCommand>,
    generation: u64,
) {
    let result = run_worker_inner(&spec, &mut requests).await;
    if let Err(error) = result {
        eprintln!("plugin worker {} stopped: {error}", spec.plugin_key);
    }
    retire_slot(&spec.plugin_key, generation);
}

async fn run_worker_inner(
    spec: &WorkerSpec,
    requests: &mut mpsc::UnboundedReceiver<WorkerCommand>,
) -> Result<(), String> {
    crate::plugins::verify_installed_content(
        &spec.plugin_root,
        spec.expected_content_hash.as_deref(),
    )?;
    let mut command = tokio::process::Command::new(node_executable());
    command
        .args([
            "--input-type=module",
            "-e",
            WORKER_SCRIPT,
            "--",
            &spec.plugin_root.to_string_lossy(),
            &spec.main_entry,
            &serde_json::to_string(&spec.capabilities)
                .map_err(|error| format!("Could not encode plugin capabilities: {error}"))?,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    allowlisted_environment(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start the plugin worker with system Node: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Plugin worker stdout is unavailable.".to_string())?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Plugin worker stdin is unavailable.".to_string())?;
    let mut stdout = BufReader::new(stdout);
    let mut host_calls = 0usize;
    let registered = tokio::time::timeout(READY_TIMEOUT, async {
        loop {
            let line = read_protocol_line(&mut stdout)
                .await?
                .ok_or_else(|| "Plugin worker exited before becoming ready.".to_string())?;
            let message: WorkerMessage = serde_json::from_str(&line)
                .map_err(|_| "Plugin worker sent a malformed protocol message.".to_string())?;
            match message {
                WorkerMessage::Ready { commands } => {
                    if commands.len() > 256 {
                        return Err("Plugin worker registered too many commands.".into());
                    }
                    return Ok(commands.into_iter().collect::<HashSet<_>>());
                }
                WorkerMessage::HostCall {
                    call_id,
                    method,
                    params,
                } => {
                    host_calls += 1;
                    if host_calls > MAX_HOST_CALLS {
                        return Err("Plugin worker exceeded the host-call limit.".into());
                    }
                    answer_host_call(spec, &mut stdin, call_id, method, params).await?;
                }
                WorkerMessage::Fatal { error } => {
                    return Err(format!("Plugin worker failed: {error}"));
                }
                WorkerMessage::CommandResult { .. } | WorkerMessage::EventAck { .. } => {
                    return Err("Plugin worker returned a result before becoming ready.".into());
                }
            }
        }
    })
    .await
    .map_err(|_| "Plugin worker did not become ready in time.".to_string())??;

    let mut pending = HashMap::<u64, oneshot::Sender<Result<Value, String>>>::new();
    let mut pending_events = HashMap::<u64, oneshot::Sender<Result<(), String>>>::new();
    let mut next_call_id = 0_u64;
    let mut next_event_id = 0_u64;
    let outcome = loop {
        tokio::select! {
            request = requests.next() => {
                let Some(request) = request else {
                    break Ok(());
                };
                match request {
                    WorkerCommand::Invoke { command_id, response } => {
                        if !registered.contains(&command_id) {
                            let _ = response.send(Err(format!(
                                "no handler registered for command {command_id}"
                            )));
                            continue;
                        }
                        let call_id = next_call_id;
                        next_call_id = next_call_id.wrapping_add(1);
                        pending.insert(call_id, response);
                        if let Err(error) = write_frame(
                            &mut stdin,
                            &serde_json::json!({
                                "type":"invokeCommand",
                                "callId":call_id,
                                "commandId":command_id,
                                "args":null
                            }),
                        )
                        .await
                        {
                            if let Some(response) = pending.remove(&call_id) {
                                let _ = response.send(Err(error.clone()));
                            }
                            break Err(error);
                        }
                    }
                    WorkerCommand::DeliverEvent { event, payload, response } => {
                        let event_id = next_event_id;
                        next_event_id = next_event_id.wrapping_add(1);
                        pending_events.insert(event_id, response);
                        if let Err(error) = write_frame(
                            &mut stdin,
                            &serde_json::json!({
                                "type":"deliverEvent",
                                "eventId":event_id,
                                "event":event,
                                "payload":payload
                            }),
                        )
                        .await
                        {
                            if let Some(response) = pending_events.remove(&event_id) {
                                let _ = response.send(Err(error.clone()));
                            }
                            break Err(error);
                        }
                    }
                    WorkerCommand::Shutdown => break Ok(()),
                }
            }
            line = read_protocol_line(&mut stdout) => {
                let line = line?
                    .ok_or_else(|| "Plugin worker exited unexpectedly.".to_string())?;
                let message: WorkerMessage = serde_json::from_str(&line)
                    .map_err(|_| "Plugin worker sent a malformed protocol message.".to_string())?;
                match message {
                    WorkerMessage::HostCall { call_id, method, params } => {
                        host_calls += 1;
                        if host_calls > MAX_HOST_CALLS {
                            break Err("Plugin worker exceeded the host-call limit.".into());
                        }
                        answer_host_call(spec, &mut stdin, call_id, method, params).await?;
                    }
                    WorkerMessage::CommandResult { call_id, ok, value, error } => {
                        if let Some(response) = pending.remove(&call_id) {
                            let result = if ok {
                                Ok(value)
                            } else {
                                Err(error.unwrap_or_else(|| "Plugin command failed.".into()))
                            };
                            let _ = response.send(result);
                        }
                    }
                    WorkerMessage::EventAck { event_id } => {
                        if let Some(response) = pending_events.remove(&event_id) {
                            let _ = response.send(Ok(()));
                        }
                    }
                    WorkerMessage::Fatal { error } => {
                        break Err(format!("Plugin worker failed: {error}"));
                    }
                    WorkerMessage::Ready { .. } => {
                        break Err("Plugin worker sent duplicate ready.".into());
                    }
                }
            }
        }
    };
    let reason = outcome
        .as_ref()
        .err()
        .cloned()
        .unwrap_or_else(|| "Plugin worker shut down.".into());
    for (_, response) in pending {
        let _ = response.send(Err(reason.clone()));
    }
    for (_, response) in pending_events {
        let _ = response.send(Err(reason.clone()));
    }
    let _ = write_frame(&mut stdin, &serde_json::json!({"type":"shutdown"})).await;
    if tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
    }
    outcome
}

pub async fn invoke_command(
    plugin_key: String,
    plugin_root: PathBuf,
    expected_content_hash: Option<String>,
    main_entry: String,
    command_id: String,
    capabilities: Vec<String>,
    data_root: PathBuf,
) -> Result<Value, String> {
    let spec = WorkerSpec {
        plugin_key,
        plugin_root,
        expected_content_hash,
        main_entry,
        capabilities,
        data_root,
    };
    crate::plugins::verify_installed_content(
        &spec.plugin_root,
        spec.expected_content_hash.as_deref(),
    )?;
    let slot = worker_slot(spec.clone())?;
    slot.in_flight.fetch_add(1, Ordering::AcqRel);
    let (response, result) = oneshot::channel();
    if slot
        .sender
        .unbounded_send(WorkerCommand::Invoke {
            command_id,
            response,
        })
        .is_err()
    {
        slot.in_flight.fetch_sub(1, Ordering::AcqRel);
        retire_slot(&spec.plugin_key, slot.generation);
        return Err("Plugin worker is no longer running.".into());
    }
    let result = tokio::time::timeout(INVOKE_TIMEOUT, result).await;
    slot.in_flight.fetch_sub(1, Ordering::AcqRel);
    match result {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("Plugin worker stopped before returning a result.".into()),
        Err(_) => {
            let _ = slot.sender.unbounded_send(WorkerCommand::Shutdown);
            Err("Plugin command timed out.".into())
        }
    }
}

pub fn subscribe_events(plugin_key: &str, events: &[String]) -> Vec<String> {
    let mut subscriptions = dynamic_subscriptions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let subscribed = subscriptions.entry(plugin_key.to_string()).or_default();
    subscribed.extend(events.iter().cloned());
    let mut values = subscribed.iter().cloned().collect::<Vec<_>>();
    values.sort();
    values
}

fn validate_event_payload(event: &str, payload: &Value) -> Result<(), String> {
    let object = payload
        .as_object()
        .ok_or_else(|| format!("Malformed {event} plugin event payload."))?;
    let string = |name: &str, minimum: usize, maximum: usize| {
        object
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| {
                let length = value.chars().count();
                length >= minimum && length <= maximum
            })
            .is_some()
    };
    let valid = match event {
        "worktree.created" => {
            string("worktreeId", 1, 2_048)
                && string("path", 1, 32 * 1_024)
                && string("branch", 0, 1_024)
        }
        "worktree.removed" => string("worktreeId", 1, 2_048) && string("path", 1, 32 * 1_024),
        "agent.status.changed" => {
            object.get("worktreeId").is_some_and(|value| {
                value.is_null()
                    || value
                        .as_str()
                        .is_some_and(|_| string("worktreeId", 1, 2_048))
            }) && string("paneKey", 1, 2_048)
                && string("state", 1, 256)
                && object
                    .get("receivedAt")
                    .and_then(Value::as_f64)
                    .is_some_and(|value| value.is_finite() && value > 0.0)
        }
        _ => false,
    };
    valid
        .then_some(())
        .ok_or_else(|| format!("Malformed {event} plugin event payload."))
}

pub async fn deliver_event(
    plugins: Vec<crate::plugins::PluginEntry>,
    event: String,
    payload: Value,
) -> Result<usize, String> {
    validate_event_payload(&event, &payload)?;
    let dynamic = dynamic_subscriptions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let mut delivered = 0usize;
    for plugin in plugins {
        let manifest_subscribed = plugin.events.iter().any(|entry| entry.on == event);
        let dynamic_subscribed = dynamic
            .get(&plugin.plugin_key)
            .is_some_and(|events| events.contains(&event));
        if !manifest_subscribed && !dynamic_subscribed {
            continue;
        }
        let Some(spec) = plugin_spec(&plugin) else {
            continue;
        };
        let slot = worker_slot(spec)?;
        slot.in_flight.fetch_add(1, Ordering::AcqRel);
        let (response, acknowledged) = oneshot::channel();
        if slot
            .sender
            .unbounded_send(WorkerCommand::DeliverEvent {
                event: event.clone(),
                payload: payload.clone(),
                response,
            })
            .is_err()
        {
            slot.in_flight.fetch_sub(1, Ordering::AcqRel);
            return Err(format!(
                "Plugin worker {} is unavailable.",
                plugin.plugin_key
            ));
        }
        let acknowledged = tokio::time::timeout(READY_TIMEOUT, acknowledged).await;
        slot.in_flight.fetch_sub(1, Ordering::AcqRel);
        match acknowledged {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => return Err(error),
            Ok(Err(_)) => {
                return Err(format!(
                    "Plugin worker {} stopped before acknowledging an event.",
                    plugin.plugin_key
                ));
            }
            Err(_) => {
                let _ = slot.sender.unbounded_send(WorkerCommand::Shutdown);
                return Err(format!(
                    "Plugin worker {} timed out while handling an event.",
                    plugin.plugin_key
                ));
            }
        }
        delivered += 1;
    }
    Ok(delivered)
}

pub fn reconcile(plugins: &[crate::plugins::PluginEntry]) {
    let active = plugins
        .iter()
        .filter_map(plugin_spec)
        .map(|spec| (spec.plugin_key.clone(), spec))
        .collect::<HashMap<_, _>>();
    let mut slots = worker_slots()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let stale = slots
        .iter()
        .filter(|(key, slot)| active.get(*key) != Some(&slot.spec))
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    for key in stale {
        if let Some(slot) = slots.remove(&key) {
            let _ = slot.sender.unbounded_send(WorkerCommand::Shutdown);
        }
        dynamic_subscriptions()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&key);
    }
}

pub fn supported_host_method(method: &str) -> bool {
    crate::plugin_host::required_capability(method).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn worker_registers_and_invokes_a_command_with_private_host_calls() {
        if std::process::Command::new("node")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let plugin = tempfile::tempdir().unwrap();
        std::fs::write(
            plugin.path().join("main.mjs"),
            r#"
export default async function activate(suaegi) {
  const previous = await suaegi.host.call('storage.get', { key: 'activations' });
  await suaegi.host.call('storage.set', { key: 'activations', value: (previous.value || 0) + 1 });
  suaegi.events.on('worktree.created', async payload => {
    await suaegi.host.call('storage.set', { key: 'event', value: payload });
  });
  suaegi.commands.register('run', async () => {
    return await suaegi.host.call('storage.get', { key: 'activations' });
  });
  suaegi.commands.register('event', async () => {
    return await suaegi.host.call('storage.get', { key: 'event' });
  });
}
"#,
        )
        .unwrap();
        let data = tempfile::tempdir().unwrap();
        let result = invoke_command(
            "acme.worker".into(),
            plugin.path().into(),
            None,
            "main.mjs".into(),
            "run".into(),
            vec!["storage".into()],
            data.path().into(),
        )
        .await
        .unwrap();
        assert_eq!(result, serde_json::json!({"value": 1}));

        let second = invoke_command(
            "acme.worker".into(),
            plugin.path().into(),
            None,
            "main.mjs".into(),
            "run".into(),
            vec!["storage".into()],
            data.path().into(),
        )
        .await
        .unwrap();
        assert_eq!(
            second,
            serde_json::json!({"value": 1}),
            "lazy workers are reused instead of activating once per command"
        );

        let spec = WorkerSpec {
            plugin_key: "acme.worker".into(),
            plugin_root: plugin.path().into(),
            expected_content_hash: None,
            main_entry: "main.mjs".into(),
            capabilities: vec!["storage".into()],
            data_root: data.path().into(),
        };
        let (event_response, event_acknowledged) = oneshot::channel();
        worker_slot(spec)
            .unwrap()
            .sender
            .unbounded_send(WorkerCommand::DeliverEvent {
                event: "worktree.created".into(),
                payload: serde_json::json!({"branch":"feature"}),
                response: event_response,
            })
            .unwrap();
        event_acknowledged.await.unwrap().unwrap();
        let event = invoke_command(
            "acme.worker".into(),
            plugin.path().into(),
            None,
            "main.mjs".into(),
            "event".into(),
            vec!["storage".into()],
            data.path().into(),
        )
        .await
        .unwrap();
        assert_eq!(
            event,
            serde_json::json!({"value":{"branch":"feature"}}),
            "events and commands share the persistent worker's serialized runtime"
        );
        reconcile(&[]);
    }

    #[tokio::test]
    async fn protocol_reader_rejects_an_unterminated_oversized_line() {
        let bytes = vec![b'x'; MAX_PROTOCOL_LINE_BYTES + 1];
        let mut reader = BufReader::new(bytes.as_slice());
        assert!(read_protocol_line(&mut reader).await.is_err());
    }
}
