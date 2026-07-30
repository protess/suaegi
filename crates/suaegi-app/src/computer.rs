use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const LIST_APPS_SCRIPT: &str = r#"
function run() {
  ObjC.import("AppKit");
  const front = $.NSWorkspace.sharedWorkspace.frontmostApplication;
  const frontPid = front ? Number(front.processIdentifier) : 0;
  const apps = $.NSWorkspace.sharedWorkspace.runningApplications.js
    .map(app => {
      const name = app.localizedName ? ObjC.unwrap(app.localizedName) : "";
      const bundle = app.bundleIdentifier ? ObjC.unwrap(app.bundleIdentifier) : null;
      return {
        name,
        bundleIdentifier: bundle,
        pid: Number(app.processIdentifier),
        active: Number(app.processIdentifier) === frontPid,
        hidden: Boolean(app.hidden),
        activationPolicy: Number(app.activationPolicy)
      };
    })
    .filter(app => app.name && app.activationPolicy !== 2)
    .sort((a, b) => a.name.localeCompare(b.name) || a.pid - b.pid);
  return JSON.stringify(apps);
}
"#;

const LIST_WINDOWS_SCRIPT: &str = r#"
function safe(fn, fallback) { try { const value = fn(); return value === undefined ? fallback : value; } catch (_) { return fallback; } }
function run(argv) {
  const pid = Number(argv[0]);
  const process = Application("System Events").applicationProcesses()
    .find(candidate => safe(() => candidate.unixId() === pid, false));
  if (!process) throw new Error("app_not_running");
  const windows = process.windows().map((window, index) => {
    const position = safe(() => window.position(), null);
    const size = safe(() => window.size(), null);
    return {
      id: safe(() => String(window.attributes.byName("AXWindowNumber").value()), null),
      index,
      title: safe(() => window.name(), ""),
      role: safe(() => window.role(), "AXWindow"),
      subrole: safe(() => window.subrole(), null),
      position,
      size,
      bounds: position && size ? { x: position[0], y: position[1], width: size[0], height: size[1] } : null,
      focused: safe(() => window.attributes.byName("AXFocused").value(), false),
      minimized: safe(() => window.attributes.byName("AXMinimized").value(), false)
    };
  });
  return JSON.stringify(windows);
}
"#;

const SNAPSHOT_SCRIPT: &str = r#"
function safe(fn, fallback) { try { const value = fn(); return value === undefined ? fallback : value; } catch (_) { return fallback; } }
function run(argv) {
  const pid = Number(argv[0]);
  const windowIndex = Number(argv[1] || 0);
  const process = Application("System Events").applicationProcesses()
    .find(candidate => safe(() => candidate.unixId() === pid, false));
  if (!process) throw new Error("app_not_running");
  const windows = process.windows();
  if (windowIndex < 0 || windowIndex >= windows.length) throw new Error("window_not_found");
  const window = windows[windowIndex];
  const contents = safe(() => window.entireContents(), []);
  const elements = [window].concat(contents).slice(0, 500).map((element, index) => {
    const position = safe(() => element.position(), null);
    const size = safe(() => element.size(), null);
    const actions = safe(() => element.actions().map(action => action.name()), []);
    return {
      index,
      role: safe(() => element.role(), null),
      subrole: safe(() => element.subrole(), null),
      title: safe(() => element.title(), null),
      description: safe(() => element.description(), null),
      value: safe(() => {
        const value = element.value();
        return ["string", "number", "boolean"].includes(typeof value) ? value : null;
      }, null),
      enabled: safe(() => element.enabled(), null),
      focused: safe(() => element.attributes.byName("AXFocused").value(), null),
      position,
      size,
      bounds: position && size ? { x: position[0], y: position[1], width: size[0], height: size[1] } : null,
      actions
    };
  });
  return JSON.stringify({
    pid,
    app: safe(() => process.name(), ""),
    window: {
      index: windowIndex,
      title: safe(() => window.name(), ""),
      position: safe(() => window.position(), null),
      size: safe(() => window.size(), null)
    },
    elements,
    truncated: contents.length + 1 > 500
  });
}
"#;

