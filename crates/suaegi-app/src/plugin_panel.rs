//! Sandboxed native WebKit surface for approved plugin panel contributions.

use futures::channel::mpsc;
use futures::{Stream, StreamExt};
use iced::Subscription;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const PANEL_MESSAGE_MAX_BYTES: usize = 64 * 1024;
const PANEL_MESSAGE_MAX_COUNT: usize = 30;
const PANEL_MESSAGE_WINDOW: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy)]
pub struct PanelBounds {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone)]
pub struct PanelCall {
    pub plugin_key: String,
    pub panel_id: String,
    pub request_id: String,
    pub action: String,
    pub params: Value,
    pub validation_error: Option<String>,
    pub validation_error_code: Option<String>,
}

#[derive(Debug, Clone)]
enum PanelEvent {
    Call(PanelCall),
    Pong {
        plugin_key: String,
        panel_id: String,
        ping_id: u64,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireCall {
    #[serde(rename = "type")]
    kind: String,
    request_id: String,
    action: String,
    #[serde(default)]
    params: Value,
}

struct PanelChannel {
    sender: mpsc::Sender<PanelEvent>,
    receiver: futures::lock::Mutex<mpsc::Receiver<PanelEvent>>,
}

fn channel() -> &'static PanelChannel {
    static CHANNEL: OnceLock<PanelChannel> = OnceLock::new();
    CHANNEL.get_or_init(|| {
        let (sender, receiver) = mpsc::channel(64);
        PanelChannel {
            sender,
            receiver: futures::lock::Mutex::new(receiver),
        }
    })
}

fn panel_stream() -> impl Stream<Item = crate::state::Message> {
    futures::stream::unfold((), |_| async {
        let event = channel().receiver.lock().await.next().await?;
        let message = match event {
            PanelEvent::Call(call) => crate::state::Message::PluginPanelCallRequested(call),
            PanelEvent::Pong {
                plugin_key,
                panel_id,
                ping_id,
            } => crate::state::Message::PluginPanelPongReceived {
                plugin_key,
                panel_id,
                ping_id,
            },
        };
        Some((message, ()))
    })
}

pub fn subscription() -> Subscription<crate::state::Message> {
    Subscription::run(panel_stream)
}

fn queue_event(event: PanelEvent) {
    let mut sender = channel().sender.clone();
    let _ = sender.try_send(event);
}

fn admission(plugin_key: &str, message_bytes: usize) -> Option<(&'static str, &'static str)> {
    static BUDGETS: OnceLock<Mutex<HashMap<String, VecDeque<Instant>>>> = OnceLock::new();
    let now = Instant::now();
    let mut budgets = BUDGETS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let timestamps = budgets.entry(plugin_key.to_string()).or_default();
    while timestamps
        .front()
        .is_some_and(|timestamp| now.duration_since(*timestamp) >= PANEL_MESSAGE_WINDOW)
    {
        timestamps.pop_front();
    }
    if timestamps.len() >= PANEL_MESSAGE_MAX_COUNT {
        return Some(("rate_limited", "too many panel requests"));
    }
    timestamps.push_back(now);
    (message_bytes > PANEL_MESSAGE_MAX_BYTES).then_some((
        "invalid_request",
        "plugin panel request exceeds the 64 KB limit",
    ))
}

fn parse_event(plugin_key: &str, panel_id: &str, body: &str) -> PanelEvent {
    if let Some((code, error)) = admission(plugin_key, body.len()) {
        return PanelEvent::Call(PanelCall {
            plugin_key: plugin_key.to_string(),
            panel_id: panel_id.to_string(),
            request_id: String::new(),
            action: String::new(),
            params: Value::Null,
            validation_error: Some(error.into()),
            validation_error_code: Some(code.into()),
        });
    }
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        if value.get("type").and_then(Value::as_str) == Some("orca-panel-pong") {
            if let Some(ping_id) = value.get("pingId").and_then(Value::as_u64) {
                return PanelEvent::Pong {
                    plugin_key: plugin_key.to_string(),
                    panel_id: panel_id.to_string(),
                    ping_id,
                };
            }
        }
    }
    PanelEvent::Call(parse_call(plugin_key, panel_id, body))
}

