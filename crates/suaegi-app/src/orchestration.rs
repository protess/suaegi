use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunRecord {
    id: String,
    objective: String,
    created_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskRecord {
    id: String,
    run_id: String,
    spec: String,
    title: Option<String>,
    display_name: Option<String>,
    deps: Vec<String>,
    parent_id: Option<String>,
    status: String,
    result: Option<Value>,
    created_at_unix_ms: u64,
    updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageRecord {
    id: String,
    run_id: String,
    from: String,
    to: String,
    subject: String,
    body: Option<String>,
    message_type: String,
    priority: Option<String>,
    thread_id: Option<String>,
    payload: Option<Value>,
    task_id: Option<String>,
    dispatch_id: Option<String>,
    outcome: Option<String>,
    files_modified: Vec<String>,
    report_path: Option<String>,
    phase: Option<String>,
    reply_to: Option<String>,
    created_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeliveryRecord {
    id: String,
    recipient: String,
    message_ids: Vec<String>,
    created_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DispatchRecord {
    id: String,
    run_id: String,
    task_id: String,
    assignee: String,
    terminal: Option<String>,
    status: String,
    preamble: String,
    retry_of: Option<String>,
    #[serde(default)]
    environment_id: Option<String>,
    #[serde(default)]
    environment_name: Option<String>,
    #[serde(default)]
    remote_runtime_epoch: Option<String>,
    #[serde(default)]
    remote_worktree_id: Option<String>,
    #[serde(default)]
    remote_to_home_sequence: u64,
    #[serde(default)]
    remote_to_worker_relay: Vec<FederationRelayRecord>,
    #[serde(default)]
    remote_to_worker_acked: u64,
    created_at_unix_ms: u64,
    updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteAttachmentRecord {
    pub(crate) dispatch_id: String,
    pub(crate) task_id: String,
    pub(crate) home_fingerprint: String,
    pub(crate) request_id: String,
    pub(crate) payload_hash: String,
    pub(crate) protocol_version: u64,
    pub(crate) runtime_epoch: String,
    pub(crate) worktree_id: Option<String>,
    pub(crate) terminal: Option<String>,
    pub(crate) status: String,
    pub(crate) effects: Vec<Value>,
    pub(crate) residual_resources: Vec<Value>,
    pub(crate) last_error: Option<String>,
    #[serde(default)]
    pub(crate) relay_to_home: Vec<FederationRelayRecord>,
    #[serde(default)]
    pub(crate) relay_to_home_acked: u64,
    #[serde(default)]
    pub(crate) relay_to_worker_imported: u64,
    pub(crate) created_at_unix_ms: u64,
    pub(crate) updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FederationRelayRecord {
    pub(crate) dispatch_id: String,
    pub(crate) direction: String,
    pub(crate) sequence: u64,
    pub(crate) message_id: String,
    pub(crate) kind: String,
    pub(crate) payload: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GateRecord {
    id: String,
    task_id: String,
    question: String,
    options: Vec<String>,
    status: String,
    resolution: Option<String>,
    created_at_unix_ms: u64,
    resolved_at_unix_ms: Option<u64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct OrchestrationState {
    schema_version: u32,
    runs: BTreeMap<String, RunRecord>,
    tasks: BTreeMap<String, TaskRecord>,
    messages: BTreeMap<String, MessageRecord>,
    deliveries: BTreeMap<String, DeliveryRecord>,
    acked_messages: BTreeMap<String, BTreeSet<String>>,
    dispatches: BTreeMap<String, DispatchRecord>,
    remote_attachments: BTreeMap<String, RemoteAttachmentRecord>,
    gates: BTreeMap<String, GateRecord>,
    bindings: BTreeMap<String, String>,
}

pub fn run(command: &str, args: &[String]) -> Result<Value, String> {
    match command {
        "run-create" => run_create(args),
        "run-use" => run_use(args),
        "run-current" => run_current(args),
        "run-list" => {
            read_state(|state| Ok(json!({"runs": state.runs.values().collect::<Vec<_>>()})))
        }
        "run-show" => run_show(args),
        "send" => send(args),
        "check" => check(args),
        "reply" => reply(args),
        "inbox" => inbox(args),
        "task-create" => task_create(args),
        "task-list" => task_list(args),
        "task-update" => task_update(args),
        "dispatch" => dispatch(args),
        "dispatch-show" => dispatch_show(args),
        "ask" => ask(args),
        "coordinator-start" | "coordinator-stop" => Ok(json!({
            "retired": true,
            "message": "Load the current orchestration skill instead.",
            "nextAction": "suaegi skills get orchestration --full"
        })),
        "gate-create" => gate_create(args),
        "gate-resolve" => gate_resolve(args),
        "gate-list" => gate_list(args),
        "reset" => reset(args),
        "worker-start" => worker_start(args),
        "worker-show" => worker_show(args),
        "worker-read" => worker_read(args),
        "worker-stop" => worker_stop(args),
        "worker-abandon" => worker_abandon(args),
        _ => Err(format!("Unknown orchestration command: {command}")),
    }
}

fn run_create(args: &[String]) -> Result<Value, String> {
    let objective = required(args, "--objective", "orchestration run-create")?;
    if objective.trim().is_empty() {
        return Err("--objective cannot be empty.".into());
    }
    let sender = identity(args)?;
    mutate_state(|state| {
        let record = RunRecord {
            id: new_id("run")?,
            objective,
            created_at_unix_ms: now_ms(),
        };
        state.bindings.insert(sender.clone(), record.id.clone());
        state.runs.insert(record.id.clone(), record.clone());
        Ok(json!({"run": record, "boundHandle": sender}))
    })
}

fn run_use(args: &[String]) -> Result<Value, String> {
    let id = required(args, "--id", "orchestration run-use")?;
    let sender = identity(args)?;
    mutate_state(|state| {
        let run = state
            .runs
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("Orchestration run not found: {id}"))?;
        state.bindings.insert(sender.clone(), id);
        Ok(json!({"run": run, "boundHandle": sender}))
    })
}

fn run_current(args: &[String]) -> Result<Value, String> {
    let sender = identity(args)?;
    read_state(|state| {
        let id = state
            .bindings
            .get(&sender)
            .ok_or_else(|| format!("No orchestration run is bound to {sender}."))?;
        let run = state
            .runs
            .get(id)
            .ok_or_else(|| "The bound orchestration run no longer exists.".to_string())?;
        Ok(json!({"run": run, "boundHandle": sender}))
    })
}

fn run_show(args: &[String]) -> Result<Value, String> {
    sync_all_federated_best_effort();
    let id = required(args, "--id", "orchestration run-show")?;
    read_state(|state| {
        let run = state
            .runs
            .get(&id)
            .ok_or_else(|| format!("Orchestration run not found: {id}"))?;
        let tasks = state
            .tasks
            .values()
            .filter(|task| task.run_id == id)
            .collect::<Vec<_>>();
        let dispatches = state
            .dispatches
            .values()
            .filter(|dispatch| dispatch.run_id == id)
            .collect::<Vec<_>>();
        Ok(json!({"run": run, "tasks": tasks, "dispatches": dispatches}))
    })
}

fn send(args: &[String]) -> Result<Value, String> {
    let sender = identity(args)?;
    let subject = required(args, "--subject", "orchestration send")?;
    let message_type = option(args, "--type")?.unwrap_or_else(|| "message".into());
    let outcome = option(args, "--outcome")?;
    if message_type == "worker_done" && !matches!(outcome.as_deref(), Some("succeeded" | "failed"))
    {
        return Err("worker_done requires --outcome succeeded or failed.".into());
    }
    let payload = option(args, "--payload")?
        .map(|value| {
            serde_json::from_str(&value).map_err(|error| format!("--payload must be JSON: {error}"))
        })
        .transpose()?;
    if let Some(relayed) = send_from_federated_worker(
        args,
        &sender,
        &subject,
        &message_type,
        outcome.as_deref(),
        payload.as_ref(),
    )? {
        return Ok(relayed);
    }
    if let Some(relayed) =
        send_to_federated_worker(args, &sender, &subject, &message_type, payload.as_ref())?
    {
        return Ok(relayed);
    }
    mutate_state(|state| {
        let run_id = resolve_run_id(state, args, &sender)?;
        let dispatch_id = option(args, "--dispatch-id")?;
        let task_id = option(args, "--task-id")?;
        let to = option(args, "--to")?.unwrap_or_else(|| format!("run:{run_id}"));
        let record = MessageRecord {
            id: new_id("msg")?,
            run_id: run_id.clone(),
            from: sender,
            to,
            subject,
            body: option(args, "--body")?,
            message_type: message_type.clone(),
            priority: option(args, "--priority")?,
            thread_id: option(args, "--thread-id")?,
            payload,
            task_id: task_id.clone(),
            dispatch_id: dispatch_id.clone(),
            outcome: outcome.clone(),
            files_modified: option(args, "--files-modified")?
                .map(|value| split_csv(&value))
                .unwrap_or_default(),
            report_path: option(args, "--report-path")?,
            phase: option(args, "--phase")?,
            reply_to: None,
            created_at_unix_ms: now_ms(),
        };
        if message_type == "worker_done" {
            if let Some(dispatch_id) = dispatch_id {
                let dispatch = state
                    .dispatches
                    .get_mut(&dispatch_id)
                    .ok_or_else(|| format!("Dispatch not found: {dispatch_id}"))?;
                if dispatch.task_id != task_id.clone().unwrap_or_default() {
                    return Err("worker_done task and dispatch do not match.".into());
                }
                dispatch.status = outcome.clone().unwrap_or_else(|| "failed".into());
                dispatch.updated_at_unix_ms = now_ms();
            }
            if let Some(task_id) = task_id {
                let task = state
                    .tasks
                    .get_mut(&task_id)
                    .ok_or_else(|| format!("Task not found: {task_id}"))?;
                task.status = if outcome.as_deref() == Some("succeeded") {
                    "completed"
                } else {
                    "failed"
                }
                .into();
                task.updated_at_unix_ms = now_ms();
            }
        }
        state.messages.insert(record.id.clone(), record.clone());
        Ok(json!({"message": record}))
    })
}

fn send_to_federated_worker(
    args: &[String],
    sender: &str,
    subject: &str,
    message_type: &str,
    payload: Option<&Value>,
) -> Result<Option<Value>, String> {
    let Some(target) = option(args, "--to")? else {
        return Ok(None);
    };
    let Some(dispatch_id) = target.strip_prefix("dispatch:") else {
        return Ok(None);
    };
    let result = mutate_state(|state| {
        let Some(dispatch) = state.dispatches.get_mut(dispatch_id) else {
            return Ok(None);
        };
        if dispatch.environment_id.is_none() {
            return Ok(None);
        }
        if !matches!(dispatch.status.as_str(), "active" | "starting") {
            return Err(format!("Federated Dispatch {dispatch_id} is not active."));
        }
        if matches!(message_type, "worker_done" | "heartbeat") {
            return Err(
                "Coordinator-to-worker control mail cannot report worker lifecycle.".into(),
            );
        }
        let sequence = dispatch
            .remote_to_worker_relay
            .last()
            .map(|item| item.sequence)
            .unwrap_or(dispatch.remote_to_worker_acked)
            .saturating_add(1);
        let message_id = new_id("msg")?;
        let control_payload = json!({
            "from": sender,
            "subject": subject,
            "body": option(args, "--body")?.unwrap_or_default(),
            "type": message_type,
            "priority": option(args, "--priority")?.unwrap_or_else(|| "normal".into()),
            "threadId": option(args, "--thread-id")?,
            "payload": payload.map(Value::to_string),
        })
        .to_string();
        dispatch.remote_to_worker_relay.push(FederationRelayRecord {
            dispatch_id: dispatch_id.to_string(),
            direction: "to_worker".into(),
            sequence,
            message_id: message_id.clone(),
            kind: "control_message".into(),
            payload: control_payload,
        });
        dispatch.updated_at_unix_ms = now_ms();
        Ok(Some(json!({
            "message": {
                "id":message_id,
                "dispatchId":dispatch_id,
                "destination":"remote_worker",
                "accepted":true,
                "sequence":sequence,
            },
            "relay":true,
        })))
    })?;
    if result.is_some() {
        sync_federated_dispatch(dispatch_id)?;
    }
    Ok(result)
}

fn send_from_federated_worker(
    args: &[String],
    sender: &str,
    subject: &str,
    message_type: &str,
    outcome: Option<&str>,
    payload: Option<&Value>,
) -> Result<Option<Value>, String> {
    mutate_state(|state| {
        let Some(dispatch_id) = state
            .remote_attachments
            .values()
            .find(|attachment| {
                attachment.terminal.as_deref() == Some(sender) && attachment.status == "ready"
            })
            .map(|attachment| attachment.dispatch_id.clone())
        else {
            return Ok(None);
        };
        let attachment = state
            .remote_attachments
            .get_mut(&dispatch_id)
            .expect("selected above");
        if let Some(requested) = option(args, "--dispatch-id")? {
            if requested != attachment.dispatch_id {
                return Err(
                    "worker message Dispatch does not match this remote attachment.".into(),
                );
            }
        }
        if let Some(requested) = option(args, "--task-id")? {
            if requested != attachment.task_id {
                return Err("worker message Task does not match this remote attachment.".into());
            }
        }
        let message_id = new_id("msg")?;
        let lifecycle_payload = if message_type == "worker_done" {
            Some(
                json!({
                    "taskId": attachment.task_id,
                    "dispatchId": attachment.dispatch_id,
                    "outcome": outcome.unwrap_or("failed"),
                    "filesModified": option(args, "--files-modified")?
                        .map(|value| split_csv(&value))
                        .unwrap_or_default(),
                    "reportPath": option(args, "--report-path")?,
                })
                .to_string(),
            )
        } else {
            payload.map(Value::to_string)
        };
        let relay_payload = json!({
            "from": sender,
            "subject": subject,
            "body": option(args, "--body")?.unwrap_or_default(),
            "type": message_type,
            "priority": option(args, "--priority")?.unwrap_or_else(|| "normal".into()),
            "threadId": option(args, "--thread-id")?,
            "payload": lifecycle_payload,
        })
        .to_string();
        let sequence = attachment
            .relay_to_home
            .last()
            .map(|item| item.sequence)
            .unwrap_or(attachment.relay_to_home_acked)
            .saturating_add(1);
        attachment.relay_to_home.push(FederationRelayRecord {
            dispatch_id: attachment.dispatch_id.clone(),
            direction: "to_home".into(),
            sequence,
            message_id: message_id.clone(),
            kind: "message".into(),
            payload: relay_payload,
        });
        attachment.updated_at_unix_ms = now_ms();
        Ok(Some(json!({
            "message": {
                "id": message_id,
                "dispatchId": attachment.dispatch_id,
                "destination": "run_home",
                "accepted": true,
                "sequence": sequence,
            },
            "relay": true,
        })))
    })
}

fn check(args: &[String]) -> Result<Value, String> {
    sync_all_federated_best_effort();
    let recipient = match option(args, "--terminal")? {
        Some(recipient) => recipient,
        None => identity(args)?,
    };
    if let Some(delivery) = option(args, "--ack")? {
        mutate_state(|state| {
            let record = state
                .deliveries
                .remove(&delivery)
                .ok_or_else(|| format!("Delivery not found: {delivery}"))?;
            if record.recipient != recipient {
                return Err("Delivery belongs to a different recipient.".into());
            }
            state
                .acked_messages
                .entry(recipient.clone())
                .or_default()
                .extend(record.message_ids);
            Ok(Value::Null)
        })?;
    }
    let wait = has(args, "--wait");
    let timeout_ms = option_u64(args, "--timeout-ms")?
        .unwrap_or(60_000)
        .min(3_600_000);
    let started = now_ms();
    loop {
        let result = mutate_state(|state| check_once(state, args, &recipient))?;
        if result["messages"]
            .as_array()
            .is_some_and(|messages| !messages.is_empty())
            || !wait
            || now_ms().saturating_sub(started) >= timeout_ms
        {
            return Ok(result);
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn check_once(
    state: &mut OrchestrationState,
    args: &[String],
    recipient: &str,
) -> Result<Value, String> {
    if !has(args, "--peek") && !has(args, "--all") {
        if let Some(delivery) = state
            .deliveries
            .values()
            .find(|delivery| delivery.recipient == recipient)
        {
            let messages = delivery
                .message_ids
                .iter()
                .filter_map(|id| state.messages.get(id))
                .collect::<Vec<_>>();
            return Ok(json!({"deliveryId": delivery.id, "messages": messages, "replayed": true}));
        }
    }
    let run_id = option(args, "--run")?.or_else(|| state.bindings.get(recipient).cloned());
    let type_filter = option(args, "--types")?.map(|value| split_csv(&value));
    let messages = state
        .messages
        .values()
        .filter(|message| {
            run_id
                .as_deref()
                .is_none_or(|run_id| message.run_id == run_id)
        })
        .filter(|message| {
            message.to == recipient
                || message.to == "@all"
                || run_id
                    .as_deref()
                    .is_some_and(|run| message.to == format!("run:{run}"))
        })
        .filter(|message| {
            type_filter
                .as_ref()
                .is_none_or(|types| types.contains(&message.message_type))
        })
        .filter(|message| {
            has(args, "--all")
                || !state
                    .acked_messages
                    .get(recipient)
                    .is_some_and(|acked| acked.contains(&message.id))
        })
        .collect::<Vec<_>>();
    if has(args, "--all") || has(args, "--peek") {
        return Ok(json!({"messages": messages, "peek": has(args, "--peek")}));
    }
    let message_ids = messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    let delivery_id = if message_ids.is_empty() {
        None
    } else {
        let id = new_id("delivery")?;
        state.deliveries.insert(
            id.clone(),
            DeliveryRecord {
                id: id.clone(),
                recipient: recipient.into(),
                message_ids,
                created_at_unix_ms: now_ms(),
            },
        );
        Some(id)
    };
    Ok(json!({"deliveryId": delivery_id, "messages": messages, "replayed": false}))
}

fn reply(args: &[String]) -> Result<Value, String> {
    let original_id = required(args, "--id", "orchestration reply")?;
    let body = required(args, "--body", "orchestration reply")?;
    let sender = identity(args)?;
    let result = mutate_state(|state| {
        let original = state
            .messages
            .get(&original_id)
            .cloned()
            .ok_or_else(|| format!("Message not found: {original_id}"))?;
        let record = MessageRecord {
            id: new_id("msg")?,
            run_id: original.run_id.clone(),
            from: sender,
            to: original.from.clone(),
            subject: format!("Re: {}", original.subject),
            body: Some(body),
            message_type: "reply".into(),
            priority: original.priority.clone(),
            thread_id: original
                .thread_id
                .clone()
                .or_else(|| Some(original.id.clone())),
            payload: None,
            task_id: original.task_id.clone(),
            dispatch_id: original.dispatch_id.clone(),
            outcome: None,
            files_modified: Vec::new(),
            report_path: None,
            phase: None,
            reply_to: Some(original.id.clone()),
            created_at_unix_ms: now_ms(),
        };
        if let Some(dispatch_id) = original.dispatch_id.as_deref() {
            if let Some(dispatch) = state
                .dispatches
                .get_mut(dispatch_id)
                .filter(|dispatch| dispatch.environment_id.is_some())
            {
                let sequence = dispatch
                    .remote_to_worker_relay
                    .last()
                    .map(|item| item.sequence)
                    .unwrap_or(dispatch.remote_to_worker_acked)
                    .saturating_add(1);
                dispatch.remote_to_worker_relay.push(FederationRelayRecord {
                    dispatch_id: dispatch_id.to_string(),
                    direction: "to_worker".into(),
                    sequence,
                    message_id: record.id.clone(),
                    kind: "control_message".into(),
                    payload: json!({
                        "from":record.from,
                        "subject":record.subject,
                        "body":record.body,
                        "type":"reply",
                        "priority":record.priority.as_deref().unwrap_or("normal"),
                        "threadId":record.thread_id,
                        "payload":Value::Null,
                        "replyTo":original.id,
                    })
                    .to_string(),
                });
                dispatch.updated_at_unix_ms = now_ms();
            }
        }
        state.messages.insert(record.id.clone(), record.clone());
        Ok(json!({
            "message": record,
            "relay": original.dispatch_id.as_deref().is_some_and(|id| {
                state.dispatches.get(id).is_some_and(|dispatch| dispatch.environment_id.is_some())
            }),
            "relayDispatchId": original.dispatch_id,
        }))
    })?;
    if let Some(dispatch_id) = result["relayDispatchId"].as_str() {
        sync_federated_dispatch(dispatch_id)?;
    }
    Ok(result)
}

fn inbox(args: &[String]) -> Result<Value, String> {
    sync_all_federated_best_effort();
    let limit = option_u64(args, "--limit")?.unwrap_or(50).clamp(1, 1_000) as usize;
    let terminal = option(args, "--terminal")?;
    read_state(|state| {
        let mut messages = state
            .messages
            .values()
            .filter(|message| terminal.as_deref().is_none_or(|to| message.to == to))
            .collect::<Vec<_>>();
        messages.sort_by_key(|message| std::cmp::Reverse(message.created_at_unix_ms));
        messages.truncate(limit);
        Ok(json!({"messages": messages}))
    })
}

fn task_create(args: &[String]) -> Result<Value, String> {
    let spec = required(args, "--spec", "orchestration task-create")?;
    let sender = identity(args)?;
    let deps = option(args, "--deps")?
        .map(|value| {
            serde_json::from_str::<Vec<String>>(&value)
                .map_err(|error| format!("--deps must be a JSON string array: {error}"))
        })
        .transpose()?
        .unwrap_or_default();
    mutate_state(|state| {
        let run_id = resolve_run_id(state, args, &sender)?;
        for dep in &deps {
            if !state.tasks.contains_key(dep) {
                return Err(format!("Dependency task not found: {dep}"));
            }
        }
        let now = now_ms();
        let task = TaskRecord {
            id: new_id("task")?,
            run_id,
            spec,
            title: option(args, "--task-title")?,
            display_name: option(args, "--display-name")?,
            deps,
            parent_id: option(args, "--parent")?,
            status: "pending".into(),
            result: None,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
        };
        let id = task.id.clone();
        state.tasks.insert(id.clone(), task);
        refresh_ready_tasks(state);
        Ok(json!({"task": state.tasks.get(&id)}))
    })
}

fn task_list(args: &[String]) -> Result<Value, String> {
    let sender = identity(args)?;
    read_state(|state| {
        let run_id = resolve_run_id(state, args, &sender)?;
        let status = option(args, "--status")?;
        let ready = has(args, "--ready");
        let tasks = state
            .tasks
            .values()
            .filter(|task| task.run_id == run_id)
            .filter(|task| status.as_deref().is_none_or(|status| task.status == status))
            .filter(|task| !ready || task.status == "ready")
            .collect::<Vec<_>>();
        Ok(json!({"runId": run_id, "tasks": tasks}))
    })
}

fn task_update(args: &[String]) -> Result<Value, String> {
    let id = required(args, "--id", "orchestration task-update")?;
    let status = required(args, "--status", "orchestration task-update")?;
    if !matches!(
        status.as_str(),
        "pending" | "ready" | "dispatched" | "completed" | "failed" | "blocked"
    ) {
        return Err("Invalid task status.".into());
    }
    let result = option(args, "--result")?
        .map(|value| {
            serde_json::from_str(&value).map_err(|error| format!("--result must be JSON: {error}"))
        })
        .transpose()?;
    mutate_state(|state| {
        let task = state
            .tasks
            .get_mut(&id)
            .ok_or_else(|| format!("Task not found: {id}"))?;
        task.status = status;
        task.result = result;
        task.updated_at_unix_ms = now_ms();
        let task = task.clone();
        refresh_ready_tasks(state);
        Ok(json!({"task": task}))
    })
}

fn dispatch(args: &[String]) -> Result<Value, String> {
    let task_id = required(args, "--task", "orchestration dispatch")?;
    let to = required(args, "--to", "orchestration dispatch")?;
    let dry_run = has(args, "--dry-run");
    let inject = has(args, "--inject");
    let sender = identity(args)?;
    let (record, task) = mutate_state(|state| {
        let task = state
            .tasks
            .get(&task_id)
            .cloned()
            .ok_or_else(|| format!("Task not found: {task_id}"))?;
        let run_id = resolve_run_id(state, args, &sender)?;
        if task.run_id != run_id {
            return Err("Task belongs to a different orchestration run.".into());
        }
        let id = new_id("dispatch")?;
        let preamble = dispatch_preamble(&id, &task, &run_id, &to);
        let now = now_ms();
        let record = DispatchRecord {
            id,
            run_id,
            task_id: task.id.clone(),
            assignee: to.clone(),
            terminal: Some(to.clone()),
            status: if dry_run { "dry_run" } else { "active" }.into(),
            preamble,
            retry_of: option(args, "--retry-of")?,
            environment_id: None,
            environment_name: None,
            remote_runtime_epoch: None,
            remote_worktree_id: None,
            remote_to_home_sequence: 0,
            remote_to_worker_relay: Vec::new(),
            remote_to_worker_acked: 0,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
        };
        if !dry_run {
            state.dispatches.insert(record.id.clone(), record.clone());
            if let Some(task) = state.tasks.get_mut(&task_id) {
                task.status = "dispatched".into();
                task.updated_at_unix_ms = now;
            }
        }
        Ok((record, task))
    })?;
    if inject && !dry_run {
        rpc_required(
            "terminal.send",
            json!({"terminal": to, "text": record.preamble, "enter": true}),
        )?;
    }
    Ok(json!({
        "dispatch": record,
        "task": task,
        "injected": inject && !dry_run,
        "dryRun": dry_run
    }))
}

fn dispatch_show(args: &[String]) -> Result<Value, String> {
    let task_id = required(args, "--task", "orchestration dispatch-show")?;
    read_state(|state| {
        let dispatch = state
            .dispatches
            .values()
            .filter(|dispatch| dispatch.task_id == task_id)
            .max_by_key(|dispatch| dispatch.created_at_unix_ms)
            .ok_or_else(|| format!("No dispatch exists for task: {task_id}"))?;
        Ok(
            json!({"dispatch": dispatch, "preamble": has(args, "--preamble").then_some(&dispatch.preamble)}),
        )
    })
}

fn ask(args: &[String]) -> Result<Value, String> {
    let sender = identity(args)?;
    let timeout_ms = option_u64(args, "--timeout-ms")?
        .unwrap_or(900_000)
        .min(3_600_000);
    let question_id = if let Some(resume) = option(args, "--resume")? {
        resume
    } else {
        let question = required(args, "--question", "orchestration ask")?;
        if let Some(id) = create_federated_question(args, &sender, &question)? {
            id
        } else {
            let message = mutate_state(|state| {
                let run_id = resolve_run_id(state, args, &sender)?;
                let record = MessageRecord {
                    id: new_id("msg")?,
                    run_id: run_id.clone(),
                    from: sender.clone(),
                    to: option(args, "--to")?.unwrap_or_else(|| format!("run:{run_id}")),
                    subject: question.clone(),
                    body: None,
                    message_type: "question".into(),
                    priority: None,
                    thread_id: None,
                    payload: option(args, "--options")?
                        .map(|value| json!({"options": split_csv(&value)})),
                    task_id: None,
                    dispatch_id: None,
                    outcome: None,
                    files_modified: Vec::new(),
                    report_path: None,
                    phase: None,
                    reply_to: None,
                    created_at_unix_ms: now_ms(),
                };
                state.messages.insert(record.id.clone(), record.clone());
                Ok(record)
            })?;
            message.id
        }
    };
    let started = now_ms();
    loop {
        let reply = read_state(|state| {
            Ok(state
                .messages
                .values()
                .find(|message| message.reply_to.as_deref() == Some(question_id.as_str()))
                .cloned())
        })?;
        if let Some(reply) = reply {
            return Ok(json!({"questionId": question_id, "reply": reply}));
        }
        if now_ms().saturating_sub(started) >= timeout_ms {
            return Ok(json!({"questionId": question_id, "pending": true, "timedOut": true}));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn create_federated_question(
    args: &[String],
    sender: &str,
    question: &str,
) -> Result<Option<String>, String> {
    mutate_state(|state| {
        let Some(dispatch_id) = state
            .remote_attachments
            .values()
            .find(|attachment| {
                attachment.terminal.as_deref() == Some(sender) && attachment.status == "ready"
            })
            .map(|attachment| attachment.dispatch_id.clone())
        else {
            return Ok(None);
        };
        let message_id = new_id("msg")?;
        let options = option(args, "--options")?.map(|value| json!({"options":split_csv(&value)}));
        let attachment = state
            .remote_attachments
            .get_mut(&dispatch_id)
            .expect("selected above");
        let sequence = attachment
            .relay_to_home
            .last()
            .map(|item| item.sequence)
            .unwrap_or(attachment.relay_to_home_acked)
            .saturating_add(1);
        attachment.relay_to_home.push(FederationRelayRecord {
            dispatch_id: dispatch_id.clone(),
            direction: "to_home".into(),
            sequence,
            message_id: message_id.clone(),
            kind: "message".into(),
            payload: json!({
                "from":sender,
                "subject":question,
                "body":"",
                "type":"question",
                "priority":"normal",
                "threadId":message_id,
                "payload":options.as_ref().map(Value::to_string),
            })
            .to_string(),
        });
        attachment.updated_at_unix_ms = now_ms();
        state.messages.insert(
            message_id.clone(),
            MessageRecord {
                id: message_id.clone(),
                run_id: format!("remote:{dispatch_id}"),
                from: sender.to_string(),
                to: "run-home".into(),
                subject: question.to_string(),
                body: None,
                message_type: "question".into(),
                priority: None,
                thread_id: Some(message_id.clone()),
                payload: options,
                task_id: Some(attachment.task_id.clone()),
                dispatch_id: Some(dispatch_id),
                outcome: None,
                files_modified: Vec::new(),
                report_path: None,
                phase: None,
                reply_to: None,
                created_at_unix_ms: now_ms(),
            },
        );
        Ok(Some(message_id))
    })
}

fn gate_create(args: &[String]) -> Result<Value, String> {
    let task_id = required(args, "--task", "orchestration gate-create")?;
    let question = required(args, "--question", "orchestration gate-create")?;
    let options = option(args, "--options")?
        .map(|value| {
            serde_json::from_str::<Vec<String>>(&value)
                .map_err(|error| format!("--options must be a JSON string array: {error}"))
        })
        .transpose()?
        .unwrap_or_default();
    mutate_state(|state| {
        let task = state
            .tasks
            .get_mut(&task_id)
            .ok_or_else(|| format!("Task not found: {task_id}"))?;
        task.status = "blocked".into();
        task.updated_at_unix_ms = now_ms();
        let gate = GateRecord {
            id: new_id("gate")?,
            task_id,
            question,
            options,
            status: "pending".into(),
            resolution: None,
            created_at_unix_ms: now_ms(),
            resolved_at_unix_ms: None,
        };
        state.gates.insert(gate.id.clone(), gate.clone());
        Ok(json!({"gate": gate}))
    })
}

fn gate_resolve(args: &[String]) -> Result<Value, String> {
    let id = required(args, "--id", "orchestration gate-resolve")?;
    let resolution = required(args, "--resolution", "orchestration gate-resolve")?;
    mutate_state(|state| {
        let gate = state
            .gates
            .get_mut(&id)
            .ok_or_else(|| format!("Gate not found: {id}"))?;
        gate.status = "resolved".into();
        gate.resolution = Some(resolution);
        gate.resolved_at_unix_ms = Some(now_ms());
        let task_id = gate.task_id.clone();
        let gate = gate.clone();
        if let Some(task) = state.tasks.get_mut(&task_id) {
            task.status = "pending".into();
            task.updated_at_unix_ms = now_ms();
        }
        refresh_ready_tasks(state);
        Ok(json!({"gate": gate}))
    })
}

fn gate_list(args: &[String]) -> Result<Value, String> {
    let task_id = option(args, "--task")?;
    let status = option(args, "--status")?;
    read_state(|state| {
        let gates = state
            .gates
            .values()
            .filter(|gate| task_id.as_deref().is_none_or(|id| gate.task_id == id))
            .filter(|gate| status.as_deref().is_none_or(|status| gate.status == status))
            .collect::<Vec<_>>();
        Ok(json!({"gates": gates}))
    })
}

fn reset(args: &[String]) -> Result<Value, String> {
    let scopes = [
        has(args, "--all"),
        has(args, "--tasks"),
        has(args, "--messages"),
    ];
    if scopes.into_iter().filter(|selected| *selected).count() != 1 {
        return Err(
            "orchestration reset requires exactly one of --all, --tasks, or --messages.".into(),
        );
    }
    mutate_state(|state| {
        let mut removed = BTreeMap::new();
        if has(args, "--all") || has(args, "--tasks") {
            removed.insert("tasks", state.tasks.len());
            removed.insert("dispatches", state.dispatches.len());
            removed.insert("gates", state.gates.len());
            state.tasks.clear();
            state.dispatches.clear();
            state.gates.clear();
        }
        if has(args, "--all") || has(args, "--messages") {
            removed.insert("messages", state.messages.len());
            removed.insert("deliveries", state.deliveries.len());
            state.messages.clear();
            state.deliveries.clear();
            state.acked_messages.clear();
        }
        if has(args, "--all") {
            removed.insert("runs", state.runs.len());
            state.runs.clear();
            state.bindings.clear();
        }
        Ok(json!({"reset": true, "removed": removed}))
    })
}

pub(crate) fn begin_federation_attachment(
    dispatch_id: &str,
    task_id: &str,
    home_fingerprint: &str,
    request_id: &str,
    payload_hash: &str,
    protocol_version: u64,
    runtime_epoch: &str,
) -> Result<Option<Value>, String> {
    mutate_state(|state| {
        if let Some(existing) = state.remote_attachments.get(dispatch_id) {
            if existing.home_fingerprint != home_fingerprint {
                return Err(format!(
                    "Remote Dispatch {dispatch_id} was not found for this Run home."
                ));
            }
            if existing.request_id != request_id
                || existing.payload_hash != payload_hash
                || existing.task_id != task_id
            {
                return Err(format!(
                    "Federated mutation {request_id} was already used with different input."
                ));
            }
            if existing.status == "starting" {
                return Err(format!(
                    "Federated worker {dispatch_id} was accepted before restart; inspect the Dispatch before retrying."
                ));
            }
            return Ok(Some(federation_attachment_receipt(existing, true)));
        }
        let now = now_ms();
        state.remote_attachments.insert(
            dispatch_id.to_string(),
            RemoteAttachmentRecord {
                dispatch_id: dispatch_id.to_string(),
                task_id: task_id.to_string(),
                home_fingerprint: home_fingerprint.to_string(),
                request_id: request_id.to_string(),
                payload_hash: payload_hash.to_string(),
                protocol_version,
                runtime_epoch: runtime_epoch.to_string(),
                worktree_id: None,
                terminal: None,
                status: "starting".into(),
                effects: Vec::new(),
                residual_resources: Vec::new(),
                last_error: None,
                relay_to_home: Vec::new(),
                relay_to_home_acked: 0,
                relay_to_worker_imported: 0,
                created_at_unix_ms: now,
                updated_at_unix_ms: now,
            },
        );
        Ok(None)
    })
}

pub(crate) fn update_federation_attachment(
    dispatch_id: &str,
    worktree_id: Option<String>,
    terminal: Option<String>,
    status: &str,
    effects: Vec<Value>,
    residual_resources: Vec<Value>,
    last_error: Option<String>,
) -> Result<Value, String> {
    mutate_state(|state| {
        let attachment = state
            .remote_attachments
            .get_mut(dispatch_id)
            .ok_or_else(|| format!("Remote Dispatch not found: {dispatch_id}"))?;
        if worktree_id.is_some() {
            attachment.worktree_id = worktree_id;
        }
        if terminal.is_some() {
            attachment.terminal = terminal;
        }
        attachment.status = status.to_string();
        attachment.effects = effects;
        attachment.residual_resources = residual_resources;
        attachment.last_error = last_error;
        attachment.updated_at_unix_ms = now_ms();
        Ok(federation_attachment_receipt(attachment, false))
    })
}

pub(crate) fn federation_attachment(
    dispatch_id: &str,
    home_fingerprint: &str,
) -> Result<RemoteAttachmentRecord, String> {
    read_state(|state| {
        state
            .remote_attachments
            .get(dispatch_id)
            .filter(|attachment| attachment.home_fingerprint == home_fingerprint)
            .cloned()
            .ok_or_else(|| {
                format!("Remote Dispatch {dispatch_id} was not found for this Run home.")
            })
    })
}

pub(crate) fn pull_federation_relay(
    dispatch_id: &str,
    home_fingerprint: &str,
    after_sequence: u64,
    limit: usize,
) -> Result<Vec<FederationRelayRecord>, String> {
    let attachment = federation_attachment(dispatch_id, home_fingerprint)?;
    Ok(attachment
        .relay_to_home
        .into_iter()
        .filter(|item| item.sequence > after_sequence)
        .take(limit.clamp(1, 100))
        .collect())
}

pub(crate) fn acknowledge_federation_relay(
    dispatch_id: &str,
    home_fingerprint: &str,
    through_sequence: u64,
) -> Result<(), String> {
    mutate_state(|state| {
        let attachment = state
            .remote_attachments
            .get_mut(dispatch_id)
            .filter(|attachment| attachment.home_fingerprint == home_fingerprint)
            .ok_or_else(|| {
                format!("Remote Dispatch {dispatch_id} was not found for this Run home.")
            })?;
        let maximum = attachment
            .relay_to_home
            .last()
            .map(|item| item.sequence)
            .unwrap_or(attachment.relay_to_home_acked);
        if through_sequence > maximum {
            return Err(format!(
                "Cannot acknowledge relay sequence {through_sequence}; latest is {maximum}."
            ));
        }
        attachment.relay_to_home_acked = attachment.relay_to_home_acked.max(through_sequence);
        attachment
            .relay_to_home
            .retain(|item| item.sequence > attachment.relay_to_home_acked);
        attachment.updated_at_unix_ms = now_ms();
        Ok(())
    })
}

pub(crate) fn import_federation_relay(
    dispatch_id: &str,
    home_fingerprint: &str,
    items: &[Value],
) -> Result<Value, String> {
    mutate_state(|state| {
        let attachment = state
            .remote_attachments
            .get(dispatch_id)
            .filter(|attachment| attachment.home_fingerprint == home_fingerprint)
            .cloned()
            .ok_or_else(|| {
                format!("Remote Dispatch {dispatch_id} was not found for this Run home.")
            })?;
        if attachment.status != "ready" {
            return Err(format!("Remote Dispatch {dispatch_id} is not active."));
        }
        let terminal = attachment
            .terminal
            .as_deref()
            .ok_or_else(|| format!("Remote Dispatch {dispatch_id} has no terminal."))?;
        let mut cursor = attachment.relay_to_worker_imported;
        let mut imported = 0_u64;
        for item in items {
            let item_dispatch = item["dispatch_id"]
                .as_str()
                .ok_or_else(|| "Federation import item has no Dispatch id.".to_string())?;
            let sequence = item["sequence"]
                .as_u64()
                .ok_or_else(|| "Federation import item has no sequence.".to_string())?;
            if item_dispatch != dispatch_id || sequence > cursor.saturating_add(1) {
                return Err(format!(
                    "Home relay for {dispatch_id} is not contiguous after sequence {cursor}."
                ));
            }
            if sequence <= cursor {
                continue;
            }
            if item["direction"].as_str() != Some("to_worker")
                || item["kind"].as_str() != Some("control_message")
            {
                return Err("Federation import item is not supported control mail.".into());
            }
            let message_id = item["message_id"]
                .as_str()
                .ok_or_else(|| "Federation import item has no message id.".to_string())?
                .to_string();
            let payload: Value = serde_json::from_str(
                item["payload"]
                    .as_str()
                    .ok_or_else(|| "Federation import item has no payload.".to_string())?,
            )
            .map_err(|_| "Federation control payload is invalid JSON.".to_string())?;
            if !state.messages.contains_key(&message_id) {
                state.messages.insert(
                    message_id.clone(),
                    MessageRecord {
                        id: message_id,
                        run_id: format!("remote:{dispatch_id}"),
                        from: payload["from"].as_str().unwrap_or("run-home").to_string(),
                        to: terminal.to_string(),
                        subject: payload["subject"].as_str().unwrap_or("").to_string(),
                        body: payload["body"].as_str().map(str::to_string),
                        message_type: payload["type"].as_str().unwrap_or("status").to_string(),
                        priority: payload["priority"].as_str().map(str::to_string),
                        thread_id: payload["threadId"].as_str().map(str::to_string),
                        payload: payload["payload"]
                            .as_str()
                            .and_then(|value| serde_json::from_str(value).ok()),
                        task_id: Some(attachment.task_id.clone()),
                        dispatch_id: Some(dispatch_id.to_string()),
                        outcome: None,
                        files_modified: Vec::new(),
                        report_path: None,
                        phase: None,
                        reply_to: payload["replyTo"].as_str().map(str::to_string),
                        created_at_unix_ms: now_ms(),
                    },
                );
                imported = imported.saturating_add(1);
            }
            cursor = sequence;
        }
        let current = state
            .remote_attachments
            .get_mut(dispatch_id)
            .expect("validated above");
        current.relay_to_worker_imported = cursor;
        current.updated_at_unix_ms = now_ms();
        Ok(json!({
            "dispatchId":dispatch_id,
            "acknowledgedThrough":cursor,
            "imported":imported,
        }))
    })
}

fn federation_attachment_receipt(attachment: &RemoteAttachmentRecord, replayed: bool) -> Value {
    json!({
        "dispatchId": attachment.dispatch_id,
        "state": attachment.status,
        "stage": match attachment.status.as_str() {
            "ready" => "input_accepted",
            "stopped" => "stopped",
            "starting" => "accepted",
            _ => "remote_attach",
        },
        "runtimeEpoch": attachment.runtime_epoch,
        "worktreeId": attachment.worktree_id,
        "terminalHandle": attachment.terminal,
        "effects": attachment.effects,
        "residualResources": attachment.residual_resources,
        "lastError": attachment.last_error,
        "mutation": {"requestId": attachment.request_id, "replayed": replayed},
    })
}

fn worker_start(args: &[String]) -> Result<Value, String> {
    let task_id = required(args, "--task", "orchestration worker-start")?;
    let sender = identity(args)?;
    let (run_id, task) = read_state(|state| {
        let run_id = resolve_run_id(state, args, &sender)?;
        let task = state
            .tasks
            .get(&task_id)
            .ok_or_else(|| format!("Task not found: {task_id}"))?;
        if task.run_id != run_id {
            return Err("Task belongs to a different orchestration run.".into());
        }
        Ok((run_id, task.clone()))
    })?;
    if let Some(environment) = option(args, "--on")? {
        return worker_start_remote(args, &run_id, &task, &environment);
    }
    let existing = option(args, "--terminal")?;
    let agent = option(args, "--agent")?;
    if existing.is_some() == agent.is_some() {
        return Err("worker-start requires exactly one of --agent or --terminal.".into());
    }
    let requested_worktree = option(args, "--worktree")?.unwrap_or_else(|| "current".into());
    let creates_worktree = matches!(requested_worktree.as_str(), "new-child" | "new-top-level");
    if creates_worktree && existing.is_some() {
        return Err("--terminal cannot combine with new-worktree creation.".into());
    }
    let creation_flags = ["--name", "--repo", "--base-branch", "--setup", "--comment"];
    if !creates_worktree && creation_flags.iter().any(|flag| has(args, flag)) {
        return Err(
            "Creation and setup options apply only to new-child or new-top-level worktrees.".into(),
        );
    }
    let timeout_ms = option_u64(args, "--timeout-ms")?
        .unwrap_or(60_000)
        .clamp(1, 3_600_000);
    let coordinator_terminal =
        rpc_required("terminal.show", json!({"terminal": sender, "limit": 1}))?;
    let coordinator_worktree = coordinator_terminal["terminal"]["worktreeId"]
        .as_str()
        .ok_or_else(|| "The coordinator terminal has no managed worktree.".to_string())?
        .to_string();
    let mut effects = Vec::new();
    let (worktree, terminal) = if creates_worktree {
        let name = required(args, "--name", "orchestration worker-start")?;
        let setup = option(args, "--setup")?.unwrap_or_else(|| "run".into());
        if !matches!(setup.as_str(), "run" | "skip" | "inherit") {
            return Err("--setup must be run, skip, or inherit.".into());
        }
        let selected_agent = agent
            .as_deref()
            .ok_or_else(|| "New worktrees require --agent.".to_string())?;
        let mut params = serde_json::Map::new();
        params.insert("name".into(), Value::String(name));
        params.insert("agent".into(), Value::String(selected_agent.to_string()));
        params.insert("runSetup".into(), Value::Bool(setup == "run"));
        if let Some(repo) = option(args, "--repo")? {
            params.insert("repo".into(), Value::String(repo));
        } else {
            let current =
                rpc_required("worktree.show", json!({"worktree": &coordinator_worktree}))?;
            if let Some(repo) = current["worktree"]["repoId"].as_str() {
                params.insert("repo".into(), Value::String(repo.to_string()));
            }
        }
        if let Some(base) = option(args, "--base-branch")? {
            params.insert("baseBranch".into(), Value::String(base));
        }
        let created = rpc_required("worktree.create", Value::Object(params))?;
        let worktree = created["path"]
            .as_str()
            .ok_or_else(|| "Worktree creation returned no path.".to_string())?
            .to_string();
        wait_for_worktree(&worktree, timeout_ms)?;
        let mut metadata = serde_json::Map::new();
        metadata.insert("worktree".into(), Value::String(worktree.clone()));
        if requested_worktree == "new-top-level" {
            metadata.insert("noParent".into(), Value::Bool(true));
        } else {
            metadata.insert(
                "parentWorktree".into(),
                Value::String(coordinator_worktree.clone()),
            );
        }
        if let Some(display_name) = option(args, "--display-name")? {
            metadata.insert("displayName".into(), Value::String(display_name));
        }
        if let Some(comment) = option(args, "--comment")? {
            metadata.insert("comment".into(), Value::String(comment));
        }
        rpc_required("worktree.set", Value::Object(metadata))?;
        effects.push(json!({
            "kind": "worktree",
            "action": "created",
            "id": worktree,
            "placement": requested_worktree,
        }));
        effects.push(json!({
            "kind": "setup",
            "action": if setup == "run" { "started" } else { "skipped" },
            "state": if setup == "run" { "running" } else { "not_applicable" },
        }));
        let terminal = wait_for_agent_terminal(&worktree, selected_agent, timeout_ms)?;
        (worktree, terminal)
    } else {
        let worktree = if requested_worktree == "current" {
            coordinator_worktree
        } else {
            let shown = rpc_required("worktree.show", json!({"worktree": &requested_worktree}))?;
            shown["worktree"]["id"]
                .as_str()
                .or_else(|| shown["worktree"]["path"].as_str())
                .ok_or_else(|| "Worktree lookup returned no id.".to_string())?
                .to_string()
        };
        effects.push(json!({"kind": "worktree", "action": "reused", "id": worktree}));
        let terminal = if let Some(terminal) = existing {
            let shown = rpc_required("terminal.show", json!({"terminal": &terminal}))?;
            if shown["terminal"]["worktreeId"].as_str() != Some(worktree.as_str()) {
                return Err(format!(
                    "Terminal {terminal} does not belong to worktree {worktree}."
                ));
            }
            if shown["terminal"]["agent"].is_null() {
                return Err(format!(
                    "Terminal {terminal} is not running a recognized agent."
                ));
            }
            effects.push(
                json!({"kind": "terminal", "role": "agent", "action": "reused", "id": terminal}),
            );
            terminal
        } else {
            let agent = agent.as_deref().expect("validated above");
            let result = rpc_required(
                "terminal.create",
                json!({
                    "worktree": &worktree,
                    "command": agent,
                    "title": option(args, "--display-name")?.or(option(args, "--name")?),
                    "focus": false
                }),
            )?;
            let terminal = result["terminal"]["handle"]
                .as_str()
                .ok_or_else(|| "Terminal creation returned no handle.".to_string())?
                .to_string();
            effects.push(
                json!({"kind": "terminal", "role": "agent", "action": "created", "id": terminal}),
            );
            terminal
        };
        (worktree, terminal)
    };
    let readiness = rpc_required(
        "terminal.wait",
        json!({"terminal": &terminal, "for": "tui-idle", "timeoutMs": timeout_ms}),
    )?;
    if readiness["wait"]["satisfied"].as_bool() != Some(true) {
        return Err(format!(
            "Agent terminal {terminal} did not become ready within {timeout_ms} ms."
        ));
    }
    if !effects.iter().any(|effect| {
        effect["kind"].as_str() == Some("terminal")
            && effect["id"].as_str() == Some(terminal.as_str())
    }) {
        effects.push(
            json!({"kind": "terminal", "role": "agent", "action": "created", "id": terminal}),
        );
    }
    let mut dispatch_args = vec![
        "--task".into(),
        task_id,
        "--to".into(),
        terminal.clone(),
        "--inject".into(),
    ];
    if let Some(run) = option(args, "--run")? {
        dispatch_args.extend(["--run".into(), run]);
    }
    if let Some(from) = option(args, "--from")? {
        dispatch_args.extend(["--from".into(), from]);
    }
    if let Some(retry) = option(args, "--retry-of")? {
        dispatch_args.extend(["--retry-of".into(), retry]);
    }
    let result = dispatch(&dispatch_args)?;
    Ok(json!({
        "ready": true,
        "state": "ready",
        "taskId": result["task"]["id"],
        "dispatchId": result["dispatch"]["id"],
        "worktreeId": worktree,
        "agentTerminalHandle": terminal,
        "effects": effects,
        "residualResources": [],
        "result": result
    }))
}

fn worker_start_remote(
    args: &[String],
    run_id: &str,
    task: &TaskRecord,
    environment_selector: &str,
) -> Result<Value, String> {
    let worktree = option(args, "--worktree")?.unwrap_or_else(|| "current".into());
    if matches!(worktree.as_str(), "current" | "new-child") {
        return Err("--on requires an exact remote worktree selector or new-top-level.".into());
    }
    let creates_worktree = worktree == "new-top-level";
    let terminal = option(args, "--terminal")?;
    let agent = option(args, "--agent")?;
    if creates_worktree {
        if option(args, "--name")?.is_none() || option(args, "--repo")?.is_none() {
            return Err(
                "Remote new-top-level requires --name and an explicit --repo from remote discovery."
                    .into(),
            );
        }
        if terminal.is_some() {
            return Err("--terminal cannot combine with remote new-worktree creation.".into());
        }
    } else if ["--name", "--repo", "--base-branch", "--setup"]
        .iter()
        .any(|flag| has(args, flag))
    {
        return Err(
            "Creation and setup options apply only to remote new-top-level worktrees.".into(),
        );
    }
    if terminal.is_some() && agent.is_some() {
        return Err("--terminal cannot combine with --agent.".into());
    }
    if terminal.is_none()
        && agent
            .as_deref()
            .and_then(suaegi_term::agent::agent_def_by_id)
            .is_none()
    {
        return Err(
            "A configured --agent is required when remote worker-start creates a terminal.".into(),
        );
    }
    let timeout_ms = option_u64(args, "--timeout-ms")?
        .unwrap_or(60_000)
        .clamp(1, 3_600_000);
    let persisted =
        suaegi_core::persistence::Store::new(crate::persistence_thread::default_data_file())
            .load()
            .state;
    let environment = persisted
        .settings
        .ui
        .runtime_environments
        .iter()
        .find(|environment| {
            environment.id == environment_selector
                || environment.name.eq_ignore_ascii_case(environment_selector)
        })
        .cloned()
        .ok_or_else(|| format!("Environment not found: {environment_selector}"))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("Could not start remote orchestration runtime: {error}"))?;
    let status = runtime.block_on(crate::remote_runtime::request(
        environment.clone(),
        "status.get",
        Value::Null,
        Duration::from_millis(timeout_ms),
    ))?;
    let capabilities = status["capabilities"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for capability in ["orchestration.contract.v1", "orchestration.federation.v1"] {
        if !capabilities
            .iter()
            .any(|candidate| candidate.as_str() == Some(capability))
        {
            return Err(format!(
                "Connected server {} does not support {capability}. No effects were applied.",
                environment.name
            ));
        }
    }

    let dispatch_id = new_id("dispatch")?;
    let request_id = new_id("mutation")?;
    let preamble = dispatch_preamble(&dispatch_id, task, run_id, &environment.name);
    let now = now_ms();
    mutate_state(|state| {
        state.dispatches.insert(
            dispatch_id.clone(),
            DispatchRecord {
                id: dispatch_id.clone(),
                run_id: run_id.to_string(),
                task_id: task.id.clone(),
                assignee: format!("{}:{}", environment.name, worktree),
                terminal: None,
                status: "starting".into(),
                preamble: preamble.clone(),
                retry_of: option(args, "--retry-of")?,
                environment_id: Some(environment.id.clone()),
                environment_name: Some(environment.name.clone()),
                remote_runtime_epoch: None,
                remote_worktree_id: None,
                remote_to_home_sequence: 0,
                remote_to_worker_relay: Vec::new(),
                remote_to_worker_acked: 0,
                created_at_unix_ms: now,
                updated_at_unix_ms: now,
            },
        );
        if let Some(task) = state.tasks.get_mut(&task.id) {
            task.status = "dispatching".into();
            task.updated_at_unix_ms = now;
        }
        Ok(())
    })?;
    let mut remote_params = serde_json::Map::new();
    remote_params.insert("dispatchId".into(), Value::String(dispatch_id.clone()));
    remote_params.insert("taskId".into(), Value::String(task.id.clone()));
    remote_params.insert("taskSpec".into(), Value::String(task.spec.clone()));
    let federation_protocol = if capabilities
        .iter()
        .any(|candidate| candidate.as_str() == Some("orchestration.federation-control-mail.v1"))
    {
        2
    } else {
        1
    };
    remote_params.insert("protocolVersion".into(), Value::from(federation_protocol));
    remote_params.insert("worktree".into(), Value::String(worktree.clone()));
    for (flag, name) in [
        ("--name", "name"),
        ("--repo", "repo"),
        ("--base-branch", "baseBranch"),
        ("--display-name", "displayName"),
        ("--comment", "comment"),
        ("--setup", "setup"),
    ] {
        if let Some(value) = option(args, flag)? {
            remote_params.insert(name.into(), Value::String(value));
        }
    }
    if creates_worktree && !remote_params.contains_key("setup") {
        remote_params.insert("setup".into(), Value::String("run".into()));
    }
    if let Some(terminal) = terminal {
        remote_params.insert("terminal".into(), Value::String(terminal));
    }
    if let Some(agent) = agent {
        remote_params.insert("agent".into(), Value::String(agent));
    }
    remote_params.insert("timeoutMs".into(), Value::from(timeout_ms));
    if has(args, "--dev") || has(args, "--dev-mode") {
        remote_params.insert("devMode".into(), Value::Bool(true));
    }
    let remote = runtime.block_on(crate::remote_runtime::request_orchestration(
        environment.clone(),
        "orchestration.federationAttachStart",
        Value::Object(remote_params),
        request_id,
        Duration::from_millis(timeout_ms.saturating_add(15_000)),
    ));
    match remote {
        Ok(remote) => {
            let state = remote["state"].as_str().unwrap_or("outcome_unknown");
            let terminal = remote["terminalHandle"].as_str().map(str::to_string);
            let worktree_id = remote["worktreeId"].as_str().map(str::to_string);
            mutate_state(|orchestration| {
                let dispatch = orchestration
                    .dispatches
                    .get_mut(&dispatch_id)
                    .ok_or_else(|| format!("Dispatch not found: {dispatch_id}"))?;
                dispatch.status = if state == "ready" {
                    "active".into()
                } else {
                    state.to_string()
                };
                dispatch.terminal = terminal.clone();
                dispatch.remote_runtime_epoch = remote["runtimeEpoch"].as_str().map(str::to_string);
                dispatch.remote_worktree_id = worktree_id.clone();
                dispatch.updated_at_unix_ms = now_ms();
                if let Some(task) = orchestration.tasks.get_mut(&task.id) {
                    task.status = if state == "ready" {
                        "dispatched".into()
                    } else {
                        "ready".into()
                    };
                    task.updated_at_unix_ms = now_ms();
                }
                Ok(())
            })?;
            Ok(json!({
                "ready": state == "ready",
                "state": state,
                "runId": run_id,
                "taskId": task.id,
                "dispatchId": dispatch_id,
                "server": {"environmentId":environment.id, "name":environment.name},
                "worktreeId": worktree_id,
                "agentTerminalHandle": terminal,
                "effects": remote["effects"],
                "residualResources": remote["residualResources"],
                "remote": remote,
            }))
        }
        Err(error) => {
            mutate_state(|orchestration| {
                if let Some(dispatch) = orchestration.dispatches.get_mut(&dispatch_id) {
                    dispatch.status = "outcome_unknown".into();
                    dispatch.updated_at_unix_ms = now_ms();
                }
                if let Some(task) = orchestration.tasks.get_mut(&task.id) {
                    task.status = "dispatched".into();
                    task.updated_at_unix_ms = now_ms();
                }
                Ok(())
            })?;
            Ok(json!({
                "ready": false,
                "state": "outcome_unknown",
                "runId": run_id,
                "taskId": task.id,
                "dispatchId": dispatch_id,
                "server": {"environmentId":environment.id, "name":environment.name},
                "lastError": error,
                "effects": [],
                "residualResources": [],
            }))
        }
    }
}

fn wait_for_worktree(selector: &str, timeout_ms: u64) -> Result<Value, String> {
    let started = Instant::now();
    loop {
        match crate::local_rpc::call("worktree.show", json!({"worktree": selector})) {
            Ok(Some(result)) => return Ok(result),
            Ok(None) => {
                return Err("The Suaegi desktop app stopped while creating the worktree.".into())
            }
            Err(error) if started.elapsed() >= Duration::from_millis(timeout_ms) => {
                return Err(format!(
                    "Worktree {selector} did not become available within {timeout_ms} ms: {error}"
                ))
            }
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn wait_for_agent_terminal(worktree: &str, agent: &str, timeout_ms: u64) -> Result<String, String> {
    let started = Instant::now();
    loop {
        match crate::local_rpc::call("terminal.list", json!({"worktree": worktree, "limit": 100})) {
            Ok(Some(result)) => {
                if let Some(handle) = result["terminals"].as_array().and_then(|terminals| {
                    terminals.iter().find_map(|terminal| {
                        let matches_agent = terminal["agent"].as_str() == Some(agent)
                            || terminal["title"].as_str() == Some(agent);
                        matches_agent
                            .then(|| terminal["handle"].as_str().map(str::to_string))
                            .flatten()
                    })
                }) {
                    return Ok(handle);
                }
            }
            Ok(None) => {
                return Err("The Suaegi desktop app stopped while starting the worker.".into())
            }
            Err(_) => {}
        }
        if started.elapsed() >= Duration::from_millis(timeout_ms) {
            return Err(format!(
                "Agent {agent} did not start in worktree {worktree} within {timeout_ms} ms."
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn worker_show(args: &[String]) -> Result<Value, String> {
    let id = required(args, "--dispatch", "orchestration worker-show")?;
    sync_federated_dispatch(&id)?;
    let (dispatch, task) = read_state(|state| {
        let dispatch = state
            .dispatches
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("Dispatch not found: {id}"))?;
        let task = state.tasks.get(&dispatch.task_id).cloned();
        Ok((dispatch, task))
    })?;
    if let Some(environment) = remote_environment_for_dispatch(&dispatch)? {
        let remote = remote_request(
            environment,
            "orchestration.federationShow",
            json!({"dispatchId": id}),
            60_000,
        )?;
        return Ok(json!({"dispatch": dispatch, "task": task, "remote": remote}));
    }
    Ok(json!({"dispatch": dispatch, "task": task}))
}

fn worker_read(args: &[String]) -> Result<Value, String> {
    let id = required(args, "--dispatch", "orchestration worker-read")?;
    sync_federated_dispatch(&id)?;
    let dispatch = read_state(|state| {
        state
            .dispatches
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("Dispatch not found: {id}"))
    })?;
    if let Some(environment) = remote_environment_for_dispatch(&dispatch)? {
        let mut params = serde_json::Map::new();
        params.insert("dispatchId".into(), Value::String(id.clone()));
        if let Some(cursor) = option_u64(args, "--cursor")? {
            params.insert("cursor".into(), Value::from(cursor));
        }
        params.insert(
            "limit".into(),
            Value::from(option_u64(args, "--limit")?.unwrap_or(1_000)),
        );
        if let Some(source) = option(args, "--source")? {
            params.insert("source".into(), Value::String(source));
        }
        let result = remote_request(
            environment,
            "orchestration.federationReadOutput",
            Value::Object(params),
            60_000,
        )?;
        return Ok(json!({
            "dispatchId": id,
            "source": "remote",
            "output": result["output"],
            "remote": result,
        }));
    }
    let terminal = dispatch
        .terminal
        .ok_or_else(|| "Dispatch has no terminal.".to_string())?;
    let result = rpc_required(
        "terminal.read",
        json!({
            "terminal": terminal,
            "cursor": option_u64(args, "--cursor")?,
            "limit": option_u64(args, "--limit")?.unwrap_or(1_000)
        }),
    )?;
    Ok(json!({"dispatchId": id, "source": "terminal", "terminal": result["terminal"]}))
}

fn worker_stop(args: &[String]) -> Result<Value, String> {
    let id = required(args, "--dispatch", "orchestration worker-stop")?;
    let dispatch = read_state(|state| {
        state
            .dispatches
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("Dispatch not found: {id}"))
    })?;
    let remote = if let Some(environment) = remote_environment_for_dispatch(&dispatch)? {
        Some(remote_request(
            environment,
            "orchestration.federationStop",
            json!({"dispatchId": id}),
            60_000,
        )?)
    } else {
        None
    };
    let terminal = mutate_state(|state| {
        let dispatch = state
            .dispatches
            .get_mut(&id)
            .ok_or_else(|| format!("Dispatch not found: {id}"))?;
        dispatch.status = "stopped".into();
        dispatch.updated_at_unix_ms = now_ms();
        Ok(dispatch.terminal.clone())
    })?;
    if remote.is_none() {
        if let Some(terminal) = terminal {
            rpc_required("terminal.close", json!({"terminal": terminal}))?;
        }
    }
    Ok(json!({"dispatchId": id, "stopped": true, "remote": remote}))
}

fn remote_environment_for_dispatch(
    dispatch: &DispatchRecord,
) -> Result<Option<suaegi_core::domain::RuntimeEnvironmentSetting>, String> {
    let Some(environment_id) = dispatch.environment_id.as_deref() else {
        return Ok(None);
    };
    let persisted =
        suaegi_core::persistence::Store::new(crate::persistence_thread::default_data_file())
            .load()
            .state;
    persisted
        .settings
        .ui
        .runtime_environments
        .into_iter()
        .find(|environment| environment.id == environment_id)
        .map(Some)
        .ok_or_else(|| {
            format!(
                "Remote environment {} for Dispatch {} is no longer configured.",
                environment_id, dispatch.id
            )
        })
}

fn remote_request(
    environment: suaegi_core::domain::RuntimeEnvironmentSetting,
    method: &str,
    params: Value,
    timeout_ms: u64,
) -> Result<Value, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("Could not start remote orchestration runtime: {error}"))?;
    runtime.block_on(crate::remote_runtime::request(
        environment,
        method.to_string(),
        params,
        Duration::from_millis(timeout_ms),
    ))
}

fn sync_federated_dispatch(dispatch_id: &str) -> Result<(), String> {
    let dispatch = read_state(|state| {
        state
            .dispatches
            .get(dispatch_id)
            .cloned()
            .ok_or_else(|| format!("Dispatch not found: {dispatch_id}"))
    })?;
    let Some(environment) = remote_environment_for_dispatch(&dispatch)? else {
        return Ok(());
    };
    let pulled = remote_request(
        environment.clone(),
        "orchestration.federationPull",
        json!({
            "dispatchId":dispatch_id,
            "afterSequence":dispatch.remote_to_home_sequence,
            "limit":50,
        }),
        15_000,
    )?;
    let items = pulled["items"].as_array().cloned().unwrap_or_default();
    let mut cursor = dispatch.remote_to_home_sequence;
    for item in items {
        let sequence = item["sequence"]
            .as_u64()
            .ok_or_else(|| "Federated relay item has no sequence.".to_string())?;
        if sequence != cursor.saturating_add(1) || item["dispatch_id"].as_str() != Some(dispatch_id)
        {
            return Err(format!(
                "Federated relay for {dispatch_id} is not contiguous after sequence {cursor}."
            ));
        }
        let message_id = item["message_id"]
            .as_str()
            .ok_or_else(|| "Federated relay item has no message id.".to_string())?
            .to_string();
        let message: Value = serde_json::from_str(
            item["payload"]
                .as_str()
                .ok_or_else(|| "Federated relay item has no payload.".to_string())?,
        )
        .map_err(|_| "Federated relay payload is invalid JSON.".to_string())?;
        mutate_state(|state| {
            if !state.messages.contains_key(&message_id) {
                let message_type = message["type"].as_str().unwrap_or("message").to_string();
                let lifecycle_payload = message["payload"]
                    .as_str()
                    .and_then(|payload| serde_json::from_str::<Value>(payload).ok());
                let outcome = lifecycle_payload
                    .as_ref()
                    .and_then(|payload| payload["outcome"].as_str())
                    .map(str::to_string);
                let record = MessageRecord {
                    id: message_id.clone(),
                    run_id: dispatch.run_id.clone(),
                    from: format!("dispatch:{dispatch_id}"),
                    to: format!("run:{}", dispatch.run_id),
                    subject: message["subject"].as_str().unwrap_or("").to_string(),
                    body: message["body"].as_str().map(str::to_string),
                    message_type: message_type.clone(),
                    priority: message["priority"].as_str().map(str::to_string),
                    thread_id: message["threadId"].as_str().map(str::to_string),
                    payload: lifecycle_payload.clone(),
                    task_id: Some(dispatch.task_id.clone()),
                    dispatch_id: Some(dispatch_id.to_string()),
                    outcome: outcome.clone(),
                    files_modified: lifecycle_payload
                        .as_ref()
                        .and_then(|payload| payload["filesModified"].as_array())
                        .map(|files| {
                            files
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default(),
                    report_path: lifecycle_payload
                        .as_ref()
                        .and_then(|payload| payload["reportPath"].as_str())
                        .map(str::to_string),
                    phase: None,
                    reply_to: None,
                    created_at_unix_ms: now_ms(),
                };
                state.messages.insert(message_id.clone(), record);
                if message_type == "worker_done" {
                    let succeeded = outcome.as_deref() == Some("succeeded");
                    if let Some(remote_dispatch) = state.dispatches.get_mut(dispatch_id) {
                        remote_dispatch.status = if succeeded {
                            "succeeded".into()
                        } else {
                            "failed".into()
                        };
                        remote_dispatch.updated_at_unix_ms = now_ms();
                    }
                    if let Some(task) = state.tasks.get_mut(&dispatch.task_id) {
                        task.status = if succeeded {
                            "completed".into()
                        } else {
                            "failed".into()
                        };
                        task.result = lifecycle_payload.clone();
                        task.updated_at_unix_ms = now_ms();
                    }
                    refresh_ready_tasks(state);
                }
            }
            if let Some(remote_dispatch) = state.dispatches.get_mut(dispatch_id) {
                remote_dispatch.remote_to_home_sequence = sequence;
                remote_dispatch.updated_at_unix_ms = now_ms();
            }
            Ok(())
        })?;
        cursor = sequence;
    }
    if cursor > 0 {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("Could not start remote orchestration runtime: {error}"))?;
        runtime.block_on(crate::remote_runtime::request_orchestration(
            environment.clone(),
            "orchestration.federationAck",
            json!({"dispatchId":dispatch_id,"throughSequence":cursor}),
            format!("relay_ack_{dispatch_id}_{cursor}"),
            Duration::from_millis(15_000),
        ))?;
    }
    let pending = read_state(|state| {
        let dispatch = state
            .dispatches
            .get(dispatch_id)
            .ok_or_else(|| format!("Dispatch not found: {dispatch_id}"))?;
        Ok(dispatch
            .remote_to_worker_relay
            .iter()
            .filter(|item| item.sequence > dispatch.remote_to_worker_acked)
            .take(50)
            .cloned()
            .collect::<Vec<_>>())
    })?;
    if let Some(last) = pending.last() {
        let through = last.sequence;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("Could not start remote orchestration runtime: {error}"))?;
        let imported = runtime.block_on(crate::remote_runtime::request_orchestration(
            environment,
            "orchestration.federationImport",
            json!({"dispatchId":dispatch_id,"items":pending}),
            format!("relay_import_{dispatch_id}_{through}"),
            Duration::from_millis(15_000),
        ))?;
        let acknowledged = imported["acknowledgedThrough"].as_u64().unwrap_or_default();
        mutate_state(|state| {
            let dispatch = state
                .dispatches
                .get_mut(dispatch_id)
                .ok_or_else(|| format!("Dispatch not found: {dispatch_id}"))?;
            dispatch.remote_to_worker_acked = dispatch.remote_to_worker_acked.max(acknowledged);
            dispatch
                .remote_to_worker_relay
                .retain(|item| item.sequence > dispatch.remote_to_worker_acked);
            dispatch.updated_at_unix_ms = now_ms();
            Ok(())
        })?;
    }
    Ok(())
}

fn sync_all_federated_best_effort() {
    let ids = read_state(|state| {
        Ok(state
            .dispatches
            .values()
            .filter(|dispatch| dispatch.environment_id.is_some())
            .filter(|dispatch| {
                matches!(
                    dispatch.status.as_str(),
                    "starting" | "active" | "outcome_unknown"
                )
            })
            .map(|dispatch| dispatch.id.clone())
            .collect::<Vec<_>>())
    })
    .unwrap_or_default();
    for id in ids {
        let _ = sync_federated_dispatch(&id);
    }
}

pub(crate) fn start_federation_relay() {
    static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("suaegi-orchestration-federation".into())
            .spawn(|| loop {
                sync_all_federated_best_effort();
                std::thread::sleep(Duration::from_secs(2));
            });
    });
}

fn worker_abandon(args: &[String]) -> Result<Value, String> {
    let id = required(args, "--dispatch", "orchestration worker-abandon")?;
    mutate_state(|state| {
        let dispatch = state
            .dispatches
            .get_mut(&id)
            .ok_or_else(|| format!("Dispatch not found: {id}"))?;
        dispatch.status = "abandoned".into();
        dispatch.updated_at_unix_ms = now_ms();
        Ok(json!({"dispatch": dispatch, "resourcesRetained": true}))
    })
}

fn dispatch_preamble(id: &str, task: &TaskRecord, run_id: &str, assignee: &str) -> String {
    format!(
        "ORCHESTRATION DISPATCH\nrun: {run_id}\ntask: {}\ndispatch: {id}\nassignee: {assignee}\n\nTASK\n{}\n\nWhen done, run:\nsuaegi orchestration send --type worker_done --subject \"done\" --task-id {} --dispatch-id {id} --outcome succeeded --json",
        task.id, task.spec, task.id
    )
}

fn refresh_ready_tasks(state: &mut OrchestrationState) {
    let completed = state
        .tasks
        .values()
        .filter(|task| task.status == "completed")
        .map(|task| task.id.clone())
        .collect::<BTreeSet<_>>();
    let blocked = state
        .gates
        .values()
        .filter(|gate| gate.status == "pending")
        .map(|gate| gate.task_id.clone())
        .collect::<BTreeSet<_>>();
    for task in state.tasks.values_mut() {
        if matches!(task.status.as_str(), "pending" | "ready") {
            task.status = if blocked.contains(&task.id) {
                "blocked"
            } else if task.deps.iter().all(|dep| completed.contains(dep)) {
                "ready"
            } else {
                "pending"
            }
            .into();
        }
    }
}

fn resolve_run_id(
    state: &OrchestrationState,
    args: &[String],
    sender: &str,
) -> Result<String, String> {
    let id = option(args, "--run")?
        .or_else(|| state.bindings.get(sender).cloned())
        .ok_or_else(|| {
            "No orchestration run selected. Use --run or orchestration run-create/run-use."
                .to_string()
        })?;
    state
        .runs
        .contains_key(&id)
        .then_some(id.clone())
        .ok_or_else(|| format!("Orchestration run not found: {id}"))
}

fn identity(args: &[String]) -> Result<String, String> {
    option(args, "--from")?
        .or_else(|| std::env::var("ORCA_TERMINAL_HANDLE").ok())
        .or_else(|| std::env::var("SUAEGI_TERMINAL_HANDLE").ok())
        .or_else(|| std::env::var("SUAEGI_PANE_KEY").ok())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "Pass --from <terminal-handle> or run inside a Suaegi terminal.".to_string())
}

fn rpc_required(method: &str, params: Value) -> Result<Value, String> {
    crate::local_rpc::call(method, params)?
        .ok_or_else(|| "The Suaegi desktop app must be running for this operation.".to_string())
}

fn state_path() -> PathBuf {
    if let Some(path) = std::env::var_os("SUAEGI_ORCHESTRATION_STATE_PATH") {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            return path;
        }
    }
    dirs::config_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("suaegi")
        .join("orchestration.json")
}

fn read_state<T>(
    operation: impl FnOnce(&OrchestrationState) -> Result<T, String>,
) -> Result<T, String> {
    let (_lock, state) = locked_state()?;
    operation(&state)
}

fn mutate_state<T>(
    operation: impl FnOnce(&mut OrchestrationState) -> Result<T, String>,
) -> Result<T, String> {
    let (_lock, mut state) = locked_state()?;
    let result = operation(&mut state)?;
    save_state(&state)?;
    Ok(result)
}

fn locked_state() -> Result<(File, OrchestrationState), String> {
    let path = state_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create orchestration storage: {error}"))?;
    }
    let lock_path = path.with_extension("lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| format!("Could not open orchestration lock: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err("Could not lock orchestration storage.".into());
        }
    }
    let state = match File::open(&path) {
        Ok(mut file) => {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)
                .map_err(|error| format!("Could not read orchestration state: {error}"))?;
            if bytes.is_empty() {
                OrchestrationState::default()
            } else {
                serde_json::from_slice(&bytes)
                    .map_err(|error| format!("Orchestration state is invalid: {error}"))?
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => OrchestrationState::default(),
        Err(error) => return Err(format!("Could not open orchestration state: {error}")),
    };
    Ok((lock, state))
}

fn save_state(state: &OrchestrationState) -> Result<(), String> {
    let path = state_path();
    let parent = path
        .parent()
        .ok_or_else(|| "Orchestration state path has no parent.".to_string())?;
    let temp = parent.join(format!(".orchestration-{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(state)
        .map_err(|error| format!("Could not encode orchestration state: {error}"))?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp)
        .map_err(|error| format!("Could not write orchestration state: {error}"))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("Could not persist orchestration state: {error}"))?;
    std::fs::rename(&temp, &path)
        .map_err(|error| format!("Could not replace orchestration state: {error}"))
}

fn new_id(prefix: &str) -> Result<String, String> {
    let mut bytes = [0_u8; 12];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("Could not create orchestration id: {error}"))?;
    Ok(format!(
        "{prefix}_{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11]
    ))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn required(args: &[String], flag: &str, command: &str) -> Result<String, String> {
    option(args, flag)?.ok_or_else(|| format!("{command} requires {flag} <value>"))
}

fn option(args: &[String], flag: &str) -> Result<Option<String>, String> {
    let Some(index) = args.iter().position(|argument| argument == flag) else {
        return Ok(None);
    };
    args.get(index + 1)
        .filter(|value| !value.starts_with("--"))
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
        .map(Some)
}

fn option_u64(args: &[String], flag: &str) -> Result<Option<u64>, String> {
    option(args, flag)?
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| format!("{flag} must be a non-negative integer."))
        })
        .transpose()
}

fn has(args: &[String], flag: &str) -> bool {
    args.iter().any(|argument| argument == flag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_readiness_requires_every_completed_dependency() {
        let mut state = OrchestrationState::default();
        state.tasks.insert(
            "a".into(),
            TaskRecord {
                id: "a".into(),
                run_id: "r".into(),
                spec: "a".into(),
                title: None,
                display_name: None,
                deps: vec![],
                parent_id: None,
                status: "completed".into(),
                result: None,
                created_at_unix_ms: 0,
                updated_at_unix_ms: 0,
            },
        );
        state.tasks.insert(
            "b".into(),
            TaskRecord {
                id: "b".into(),
                run_id: "r".into(),
                spec: "b".into(),
                title: None,
                display_name: None,
                deps: vec!["a".into()],
                parent_id: None,
                status: "pending".into(),
                result: None,
                created_at_unix_ms: 0,
                updated_at_unix_ms: 0,
            },
        );
        refresh_ready_tasks(&mut state);
        assert_eq!(state.tasks["b"].status, "ready");
        state.tasks.get_mut("a").unwrap().status = "failed".into();
        refresh_ready_tasks(&mut state);
        assert_eq!(state.tasks["b"].status, "pending");
    }

    #[test]
    fn dispatch_preamble_contains_exact_lifecycle_ids() {
        let task = TaskRecord {
            id: "task_1".into(),
            run_id: "run_1".into(),
            spec: "Implement it".into(),
            title: None,
            display_name: None,
            deps: vec![],
            parent_id: None,
            status: "ready".into(),
            result: None,
            created_at_unix_ms: 0,
            updated_at_unix_ms: 0,
        };
        let preamble = dispatch_preamble("dispatch_1", &task, "run_1", "term_1");
        assert!(preamble.contains("--task-id task_1"));
        assert!(preamble.contains("--dispatch-id dispatch_1"));
        assert!(preamble.contains("Implement it"));
    }

    #[test]
    fn csv_fields_are_trimmed_and_empty_values_dropped() {
        assert_eq!(split_csv("a, b,,c"), vec!["a", "b", "c"]);
    }
}