const ELEMENT_ACTION_SCRIPT: &str = r#"
function safe(fn, fallback) { try { const value = fn(); return value === undefined ? fallback : value; } catch (_) { return fallback; } }
function run(argv) {
  const pid = Number(argv[0]);
  const windowIndex = Number(argv[1]);
  const elementIndex = Number(argv[2]);
  const operation = argv[3];
  const value = argv[4] || "";
  const process = Application("System Events").applicationProcesses()
    .find(candidate => safe(() => candidate.unixId() === pid, false));
  if (!process) throw new Error("app_not_running");
  process.frontmost = true;
  const windows = process.windows();
  if (windowIndex < 0 || windowIndex >= windows.length) throw new Error("window_not_found");
  const window = windows[windowIndex];
  const elements = [window].concat(safe(() => window.entireContents(), []));
  if (elementIndex < 0 || elementIndex >= elements.length) throw new Error("element_not_found");
  const element = elements[elementIndex];
  if (operation === "set-value") {
    element.value = value;
  } else {
    const actions = safe(() => element.actions(), []);
    const wanted = operation === "click" ? "AXPress" : value;
    const action = actions.find(candidate => safe(() => candidate.name() === wanted, false));
    if (!action) throw new Error("action_not_available");
    action.perform();
  }
  return JSON.stringify({ ok: true, pid, windowIndex, elementIndex, operation });
}
"#;

const KEYBOARD_ACTION_SCRIPT: &str = r#"
function safe(fn, fallback) { try { const value = fn(); return value === undefined ? fallback : value; } catch (_) { return fallback; } }
function run(argv) {
  ObjC.import("AppKit");
  const pid = Number(argv[0]);
  const operation = argv[1];
  const value = argv[2] || "";
  const modifiers = argv.slice(3);
  const se = Application("System Events");
  const process = se.applicationProcesses().find(candidate => safe(() => candidate.unixId() === pid, false));
  if (!process) throw new Error("app_not_running");
  process.frontmost = true;
  delay(0.05);
  const keyCodes = {
    return: 36, enter: 36, escape: 53, esc: 53, tab: 48, space: 49,
    backspace: 51, delete: 51, forwarddelete: 117,
    left: 123, right: 124, down: 125, up: 126,
    home: 115, end: 119, pageup: 116, pagedown: 121
  };
  const using = modifiers.map(modifier => ({
    command: "command down", cmd: "command down", meta: "command down",
    control: "control down", ctrl: "control down",
    option: "option down", alt: "option down", shift: "shift down"
  })[modifier]).filter(Boolean);
  if (operation === "type") {
    se.keystroke(value);
  } else if (operation === "paste") {
    const pasteboard = $.NSPasteboard.generalPasteboard;
    const oldValue = pasteboard.stringForType($.NSPasteboardTypeString);
    pasteboard.clearContents;
    pasteboard.setStringForType($(value), $.NSPasteboardTypeString);
    se.keystroke("v", { using: ["command down"] });
    delay(0.08);
    pasteboard.clearContents;
    if (oldValue) pasteboard.setStringForType(oldValue, $.NSPasteboardTypeString);
  } else {
    const lower = value.toLowerCase().replace(/[-_ ]/g, "");
    if (Object.prototype.hasOwnProperty.call(keyCodes, lower)) {
      se.keyCode(keyCodes[lower], using.length ? { using } : {});
    } else if (value.length === 1) {
      se.keystroke(value, using.length ? { using } : {});
    } else {
      throw new Error("unsupported_key");
    }
  }
  return JSON.stringify({ ok: true, pid, operation });
}
"#;