fn parse_call(plugin_key: &str, panel_id: &str, body: &str) -> PanelCall {
    let fallback_request_id = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.get("requestId")?.as_str().map(str::to_string))
        .filter(|value| !value.is_empty() && value.len() <= 128)
        .unwrap_or_default();
    let parsed = serde_json::from_str::<WireCall>(body);
    match parsed {
        Ok(call)
            if call.kind == "orca-panel-action"
                && !call.request_id.is_empty()
                && call.request_id.len() <= 128
                && matches!(
                    call.action.as_str(),
                    "workspace.readContext" | "terminal.sendText" | "notifications.show"
                ) =>
        {
            PanelCall {
                plugin_key: plugin_key.to_string(),
                panel_id: panel_id.to_string(),
                request_id: call.request_id,
                action: call.action,
                params: call.params,
                validation_error: None,
                validation_error_code: None,
            }
        }
        _ => PanelCall {
            plugin_key: plugin_key.to_string(),
            panel_id: panel_id.to_string(),
            request_id: fallback_request_id,
            action: String::new(),
            params: Value::Null,
            validation_error: Some("invalid or forbidden plugin panel request".into()),
            validation_error_code: Some("invalid_request".into()),
        },
    }
}

pub fn shell_html(plugin_html: &str, dark: bool) -> String {
    let scheme = if dark { "dark" } else { "light" };
    format!(
        r#"<!doctype html>
<html class="{scheme}">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; connect-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:; base-uri 'none'; form-action 'none'">
<meta name="color-scheme" content="{scheme}">
<style>
:root{{--background:#171717;--foreground:#ededed;--card:#202020;--card-foreground:#ededed;--popover:#202020;--popover-foreground:#ededed;--primary:#5b8cff;--primary-foreground:#fff;--secondary:#2b2b2b;--secondary-foreground:#ededed;--muted:#292929;--muted-foreground:#a3a3a3;--accent:#303030;--accent-foreground:#fff;--destructive:#dc4c4c;--destructive-foreground:#fff;--border:#383838;--input:#383838;--ring:#5b8cff;--radius:6px}}
html.light{{--background:#fff;--foreground:#202020;--card:#f7f7f7;--card-foreground:#202020;--popover:#fff;--popover-foreground:#202020;--primary:#356ae6;--primary-foreground:#fff;--secondary:#eee;--secondary-foreground:#202020;--muted:#f0f0f0;--muted-foreground:#666;--accent:#e8e8e8;--accent-foreground:#111;--destructive:#c83f3f;--destructive-foreground:#fff;--border:#ddd;--input:#ddd;--ring:#356ae6;--radius:6px}}
html,body{{margin:0;min-height:100%;background:var(--background);color:var(--foreground);font:13px -apple-system,BlinkMacSystemFont,sans-serif}}
</style>
<script>
'use strict';
try{{Object.defineProperty(window,'open',{{value:function(){{return null}},writable:false,configurable:false}})}}catch(_e){{}}
window.addEventListener('click',function(event){{var node=event.target;while(node&&node!==document){{if(node.nodeType===1&&node.tagName==='A'&&node.hasAttribute('href')){{event.preventDefault();event.stopImmediatePropagation();return}}node=node.parentNode}}}},true);
window.addEventListener('submit',function(event){{event.preventDefault();event.stopImmediatePropagation()}},true);
window.addEventListener('message',function(event){{var data=event.data;if(data&&data.type==='orca-panel-action'){{window.ipc.postMessage(JSON.stringify(data))}}}});
window.addEventListener('message',function(event){{var data=event.data;if(data&&data.type==='orca-panel-ping'&&Number.isSafeInteger(data.pingId)&&data.pingId>=0){{window.ipc.postMessage(JSON.stringify({{type:'orca-panel-pong',pingId:data.pingId}}))}}}});
</script>
</head>
{plugin_html}"#
    )
}

pub fn read_html(
    plugin: &crate::plugins::PluginEntry,
    panel_id: &str,
) -> Result<(String, String), String> {
    const MAX_PANEL_BYTES: u64 = 2 * 1024 * 1024;
    if plugin.status != crate::plugins::PluginStatus::Idle || plugin.blocked_by_kill_list.is_some()
    {
        return Err("plugin is not approved and active".into());
    }
    crate::plugins::verify_installed_content(&plugin.root, plugin.content_hash.as_deref())?;
    let panel = plugin
        .panels
        .iter()
        .find(|panel| panel.id == panel_id)
        .ok_or_else(|| "plugin panel is no longer available".to_string())?;
    let root = plugin
        .root
        .canonicalize()
        .map_err(|error| format!("could not resolve plugin root: {error}"))?;
    let path = root
        .join(&panel.entry)
        .canonicalize()
        .map_err(|error| format!("could not resolve plugin panel: {error}"))?;
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|error| format!("could not inspect plugin panel: {error}"))?;
    if !path.starts_with(&root)
        || metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_PANEL_BYTES
    {
        return Err("plugin panel escaped its root or exceeded the 2 MB limit".into());
    }
    let html = std::fs::read_to_string(path)
        .map_err(|_| "plugin panel must be readable UTF-8 HTML".to_string())?;
    Ok((panel.title.clone(), html))
}

#[cfg(target_os = "macos")]
mod native {
    use super::{parse_event, queue_event, shell_html, PanelBounds};
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use raw_window_handle::{
        AppKitWindowHandle, HandleError, HasWindowHandle, RawWindowHandle, WindowHandle,
    };
    use serde_json::Value;
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::ptr::NonNull;
    use std::time::{Duration, Instant};
    use wry::dpi::{LogicalPosition, LogicalSize};
    use wry::{Rect, WebView, WebViewBuilder};

    thread_local! {
        static PANEL: RefCell<Option<PanelRuntime>> = const { RefCell::new(None) };
    }

    struct PanelRuntime {
        plugin_key: String,
        panel_id: String,
        webview: WebView,
        next_ping_id: u64,
        awaiting_pong: Option<(u64, Instant)>,
    }

    struct ParentViewHandle(NonNull<c_void>);

    impl HasWindowHandle for ParentViewHandle {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let raw = RawWindowHandle::AppKit(AppKitWindowHandle::new(self.0));
            // SAFETY: this points to the retained contentView of the live app window.
            Ok(unsafe { WindowHandle::borrow_raw(raw) })
        }
    }

    fn rect(bounds: PanelBounds) -> Rect {
        Rect {
            position: LogicalPosition::new(bounds.left as f64, bounds.top as f64).into(),
            size: LogicalSize::new(bounds.width.max(1.0) as f64, bounds.height.max(1.0) as f64)
                .into(),
        }
    }

    pub fn ensure(
        plugin_key: &str,
        panel_id: &str,
        html: &str,
        dark: bool,
        bounds: PanelBounds,
    ) -> Result<(), String> {
        let bounds = rect(bounds);
        let reused = PANEL.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(runtime) = slot.as_mut() else {
                return Ok(false);
            };
            if runtime.plugin_key != plugin_key || runtime.panel_id != panel_id {
                return Ok(false);
            }
            runtime
                .webview
                .set_bounds(bounds)
                .map_err(|error| error.to_string())?;
            runtime
                .webview
                .set_visible(true)
                .map_err(|error| error.to_string())?;
            Ok::<bool, String>(true)
        })?;
        if reused {
            return Ok(());
        }
        PANEL.with(|slot| {
            if let Some(previous) = slot.borrow_mut().take() {
                drop(previous);
            }
        });

        let mtm = MainThreadMarker::new()
            .ok_or_else(|| "plugin panels must be created on the macOS main thread".to_string())?;
        let app = NSApplication::sharedApplication(mtm);
        let window = app
            .keyWindow()
            .or_else(|| app.mainWindow())
            .or_else(|| app.windows().firstObject())
            .ok_or_else(|| "Suaegi window is not ready yet".to_string())?;
        let content = window
            .contentView()
            .ok_or_else(|| "Suaegi window has no content view".to_string())?;
        let parent = ParentViewHandle(NonNull::from(&*content).cast());
        let callback_plugin = plugin_key.to_string();
        let callback_panel = panel_id.to_string();
        let webview = WebViewBuilder::new()
            .with_html(shell_html(html, dark))
            .with_bounds(bounds)
            .with_accept_first_mouse(true)
            .with_devtools(cfg!(debug_assertions))
            .with_navigation_handler(|_| false)
            .with_ipc_handler(move |request| {
                queue_event(parse_event(
                    &callback_plugin,
                    &callback_panel,
                    request.body(),
                ));
            })
            .build_as_child(&parent)
            .map_err(|error| error.to_string())?;
        webview.focus().map_err(|error| error.to_string())?;
        PANEL.with(|slot| {
            *slot.borrow_mut() = Some(PanelRuntime {
                plugin_key: plugin_key.to_string(),
                panel_id: panel_id.to_string(),
                webview,
                next_ping_id: 0,
                awaiting_pong: None,
            });
        });
        Ok(())
    }

    pub fn set_visible(visible: bool) {
        PANEL.with(|slot| {
            if let Some(runtime) = slot.borrow().as_ref() {
                let _ = runtime.webview.set_visible(visible);
            }
        });
    }

    pub fn resize(bounds: PanelBounds) -> Result<(), String> {
        PANEL.with(|slot| {
            let slot = slot.borrow();
            let runtime = slot
                .as_ref()
                .ok_or_else(|| "plugin panel is not initialized".to_string())?;
            runtime
                .webview
                .set_bounds(rect(bounds))
                .map_err(|error| error.to_string())
        })
    }

    pub fn respond(
        plugin_key: &str,
        panel_id: &str,
        request_id: &str,
        result: Result<Value, String>,
    ) -> Result<(), String> {
        let message = match result {
            Ok(value) => serde_json::json!({
                "type": "orca-panel-action-result",
                "requestId": request_id,
                "ok": true,
                "value": value,
            }),
            Err(error) => serde_json::json!({
                "type": "orca-panel-action-result",
                "requestId": request_id,
                "ok": false,
                "errorCode": error_code(&error),
                "error": error.chars().take(1000).collect::<String>(),
            }),
        };
        let encoded =
            serde_json::to_string(&message).map_err(|error| format!("encode failed: {error}"))?;
        PANEL.with(|slot| {
            let slot = slot.borrow();
            let runtime = slot
                .as_ref()
                .ok_or_else(|| "plugin panel is not initialized".to_string())?;
            if runtime.plugin_key != plugin_key || runtime.panel_id != panel_id {
                return Ok(());
            }
            runtime
                .webview
                .evaluate_script(&format!("window.postMessage({encoded}, '*');"))
                .map_err(|error| error.to_string())
        })
    }

    pub fn respond_error(
        plugin_key: &str,
        panel_id: &str,
        request_id: &str,
        error_code: &str,
        error: &str,
    ) -> Result<(), String> {
        let message = serde_json::json!({
            "type": "orca-panel-action-result",
            "requestId": request_id,
            "ok": false,
            "errorCode": error_code,
            "error": error.chars().take(1000).collect::<String>(),
        });
        let encoded =
            serde_json::to_string(&message).map_err(|error| format!("encode failed: {error}"))?;
        evaluate_for(
            plugin_key,
            panel_id,
            &format!("window.postMessage({encoded}, '*');"),
        )
    }

    pub fn ping() -> Result<(), String> {
        PANEL.with(|slot| {
            let mut slot = slot.borrow_mut();
            let runtime = slot
                .as_mut()
                .ok_or_else(|| "plugin panel is not initialized".to_string())?;
            if runtime
                .awaiting_pong
                .is_some_and(|(_, sent)| sent.elapsed() >= Duration::from_secs(5))
            {
                return Err("plugin panel stopped responding".into());
            }
            if runtime.awaiting_pong.is_some() {
                return Ok(());
            }
            runtime.next_ping_id = runtime.next_ping_id.wrapping_add(1);
            let ping_id = runtime.next_ping_id;
            runtime
                .webview
                .evaluate_script(&format!(
                    "window.postMessage({{type:'orca-panel-ping',pingId:{ping_id}}}, '*');"
                ))
                .map_err(|error| error.to_string())?;
            runtime.awaiting_pong = Some((ping_id, Instant::now()));
            Ok(())
        })
    }

    pub fn accept_pong(plugin_key: &str, panel_id: &str, ping_id: u64) -> bool {
        PANEL.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(runtime) = slot.as_mut() else {
                return false;
            };
            if runtime.plugin_key != plugin_key || runtime.panel_id != panel_id {
                return false;
            }
            if runtime
                .awaiting_pong
                .is_some_and(|(expected, _)| expected == ping_id)
            {
                runtime.awaiting_pong = None;
                return true;
            }
            false
        })
    }

    fn evaluate_for(plugin_key: &str, panel_id: &str, script: &str) -> Result<(), String> {
        PANEL.with(|slot| {
            let slot = slot.borrow();
            let runtime = slot
                .as_ref()
                .ok_or_else(|| "plugin panel is not initialized".to_string())?;
            if runtime.plugin_key != plugin_key || runtime.panel_id != panel_id {
                return Ok(());
            }
            runtime
                .webview
                .evaluate_script(script)
                .map_err(|error| error.to_string())
        })
    }

    fn error_code(error: &str) -> &'static str {
        if error.contains("capability") {
            "capability_denied"
        } else if error.contains("params")
            || error.contains("requires")
            || error.contains("must be")
        {
            "invalid_params"
        } else if error.contains("unknown") {
            "unknown_method"
        } else if error.contains("approved") || error.contains("active") {
            "unavailable"
        } else {
            "action_failed"
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod native {
    use super::PanelBounds;
    use serde_json::Value;

    pub fn ensure(
        _plugin_key: &str,
        _panel_id: &str,
        _html: &str,
        _dark: bool,
        _bounds: PanelBounds,
    ) -> Result<(), String> {
        Err("Native plugin panels are currently available on macOS.".into())
    }
    pub fn set_visible(_visible: bool) {}
    pub fn resize(_bounds: PanelBounds) -> Result<(), String> {
        Ok(())
    }
    pub fn respond(
        _plugin_key: &str,
        _panel_id: &str,
        _request_id: &str,
        _result: Result<Value, String>,
    ) -> Result<(), String> {
        Ok(())
    }
    pub fn respond_error(
        _plugin_key: &str,
        _panel_id: &str,
        _request_id: &str,
        _error_code: &str,
        _error: &str,
    ) -> Result<(), String> {
        Ok(())
    }
    pub fn ping() -> Result<(), String> {
        Ok(())
    }
    pub fn accept_pong(_plugin_key: &str, _panel_id: &str, _ping_id: u64) -> bool {
        true
    }
}

pub use native::{accept_pong, ensure, ping, resize, respond, respond_error, set_visible};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_bridge_accepts_only_the_three_orca_panel_actions() {
        let call = parse_call(
            "acme.demo",
            "dashboard",
            r#"{"type":"orca-panel-action","requestId":"one","action":"workspace.readContext","params":{}}"#,
        );
        assert_eq!(call.action, "workspace.readContext");
        assert!(call.validation_error.is_none());

        let forbidden = parse_call(
            "acme.demo",
            "dashboard",
            r#"{"type":"orca-panel-action","requestId":"two","action":"secrets.get","params":{"key":"token"}}"#,
        );
        assert!(forbidden.validation_error.is_some());
        assert_eq!(forbidden.request_id, "two");
    }

    #[test]
    fn shell_installs_csp_before_plugin_markup_and_blocks_navigation() {
        let html = shell_html("<main>panel</main>", true);
        assert!(html.find("Content-Security-Policy") < html.find("<main>panel</main>"));
        assert!(html.contains("connect-src 'none'"));
        assert!(html.contains("orca-panel-action"));
        assert!(html.contains("orca-panel-ping"));
        assert!(html.contains("orca-panel-pong"));
    }

    #[test]
    fn panel_bridge_enforces_one_sliding_budget_across_plugin_panels() {
        let body = r#"{"type":"orca-panel-action","requestId":"one","action":"workspace.readContext","params":{}}"#;
        for index in 0..PANEL_MESSAGE_MAX_COUNT {
            let event = parse_event("rate-limit.fixture", &format!("panel-{index}"), body);
            assert!(matches!(
                event,
                PanelEvent::Call(PanelCall {
                    validation_error: None,
                    ..
                })
            ));
        }
        let event = parse_event("rate-limit.fixture", "overflow", body);
        assert!(matches!(event, PanelEvent::Call(PanelCall {
            validation_error_code: Some(code),
            ..
        }) if code == "rate_limited"));
    }
}