const POINTER_ACTION_SCRIPT: &str = r#"
function event(type, x, y, button) {
  const point = $.CGPointMake(x, y);
  const created = $.CGEventCreateMouseEvent(null, type, point, button);
  $.CGEventPost($.kCGHIDEventTap, created);
}
function run(argv) {
  ObjC.import("CoreGraphics");
  const operation = argv[0];
  const values = argv.slice(1).map(Number);
  const left = $.kCGMouseButtonLeft;
  if (operation === "click") {
    const [x, y, count, buttonValue] = values;
    const button = buttonValue || left;
    const down = button === 1 ? $.kCGEventRightMouseDown : button === 2 ? $.kCGEventOtherMouseDown : $.kCGEventLeftMouseDown;
    const up = button === 1 ? $.kCGEventRightMouseUp : button === 2 ? $.kCGEventOtherMouseUp : $.kCGEventLeftMouseUp;
    for (let i = 0; i < Math.max(1, count); i++) { event(down, x, y, button); event(up, x, y, button); }
  } else if (operation === "drag") {
    const [fromX, fromY, toX, toY] = values;
    event($.kCGEventLeftMouseDown, fromX, fromY, left);
    event($.kCGEventLeftMouseDragged, toX, toY, left);
    event($.kCGEventLeftMouseUp, toX, toY, left);
  } else if (operation === "scroll") {
    const [dx, dy] = values;
    const created = $.CGEventCreateScrollWheelEvent(null, $.kCGScrollEventUnitPixel, 2, dy, dx);
    $.CGEventPost($.kCGHIDEventTap, created);
  } else {
    throw new Error("unsupported_pointer_operation");
  }
  return JSON.stringify({ ok: true, operation });
}
"#;

#[derive(Debug, Clone)]
struct ComputerApp {
    name: String,
    bundle_identifier: Option<String>,
    pid: i64,
}

pub fn capabilities() -> Value {
    json!({
        "provider": "macos-accessibility",
        "platform": std::env::consts::OS,
        "available": cfg!(target_os = "macos"),
        "features": {
            "apps": true,
            "windows": true,
            "accessibilitySnapshot": true,
            "screenshot": true,
            "click": true,
            "secondaryActions": true,
            "scroll": true,
            "drag": true,
            "keyboard": true,
            "paste": true,
            "setValue": true
        },
        "limits": {"maxElements": 500}
    })
}

pub fn run(command: &str, args: &[String]) -> Result<Value, String> {
    require_macos()?;
    match command {
        "capabilities" => Ok(capabilities()),
        "list-apps" => list_apps(),
        "permissions" => open_permissions(args),
        "list-windows" => {
            let app = resolve_app(&required(args, "--app", "computer list-windows")?)?;
            let windows = run_jxa(LIST_WINDOWS_SCRIPT, &[app.pid.to_string()])?;
            Ok(json!({"app": app_json(&app), "windows": windows}))
        }
        "get-app-state" => snapshot_command(args),
        "click" => click_command(args),
        "perform-secondary-action" => element_action_command(args, "secondary"),
        "scroll" => scroll_command(args),
        "drag" => drag_command(args),
        "type-text" => keyboard_command(args, "type"),
        "press-key" => keyboard_command(args, "key"),
        "hotkey" => keyboard_command(args, "hotkey"),
        "paste-text" => keyboard_command(args, "paste"),
        "set-value" => element_action_command(args, "set-value"),
        _ => Err(format!("Unknown computer command: {command}")),
    }
}

fn require_macos() -> Result<(), String> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err("Computer Use is currently available only on macOS.".into())
    }
}

fn list_apps() -> Result<Value, String> {
    let apps = run_jxa(LIST_APPS_SCRIPT, &[])?;
    Ok(json!({"apps": apps}))
}

fn resolve_app(selector: &str) -> Result<ComputerApp, String> {
    let apps = run_jxa(LIST_APPS_SCRIPT, &[])?;
    let rows = apps
        .as_array()
        .ok_or_else(|| "Computer Use returned an invalid app list.".to_string())?;
    let pid_selector = selector
        .strip_prefix("pid:")
        .and_then(|value| value.parse::<i64>().ok());
    let mut matches = rows
        .iter()
        .filter(|row| {
            pid_selector.is_some_and(|pid| row["pid"].as_i64() == Some(pid))
                || row["name"]
                    .as_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case(selector))
                || row["bundleIdentifier"]
                    .as_str()
                    .is_some_and(|bundle| bundle.eq_ignore_ascii_case(selector))
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(if matches.is_empty() {
            format!("Computer Use app was not found: {selector}")
        } else {
            format!("Computer Use app selector is ambiguous: {selector}")
        });
    }
    let row = matches.remove(0);
    Ok(ComputerApp {
        name: row["name"].as_str().unwrap_or_default().to_string(),
        bundle_identifier: row["bundleIdentifier"].as_str().map(str::to_string),
        pid: row["pid"].as_i64().unwrap_or_default(),
    })
}

fn app_json(app: &ComputerApp) -> Value {
    json!({
        "name": app.name,
        "bundleIdentifier": app.bundle_identifier,
        "pid": app.pid
    })
}

fn open_permissions(args: &[String]) -> Result<Value, String> {
    let id = option(args, "--id")?.unwrap_or_else(|| "accessibility".into());
    let url = match id.as_str() {
        "accessibility" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        "screenshots" | "screen-recording" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
        _ => return Err("--id must be accessibility or screenshots.".into()),
    };
    let status = Command::new("/usr/bin/open")
        .arg(url)
        .status()
        .map_err(|error| format!("Could not open macOS privacy settings: {error}"))?;
    if !status.success() {
        return Err("macOS privacy settings could not be opened.".into());
    }
    Ok(json!({"opened": true, "permission": id}))
}

fn snapshot_command(args: &[String]) -> Result<Value, String> {
    let app = resolve_app(&required(args, "--app", "computer get-app-state")?)?;
    let window_index = window_index(args)?;
    let previous = restore_target(args)?;
    activate(app.pid)?;
    let mut snapshot = snapshot(&app, window_index)?;
    if !has(args, "--no-screenshot") {
        snapshot["screenshot"] = capture_screenshot().map_or(Value::Null, Value::String);
    }
    restore(previous);
    Ok(snapshot)
}

fn snapshot(app: &ComputerApp, window_index: usize) -> Result<Value, String> {
    let mut value = run_jxa(
        SNAPSHOT_SCRIPT,
        &[app.pid.to_string(), window_index.to_string()],
    )?;
    value["appIdentity"] = app_json(app);
    Ok(value)
}

fn click_command(args: &[String]) -> Result<Value, String> {
    let app = resolve_app(&required(args, "--app", "computer click")?)?;
    let previous = restore_target(args)?;
    activate(app.pid)?;
    let element = option_usize(args, "--element-index")?;
    let coordinates = coordinates(args, "--x", "--y")?;
    if element.is_some() == coordinates.is_some() {
        return Err("computer click requires either --element-index or --x and --y.".into());
    }
    let count = option_usize(args, "--click-count")?
        .unwrap_or(1)
        .clamp(1, 3);
    let button = match option(args, "--mouse-button")?.as_deref().unwrap_or("left") {
        "left" => 0,
        "right" => 1,
        "middle" => 2,
        _ => return Err("--mouse-button must be left, right, or middle.".into()),
    };
    if let Some(index) = element {
        for _ in 0..count {
            run_jxa(
                ELEMENT_ACTION_SCRIPT,
                &[
                    app.pid.to_string(),
                    window_index(args)?.to_string(),
                    index.to_string(),
                    "click".into(),
                    String::new(),
                ],
            )?;
        }
    } else if let Some((x, y)) = coordinates {
        run_jxa(
            POINTER_ACTION_SCRIPT,
            &[
                "click".into(),
                x.to_string(),
                y.to_string(),
                count.to_string(),
                button.to_string(),
            ],
        )?;
    }
    let result = action_result(&app, args, "click")?;
    restore(previous);
    Ok(result)
}

fn element_action_command(args: &[String], operation: &str) -> Result<Value, String> {
    let app = resolve_app(&required(args, "--app", "computer action")?)?;
    let index = option_usize(args, "--element-index")?
        .ok_or_else(|| "--element-index is required.".to_string())?;
    let (script_operation, value) = match operation {
        "secondary" => (
            "secondary",
            required(args, "--action", "computer perform-secondary-action")?,
        ),
        "set-value" => (
            "set-value",
            text_or_stdin(args, "--value", "--value-stdin", "computer set-value")?,
        ),
        _ => return Err("Unsupported accessibility action.".into()),
    };
    let previous = restore_target(args)?;
    activate(app.pid)?;
    run_jxa(
        ELEMENT_ACTION_SCRIPT,
        &[
            app.pid.to_string(),
            window_index(args)?.to_string(),
            index.to_string(),
            script_operation.into(),
            value,
        ],
    )?;
    let result = action_result(&app, args, operation)?;
    restore(previous);
    Ok(result)
}

fn scroll_command(args: &[String]) -> Result<Value, String> {
    let app = resolve_app(&required(args, "--app", "computer scroll")?)?;
    let direction = required(args, "--direction", "computer scroll")?;
    let pages = option_f64(args, "--pages")?.unwrap_or(1.0);
    if !pages.is_finite() || pages <= 0.0 {
        return Err("--pages must be a positive number.".into());
    }
    let (dx, dy) = match direction.as_str() {
        "up" => (0.0, pages * 600.0),
        "down" => (0.0, pages * -600.0),
        "left" => (pages * 600.0, 0.0),
        "right" => (pages * -600.0, 0.0),
        _ => return Err("--direction must be up, down, left, or right.".into()),
    };
    let previous = restore_target(args)?;
    activate(app.pid)?;
    if let Some(index) = option_usize(args, "--element-index")? {
        let state = snapshot(&app, window_index(args)?)?;
        let bounds = state["elements"]
            .get(index)
            .and_then(|element| element.get("bounds"))
            .ok_or_else(|| "Element has no scrollable screen bounds.".to_string())?;
        let x = bounds["x"].as_f64().unwrap_or_default()
            + bounds["width"].as_f64().unwrap_or_default() / 2.0;
        let y = bounds["y"].as_f64().unwrap_or_default()
            + bounds["height"].as_f64().unwrap_or_default() / 2.0;
        run_jxa(
            POINTER_ACTION_SCRIPT,
            &[
                "click".into(),
                x.to_string(),
                y.to_string(),
                "1".into(),
                "0".into(),
            ],
        )?;
    } else if coordinates(args, "--x", "--y")?.is_none() {
        return Err("computer scroll requires --element-index or --x and --y.".into());
    }
    run_jxa(
        POINTER_ACTION_SCRIPT,
        &["scroll".into(), dx.to_string(), dy.to_string()],
    )?;
    let result = action_result(&app, args, "scroll")?;
    restore(previous);
    Ok(result)
}

fn drag_command(args: &[String]) -> Result<Value, String> {
    let app = resolve_app(&required(args, "--app", "computer drag")?)?;
    let from_element = option_usize(args, "--from-element-index")?;
    let to_element = option_usize(args, "--to-element-index")?;
    let element_pair = match (from_element, to_element) {
        (Some(from), Some(to)) => Some((from, to)),
        (None, None) => None,
        _ => return Err("Both --from-element-index and --to-element-index are required.".into()),
    };
    let coordinate_pair = match (
        coordinates(args, "--from-x", "--from-y")?,
        coordinates(args, "--to-x", "--to-y")?,
    ) {
        (Some(from), Some(to)) => Some((from, to)),
        (None, None) => None,
        _ => return Err("All drag coordinates are required.".into()),
    };
    if element_pair.is_some() == coordinate_pair.is_some() {
        return Err("Use either element indices or coordinates for computer drag.".into());
    }
    let (from, to) = if let Some((from, to)) = element_pair {
        let state = snapshot(&app, window_index(args)?)?;
        (element_center(&state, from)?, element_center(&state, to)?)
    } else {
        coordinate_pair.expect("checked above")
    };
    let previous = restore_target(args)?;
    activate(app.pid)?;
    run_jxa(
        POINTER_ACTION_SCRIPT,
        &[
            "drag".into(),
            from.0.to_string(),
            from.1.to_string(),
            to.0.to_string(),
            to.1.to_string(),
        ],
    )?;
    let result = action_result(&app, args, "drag")?;
    restore(previous);
    Ok(result)
}

fn keyboard_command(args: &[String], operation: &str) -> Result<Value, String> {
    let app = resolve_app(&required(args, "--app", "computer keyboard")?)?;
    let (script_operation, value, modifiers) = match operation {
        "type" => (
            "type",
            text_or_stdin(args, "--text", "--text-stdin", "computer type-text")?,
            Vec::new(),
        ),
        "paste" => (
            "paste",
            text_or_stdin(args, "--text", "--text-stdin", "computer paste-text")?,
            Vec::new(),
        ),
        "key" => (
            "key",
            required(args, "--key", "computer press-key")?,
            Vec::new(),
        ),
        "hotkey" => {
            let raw = required(args, "--key", "computer hotkey")?;
            let mut parts = raw
                .split('+')
                .map(|part| part.trim().to_ascii_lowercase())
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>();
            if parts.is_empty() {
                return Err("--key must contain a key combination.".into());
            }
            let key = parts.pop().unwrap();
            ("key", key, parts)
        }
        _ => return Err("Unsupported keyboard operation.".into()),
    };
    let previous = restore_target(args)?;
    activate(app.pid)?;
    let mut script_args = vec![app.pid.to_string(), script_operation.into(), value];
    script_args.extend(modifiers);
    run_jxa(KEYBOARD_ACTION_SCRIPT, &script_args)?;
    let result = action_result(&app, args, operation)?;
    restore(previous);
    Ok(result)
}

fn action_result(app: &ComputerApp, args: &[String], operation: &str) -> Result<Value, String> {
    if has(args, "--no-screenshot") {
        Ok(json!({"ok": true, "operation": operation, "app": app_json(app)}))
    } else {
        let mut state = snapshot(app, window_index(args)?)?;
        state["operation"] = Value::String(operation.into());
        state["ok"] = Value::Bool(true);
        state["screenshot"] = capture_screenshot().map_or(Value::Null, Value::String);
        Ok(state)
    }
}

fn capture_screenshot() -> Option<String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    let path = std::env::temp_dir().join(format!("suaegi-computer-{stamp}.png"));
    let status = Command::new("/usr/sbin/screencapture")
        .args(["-x", "-t", "png"])
        .arg(&path)
        .status()
        .ok()?;
    status
        .success()
        .then(|| path.to_string_lossy().into_owned())
}

fn restore_target(args: &[String]) -> Result<Option<i64>, String> {
    if has(args, "--restore-window") {
        frontmost_pid().map(Some)
    } else {
        Ok(None)
    }
}

fn restore(pid: Option<i64>) {
    if let Some(pid) = pid {
        let _ = activate(pid);
    }
}

fn frontmost_pid() -> Result<i64, String> {
    let value = run_jxa(
        r#"function run(){ObjC.import("AppKit");return JSON.stringify(Number($.NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier));}"#,
        &[],
    )?;
    value
        .as_i64()
        .ok_or_else(|| "Could not determine the frontmost app.".to_string())
}

fn activate(pid: i64) -> Result<(), String> {
    run_jxa(
        r#"function run(argv){ObjC.import("AppKit");const app=$.NSRunningApplication.runningApplicationWithProcessIdentifier(Number(argv[0]));if(!app)throw new Error("app_not_running");app.activateWithOptions($.NSApplicationActivateIgnoringOtherApps);return JSON.stringify({ok:true});}"#,
        &[pid.to_string()],
    )
    .map(|_| ())
}

fn run_jxa(script: &str, args: &[String]) -> Result<Value, String> {
    let output = Command::new("/usr/bin/osascript")
        .args(["-l", "JavaScript", "-e", script])
        .args(args)
        .output()
        .map_err(|error| format!("Could not run macOS Computer Use: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let kind = if stderr.contains("not authorized") || stderr.contains("-1743") {
            "Accessibility permission is required. Run `suaegi computer permissions --id accessibility`."
        } else if stderr.contains("window_not_found") {
            "The selected app window was not found."
        } else if stderr.contains("element_not_found") {
            "The selected accessibility element was not found."
        } else if stderr.contains("action_not_available") {
            "The requested accessibility action is not available on that element."
        } else if stderr.contains("app_not_running") {
            "The selected app is no longer running."
        } else {
            "macOS Computer Use could not complete the operation."
        };
        return Err(kind.into());
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|_| "macOS Computer Use returned invalid JSON.".to_string())
}

fn element_center(snapshot: &Value, index: usize) -> Result<(f64, f64), String> {
    let bounds = snapshot["elements"]
        .get(index)
        .and_then(|element| element.get("bounds"))
        .ok_or_else(|| format!("Element {index} has no screen bounds."))?;
    Ok((
        bounds["x"].as_f64().unwrap_or_default()
            + bounds["width"].as_f64().unwrap_or_default() / 2.0,
        bounds["y"].as_f64().unwrap_or_default()
            + bounds["height"].as_f64().unwrap_or_default() / 2.0,
    ))
}

fn window_index(args: &[String]) -> Result<usize, String> {
    if option(args, "--window-id")?.is_some() {
        return Err(
            "--window-id is unavailable on this macOS accessibility provider; use --window-index."
                .into(),
        );
    }
    Ok(option_usize(args, "--window-index")?.unwrap_or(0))
}

fn coordinates(args: &[String], x_flag: &str, y_flag: &str) -> Result<Option<(f64, f64)>, String> {
    let x = option_f64(args, x_flag)?;
    let y = option_f64(args, y_flag)?;
    match (x, y) {
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Ok(Some((x, y))),
        (None, None) => Ok(None),
        _ => Err(format!("{x_flag} and {y_flag} must be provided together.")),
    }
}

fn text_or_stdin(
    args: &[String],
    value_flag: &str,
    stdin_flag: &str,
    command: &str,
) -> Result<String, String> {
    let value = option(args, value_flag)?;
    let stdin = has(args, stdin_flag);
    if value.is_some() == stdin {
        return Err(format!(
            "{command} requires exactly one of {value_flag} or {stdin_flag}."
        ));
    }
    if let Some(value) = value {
        return Ok(value);
    }
    let mut value = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut value)
        .map_err(|error| format!("Could not read text from stdin: {error}"))?;
    Ok(value)
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

fn option_usize(args: &[String], flag: &str) -> Result<Option<usize>, String> {
    option(args, flag)?
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| format!("{flag} must be a non-negative integer."))
        })
        .transpose()
}

fn option_f64(args: &[String], flag: &str) -> Result<Option<f64>, String> {
    option(args, flag)?
        .map(|value| {
            value
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .ok_or_else(|| format!("{flag} must be a finite number."))
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
    fn computer_capabilities_cover_the_orca_action_surface() {
        let value = capabilities();
        for key in [
            "apps",
            "windows",
            "accessibilitySnapshot",
            "screenshot",
            "click",
            "secondaryActions",
            "scroll",
            "drag",
            "keyboard",
            "paste",
            "setValue",
        ] {
            assert_eq!(value["features"][key], true, "missing {key}");
        }
    }

    #[test]
    fn paired_coordinates_and_exclusive_text_inputs_are_validated() {
        assert_eq!(
            coordinates(
                &["--x".into(), "4".into(), "--y".into(), "8".into()],
                "--x",
                "--y"
            )
            .unwrap(),
            Some((4.0, 8.0))
        );
        assert!(coordinates(&["--x".into(), "4".into()], "--x", "--y").is_err());
        assert!(text_or_stdin(
            &["--text".into(), "a".into(), "--text-stdin".into()],
            "--text",
            "--text-stdin",
            "computer type-text"
        )
        .is_err());
    }

    #[test]
    fn jxa_scripts_are_syntactically_balanced_and_do_not_embed_user_values() {
        for script in [
            LIST_APPS_SCRIPT,
            LIST_WINDOWS_SCRIPT,
            SNAPSHOT_SCRIPT,
            ELEMENT_ACTION_SCRIPT,
            KEYBOARD_ACTION_SCRIPT,
            POINTER_ACTION_SCRIPT,
        ] {
            assert!(script.contains("function run("));
            assert!(!script.contains("${"));
        }
    }
}
