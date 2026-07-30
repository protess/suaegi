//! Orca-compatible in-app browser surface.
//!
//! The toolbar is rendered by Iced while the page itself is a native child
//! webview. On macOS Wry uses WKWebView, so pages retain native cookie,
//! accessibility, download, and developer-tools behavior instead of being a
//! screenshot or an external-browser shortcut.

use std::path::Path;

use iced::widget::{button, column, container, row, text_input, Space};
use iced::{Alignment, Element, Length};

use crate::i18n::text;
use crate::state::{AppState, Message};
use crate::theme;

const TAB_STRIP_HEIGHT: f32 = 34.0;
const NAVIGATION_HEIGHT: f32 = 42.0;
pub const TOOLBAR_HEIGHT: f32 = TAB_STRIP_HEIGHT + NAVIGATION_HEIGHT;
pub const STATUS_BAR_HEIGHT: f32 = 22.0;

// Record native WebKit dialog results after the delegate resumes JavaScript.
// The delegate, rather than this script, owns the pending completion handler so
// page execution remains suspended exactly as it does in Orca's browser.
const DIALOG_INIT_SCRIPT: &str = r#"
(() => {
  if (window.__suaegiDialogState?.installed) return;
  const state = window.__suaegiDialogState = {
    installed: true,
    nextId: 1,
    dialogs: []
  };
  const record = (type, message, defaultText, accepted, value) => {
    const dialog = {
      id: state.nextId++,
      type,
      message: String(message ?? ""),
      defaultText: defaultText == null ? null : String(defaultText),
      accepted,
      text: type === "prompt" && value != null ? String(value) : null
    };
    state.dialogs.push(dialog);
    if (state.dialogs.length > 50) state.dialogs.splice(0, state.dialogs.length - 50);
    return value;
  };
  const nativeAlert = window.alert.bind(window);
  const nativeConfirm = window.confirm.bind(window);
  const nativePrompt = window.prompt.bind(window);
  window.alert = message => {
    nativeAlert(message);
    record("alert", message, null, true, undefined);
  };
  window.confirm = message => {
    const value = nativeConfirm(message);
    return record("confirm", message, null, value, value);
  };
  window.prompt = (message, defaultText = "") => {
    const value = nativePrompt(message, defaultText);
    return record("prompt", message, defaultText, value != null, value);
  };
})();
"#;

#[derive(Debug, Clone, Copy)]
pub struct BrowserBounds {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

pub fn device_viewport(name: &str) -> Option<(f32, f32, f64, bool)> {
    let normalized = name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    match normalized.as_str() {
        "iphone15pro" => Some((393.0, 852.0, 3.0, true)),
        "iphone15promax" => Some((430.0, 932.0, 3.0, true)),
        "iphone14" | "iphone14pro" => Some((390.0, 844.0, 3.0, true)),
        "iphonese" | "iphonese3" => Some((375.0, 667.0, 2.0, true)),
        "pixel7" | "pixel8" => Some((412.0, 915.0, 2.625, true)),
        "galaxys23" | "samsunggalaxys23" => Some((360.0, 780.0, 3.0, true)),
        "ipadpro" | "ipadpro129" => Some((1024.0, 1366.0, 2.0, true)),
        "desktopchrome" | "desktop" => Some((1280.0, 720.0, 1.0, false)),
        _ => None,
    }
}

pub fn automation_script(action: &str, params: &serde_json::Value) -> Result<String, String> {
    let encoded = |value: &serde_json::Value| {
        serde_json::to_string(value)
            .map_err(|error| format!("Could not encode browser input: {error}"))
    };
    let reference = params
        .get("element")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let element_lookup = format!(
        r#"
const ref = {};
const match = /^@e(\d+)$/.exec(ref);
const el = match && window.__suaegiElements
  ? window.__suaegiElements[Number(match[1]) - 1]
  : null;
if (!el || !el.isConnected) return fail("browser_stale_ref", `Element reference is stale: ${{ref}}`);
"#,
        encoded(&serde_json::Value::String(reference.to_string()))?
    );
    let body = match action {
        "snapshot" => r#"
const visible = el => {
  const style = getComputedStyle(el);
  const rect = el.getBoundingClientRect();
  return style.visibility !== "hidden" && style.display !== "none" && rect.width > 0 && rect.height > 0;
};
const selector = [
  "a[href]", "button", "input", "textarea", "select", "summary",
  "[role=button]", "[role=link]", "[role=checkbox]", "[role=radio]",
  "[role=tab]", "[contenteditable=true]", "[tabindex]"
].join(",");
const elements = [...document.querySelectorAll(selector)].filter(visible).slice(0, 500);
window.__suaegiElements = elements;
const rows = elements.map((el, index) => {
  const rect = el.getBoundingClientRect();
  const label = (
    el.getAttribute("aria-label") ||
    el.getAttribute("title") ||
    el.getAttribute("placeholder") ||
    el.innerText ||
    el.value ||
    el.textContent ||
    ""
  ).replace(/\s+/g, " ").trim().slice(0, 300);
  return {
    ref: `@e${index + 1}`,
    tag: el.tagName.toLowerCase(),
    role: el.getAttribute("role"),
    label,
    type: el.getAttribute("type"),
    checked: "checked" in el ? Boolean(el.checked) : undefined,
    disabled: "disabled" in el ? Boolean(el.disabled) : undefined,
    x: Math.round(rect.x),
    y: Math.round(rect.y),
    width: Math.round(rect.width),
    height: Math.round(rect.height)
  };
});
return pass({
  url: location.href,
  title: document.title,
  elements: rows,
  text: (document.body?.innerText || "").replace(/\n{3,}/g, "\n\n").slice(0, 30000)
});
"#
        .to_string(),
        "click" => format!("{element_lookup}\nel.click(); return pass({{element: ref}});"),
        "dblclick" => format!(
            "{element_lookup}\nel.dispatchEvent(new MouseEvent(\"dblclick\", {{bubbles:true, cancelable:true, view:window}})); return pass({{element:ref}});"
        ),
        "focus" => format!("{element_lookup}\nel.focus(); return pass({{element: ref}});"),
        "hover" => format!(
            "{element_lookup}\nel.dispatchEvent(new MouseEvent(\"mouseover\", {{bubbles:true}})); return pass({{element: ref}});"
        ),
        "fill" | "type" => {
            let value = params
                .get("value")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| format!("browser {action} requires a string value"))?;
            let value = encoded(&serde_json::Value::String(value.to_string()))?;
            let target = if action == "fill" {
                element_lookup.clone()
            } else {
                r#"
const el = document.activeElement;
if (!el || el === document.body) return fail("browser_no_focus", "No editable browser element is focused.");
"#
                .to_string()
            };
            let assignment = if action == "fill" {
                "next"
            } else {
                "String(el.value || \"\") + next"
            };
            format!(
                r#"{target}
el.focus();
const next = {value};
const assigned = {assignment};
const prototype = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
const setter = Object.getOwnPropertyDescriptor(prototype, "value")?.set;
if (setter) setter.call(el, assigned); else el.value = assigned;
el.dispatchEvent(new InputEvent("input", {{bubbles:true, inputType:"insertText", data:next}}));
el.dispatchEvent(new Event("change", {{bubbles:true}}));
return pass({{element:ref, value:el.value}});
"#
            )
        }
        "clear" => format!(
            r#"{element_lookup}
el.focus();
el.value = "";
el.dispatchEvent(new InputEvent("input", {{bubbles:true, inputType:"deleteContentBackward"}}));
el.dispatchEvent(new Event("change", {{bubbles:true}}));
return pass({{element:ref}});
"#
        ),
        "select-all" => format!(
            r#"{element_lookup}
el.focus();
if (typeof el.select === "function") el.select();
else {{
  const selection = getSelection();
  const range = document.createRange();
  range.selectNodeContents(el);
  selection.removeAllRanges();
  selection.addRange(range);
}}
return pass({{element:ref}});
"#
        ),
        "select" => {
            let value = params
                .get("value")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "browser select requires a string value".to_string())?;
            format!(
                "{element_lookup}\nel.value = {}; el.dispatchEvent(new Event(\"input\", {{bubbles:true}})); el.dispatchEvent(new Event(\"change\", {{bubbles:true}})); return pass({{element:ref,value:el.value}});",
                encoded(&serde_json::Value::String(value.to_string()))?
            )
        }
        "check" | "uncheck" => format!(
            "{element_lookup}\nel.checked = {}; el.dispatchEvent(new Event(\"input\", {{bubbles:true}})); el.dispatchEvent(new Event(\"change\", {{bubbles:true}})); return pass({{element:ref,checked:Boolean(el.checked)}});",
            action == "check"
        ),
        "keypress" => {
            let key = params
                .get("key")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "browser keypress requires a key".to_string())?;
            let key = encoded(&serde_json::Value::String(key.to_string()))?;
            format!(
                r#"
const target = document.activeElement || document.body;
target.dispatchEvent(new KeyboardEvent("keydown", {{key:{key}, bubbles:true}}));
target.dispatchEvent(new KeyboardEvent("keyup", {{key:{key}, bubbles:true}}));
if ({key} === "Enter" && target.form) target.form.requestSubmit();
return pass({{key:{key}}});
"#
            )
        }
        "scroll" => {
            let direction = params
                .get("direction")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("down");
            let amount = params
                .get("amount")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(700.0)
                .abs()
                .min(100_000.0);
            let (x, y) = match direction {
                "up" => (0.0, -amount),
                "left" => (-amount, 0.0),
                "right" => (amount, 0.0),
                _ => (0.0, amount),
            };
            format!(
                "window.scrollBy({{left:{x},top:{y},behavior:\"instant\"}}); return pass({{x:window.scrollX,y:window.scrollY}});"
            )
        }
        "scroll-into-view" => format!(
            "{element_lookup}\nel.scrollIntoView({{block:\"center\",inline:\"nearest\",behavior:\"instant\"}}); return pass({{element:ref}});"
        ),
        "drag" => {
            let from = params
                .get("from")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "browser drag requires a source element".to_string())?;
            let to = params
                .get("to")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "browser drag requires a destination element".to_string())?;
            format!(
                r#"
const fromRef = {};
const toRef = {};
const indexOf = value => {{
  const match = /^@e(\d+)$/.exec(value);
  return match ? Number(match[1]) - 1 : -1;
}};
const source = window.__suaegiElements?.[indexOf(fromRef)];
const destination = window.__suaegiElements?.[indexOf(toRef)];
if (!source?.isConnected || !destination?.isConnected) return fail("browser_stale_ref", "A drag element reference is stale.");
const transfer = new DataTransfer();
source.dispatchEvent(new DragEvent("dragstart", {{bubbles:true,dataTransfer:transfer}}));
destination.dispatchEvent(new DragEvent("dragenter", {{bubbles:true,dataTransfer:transfer}}));
destination.dispatchEvent(new DragEvent("dragover", {{bubbles:true,cancelable:true,dataTransfer:transfer}}));
destination.dispatchEvent(new DragEvent("drop", {{bubbles:true,cancelable:true,dataTransfer:transfer}}));
source.dispatchEvent(new DragEvent("dragend", {{bubbles:true,dataTransfer:transfer}}));
return pass({{from:fromRef,to:toRef}});
"#,
                encoded(&serde_json::Value::String(from.to_string()))?,
                encoded(&serde_json::Value::String(to.to_string()))?
            )
        }
        "upload" => {
            let uploads = params
                .get("uploads")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| "browser upload requires encoded files".to_string())?;
            format!(
                r#"{element_lookup}
if (!(el instanceof HTMLInputElement) || el.type !== "file") return fail("browser_not_file_input","The target element is not a file input.");
const uploads = {};
const transfer = new DataTransfer();
for (const upload of uploads) {{
  const binary = atob(upload.data);
  const bytes = new Uint8Array(binary.length);
  for (let index=0; index<binary.length; index++) bytes[index] = binary.charCodeAt(index);
  transfer.items.add(new File([bytes],upload.name,{{type:upload.type || "application/octet-stream",lastModified:upload.lastModified || Date.now()}}));
}}
el.files = transfer.files;
el.dispatchEvent(new Event("input",{{bubbles:true}}));
el.dispatchEvent(new Event("change",{{bubbles:true}}));
return pass({{uploaded:el.files.length}});
"#,
                encoded(&serde_json::Value::Array(uploads.clone()))?
            )
        }
        "get" => {
            let what = params
                .get("what")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "browser get requires a property".to_string())?;
            let lookup = if reference.is_empty() {
                "const el = null;".to_string()
            } else {
                element_lookup.clone()
            };
            format!(
                r#"{lookup}
const what = {};
let result;
switch (what) {{
  case "url": result = location.href; break;
  case "title": result = document.title; break;
  case "count": result = window.__suaegiElements?.length || 0; break;
  case "text": result = el ? (el.innerText || el.textContent || "") : (document.body?.innerText || ""); break;
  case "html": result = el ? el.outerHTML : document.documentElement.outerHTML; break;
  case "value": result = el?.value ?? el?.getAttribute?.("value") ?? null; break;
  case "box": {{
    if (!el) return fail("browser_element_required", "box requires --element");
    const rect = el.getBoundingClientRect();
    result = {{x:rect.x,y:rect.y,width:rect.width,height:rect.height}};
    break;
  }}
  default:
    if (!el) return fail("browser_element_required", `${{what}} requires --element`);
    result = el[what] ?? el.getAttribute(what);
}}
return pass(result);
"#,
                encoded(&serde_json::Value::String(what.to_string()))?
            )
        }
        "is" => {
            let what = params
                .get("what")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "browser is requires a state".to_string())?;
            format!(
                r#"{element_lookup}
const what = {};
const style = getComputedStyle(el);
const rect = el.getBoundingClientRect();
const visible = style.visibility !== "hidden" && style.display !== "none" && rect.width > 0 && rect.height > 0;
const result = what === "visible" ? visible
  : what === "enabled" ? !Boolean(el.disabled) && el.getAttribute("aria-disabled") !== "true"
  : what === "checked" ? Boolean(el.checked) || el.getAttribute("aria-checked") === "true"
  : what === "focused" ? document.activeElement === el
  : what === "editable" ? !Boolean(el.disabled) && !Boolean(el.readOnly) && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName))
  : null;
if (result === null) return fail("browser_invalid_state", `Unsupported element state: ${{what}}`);
return pass(result);
"#,
                encoded(&serde_json::Value::String(what.to_string()))?
            )
        }
        "insert-text" => {
            let value = params
                .get("value")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "browser inserttext requires text".to_string())?;
            format!(
                r#"
const el = document.activeElement;
if (!el || el === document.body) return fail("browser_no_focus", "No editable browser element is focused.");
const next = {};
if (el.isContentEditable) {{
  document.execCommand("insertText", false, next);
}} else {{
  const start = el.selectionStart ?? String(el.value || "").length;
  const end = el.selectionEnd ?? start;
  el.setRangeText(next, start, end, "end");
  el.dispatchEvent(new InputEvent("input", {{bubbles:true,inputType:"insertText",data:next}}));
}}
return pass({{value:el.value ?? el.textContent}});
"#,
                encoded(&serde_json::Value::String(value.to_string()))?
            )
        }
        "highlight" => format!(
            r#"{element_lookup}
const previous = el.style.outline;
el.style.outline = "3px solid #ff7a18";
el.scrollIntoView({{block:"center",inline:"nearest"}});
setTimeout(() => {{ el.style.outline = previous; }}, 1500);
return pass({{element:ref}});
"#
        ),
        "mouse-move" | "mouse-down" | "mouse-up" => {
            let x = params
                .get("x")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(-1.0);
            let y = params
                .get("y")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(-1.0);
            let button = params
                .get("button")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("left");
            let event = match action {
                "mouse-down" => "mousedown",
                "mouse-up" => "mouseup",
                _ => "mousemove",
            };
            format!(
                r#"
const x = {x}, y = {y};
const target = document.elementFromPoint(x,y) || document.body;
const buttons = {{left:0,middle:1,right:2}};
target.dispatchEvent(new MouseEvent("{}", {{
  bubbles:true,cancelable:true,clientX:x,clientY:y,button:buttons[{}] ?? 0,view:window
}}));
return pass({{x,y,target:target.tagName?.toLowerCase()}});
"#,
                event,
                encoded(&serde_json::Value::String(button.to_string()))?
            )
        }
        "mouse-wheel" => {
            let dx = params
                .get("dx")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0);
            let dy = params
                .get("dy")
                .and_then(serde_json::Value::as_f64)
                .ok_or_else(|| "mouse wheel requires dy".to_string())?;
            format!(
                r#"
const target = document.elementFromPoint(innerWidth/2,innerHeight/2) || document.body;
target.dispatchEvent(new WheelEvent("wheel", {{bubbles:true,cancelable:true,deltaX:{dx},deltaY:{dy},view:window}}));
window.scrollBy({{left:{dx},top:{dy},behavior:"instant"}});
return pass({{x:scrollX,y:scrollY}});
"#
            )
        }
        "geolocation" => {
            let latitude = params
                .get("latitude")
                .and_then(serde_json::Value::as_f64)
                .ok_or_else(|| "geolocation requires latitude".to_string())?;
            let longitude = params
                .get("longitude")
                .and_then(serde_json::Value::as_f64)
                .ok_or_else(|| "geolocation requires longitude".to_string())?;
            let accuracy = params
                .get("accuracy")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(1.0)
                .max(0.0);
            format!(
                r#"
const coordinates = Object.freeze({{latitude:{latitude},longitude:{longitude},accuracy:{accuracy},altitude:null,altitudeAccuracy:null,heading:null,speed:null}});
const position = () => ({{coords:coordinates,timestamp:Date.now()}});
Object.defineProperty(navigator,"geolocation",{{configurable:true,value:{{
  getCurrentPosition(success) {{ queueMicrotask(() => success(position())); }},
  watchPosition(success) {{ queueMicrotask(() => success(position())); return 1; }},
  clearWatch() {{}}
}}}});
return pass({{latitude:{latitude},longitude:{longitude},accuracy:{accuracy}}});
"#
            )
        }
        "viewport" | "set-device" => {
            let (width, height, scale, mobile, name) = if action == "set-device" {
                let name = params
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "set device requires a name".to_string())?;
                let (width, height, scale, mobile) = device_viewport(name)
                    .ok_or_else(|| format!("Unknown browser device: {name}"))?;
                (width as f64, height as f64, scale, mobile, Some(name))
            } else {
                (
                    params
                        .get("width")
                        .and_then(serde_json::Value::as_f64)
                        .ok_or_else(|| "viewport requires width".to_string())?,
                    params
                        .get("height")
                        .and_then(serde_json::Value::as_f64)
                        .ok_or_else(|| "viewport requires height".to_string())?,
                    params
                        .get("deviceScaleFactor")
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or(1.0),
                    params
                        .get("mobile")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                    None,
                )
            };
            if !width.is_finite()
                || !height.is_finite()
                || !scale.is_finite()
                || width <= 0.0
                || height <= 0.0
                || scale <= 0.0
            {
                return Err("Browser viewport values must be positive finite numbers.".into());
            }
            format!(
                r#"
const viewport = {{width:{width},height:{height},deviceScaleFactor:{scale},mobile:{mobile},name:{}}};
window.__suaegiViewport = viewport;
for (const [key,value] of [["devicePixelRatio",viewport.deviceScaleFactor],["maxTouchPoints",viewport.mobile ? 5 : 0]]) {{
  try {{ Object.defineProperty(key === "maxTouchPoints" ? navigator : window,key,{{configurable:true,get:()=>value}}); }} catch {{}}
}}
try {{ Object.defineProperty(navigator,"userAgentData",{{configurable:true,value:undefined}}); }} catch {{}}
return pass(viewport);
"#,
                encoded(&name.map_or(serde_json::Value::Null, |name| {
                    serde_json::Value::String(name.to_string())
                }))?
            )
        }
        "set-offline" => {
            let offline = params
                .get("state")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);
            format!(
                r#"
window.__suaegiOffline = {};
if (!window.__suaegiOriginalFetch) {{
  window.__suaegiOriginalFetch = window.fetch.bind(window);
  window.fetch = (...args) => window.__suaegiOffline
    ? Promise.reject(new TypeError("Failed to fetch (Suaegi offline emulation)"))
    : window.__suaegiOriginalFetch(...args);
}}
Object.defineProperty(navigator,"onLine",{{configurable:true,get:()=>!window.__suaegiOffline}});
window.dispatchEvent(new Event(window.__suaegiOffline ? "offline" : "online"));
return pass({{offline:window.__suaegiOffline}});
"#,
                offline
            )
        }
        "set-headers" | "set-credentials" => {
            let headers = if action == "set-headers" {
                params
                    .get("headers")
                    .cloned()
                    .ok_or_else(|| "set headers requires a JSON object".to_string())?
            } else {
                let user = params
                    .get("user")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "set credentials requires user".to_string())?;
                let password = params
                    .get("pass")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "set credentials requires pass".to_string())?;
                serde_json::json!({
                    "Authorization": format!("Basic {}", base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        format!("{user}:{password}")
                    ))
                })
            };
            format!(
                r#"
window.__suaegiExtraHeaders = {};
if (!window.__suaegiHeaderFetch) {{
  window.__suaegiHeaderFetch = window.fetch.bind(window);
  window.fetch = (input, init={{}}) => {{
    const headers = new Headers(init.headers || (input instanceof Request ? input.headers : undefined));
    for (const [key,value] of Object.entries(window.__suaegiExtraHeaders || {{}})) headers.set(key,String(value));
    return window.__suaegiHeaderFetch(input,{{...init,headers}});
  }};
}}
return pass({{headers:window.__suaegiExtraHeaders}});
"#,
                encoded(&headers)?
            )
        }
        "set-preferences" | "set-media" => {
            let color_scheme = params
                .get("colorScheme")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("no-preference");
            let reduced_motion = params
                .get("reducedMotion")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("no-preference");
            format!(
                r#"
window.__suaegiPreferences = {{colorScheme:{},reducedMotion:{}}};
if (!window.__suaegiMatchMedia) {{
  window.__suaegiMatchMedia = window.matchMedia.bind(window);
  window.matchMedia = query => {{
    const original = window.__suaegiMatchMedia(query);
    const prefs = window.__suaegiPreferences;
    const matches = query.includes("prefers-color-scheme")
      ? query.includes(prefs.colorScheme)
      : query.includes("prefers-reduced-motion")
        ? query.includes(prefs.reducedMotion)
        : original.matches;
    return Object.assign(original,{{matches}});
  }};
}}
return pass(window.__suaegiPreferences);
"#,
                encoded(&serde_json::Value::String(color_scheme.to_string()))?,
                encoded(&serde_json::Value::String(reduced_motion.to_string()))?
            )
        }
        "clipboard-read" => {
            "return navigator.clipboard.readText().then(value => pass(value), error => fail(\"browser_clipboard_error\", String(error)));".into()
        }
        "clipboard-write" => {
            let value = params
                .get("value")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "clipboard write requires text".to_string())?;
            format!(
                "return navigator.clipboard.writeText({}).then(() => pass({{written:true}}), error => fail(\"browser_clipboard_error\", String(error)));",
                encoded(&serde_json::Value::String(value.to_string()))?
            )
        }
        "capture-start" => r#"
const runtime = window.__suaegiCapture ||= {active:false,console:[],network:[],installed:false};
runtime.console = []; runtime.network = []; runtime.active = true;
if (!runtime.installed) {
  runtime.installed = true;
  for (const level of ["log","info","warn","error","debug"]) {
    const original = console[level].bind(console);
    console[level] = (...args) => {
      if (runtime.active) runtime.console.push({level,text:args.map(value => {
        try { return typeof value === "string" ? value : JSON.stringify(value); } catch { return String(value); }
      }).join(" "),timestamp:Date.now()});
      if (runtime.console.length > 5000) runtime.console.splice(0,runtime.console.length-5000);
      return original(...args);
    };
  }
  const originalFetch = window.fetch.bind(window);
  window.fetch = async (...args) => {
    const started = Date.now();
    const url = String(args[0] instanceof Request ? args[0].url : args[0]);
    const method = String(args[1]?.method || (args[0] instanceof Request ? args[0].method : "GET")).toUpperCase();
    try {
      const response = await originalFetch(...args);
      if (runtime.active) runtime.network.push({url,method,status:response.status,durationMs:Date.now()-started});
      return response;
    } catch (error) {
      if (runtime.active) runtime.network.push({url,method,error:String(error),durationMs:Date.now()-started});
      throw error;
    }
  };
}
return pass({capturing:true});
"#
        .into(),
        "capture-stop" => {
            "if (window.__suaegiCapture) window.__suaegiCapture.active=false; return pass({capturing:false});".into()
        }
        "intercept-enable" => {
            let patterns = params
                .get("patterns")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_else(|| vec![serde_json::Value::String("*".into())]);
            format!(
                r#"
const state = window.__suaegiIntercept ||= {{enabled:false,patterns:["*"],requests:[],pending:[],installed:false,nextId:1}};
state.enabled = true;
state.patterns = {};
state.requests = [];
if (!state.installed) {{
  state.installed = true;
  state.originalFetch = window.fetch.bind(window);
  window.fetch = (input, init={{}}) => {{
    const url = String(input instanceof Request ? input.url : input);
    const method = String(init.method || (input instanceof Request ? input.method : "GET")).toUpperCase();
    const escape = value => value.replace(/[.+^${{}}()|[\]\\]/g,"\\$&").replace(/\*/g,".*").replace(/\?/g,".");
    const matched = state.enabled && state.patterns.some(pattern => new RegExp(`^${{escape(String(pattern))}}$`).test(url));
    if (!matched) return state.originalFetch(input,init);
    const id = String(state.nextId++);
    state.requests.push({{id,url,method,resourceType:"fetch",timestamp:Date.now()}});
    if (state.requests.length > 500) state.requests.splice(0,state.requests.length-500);
    return new Promise(resolve => state.pending.push(() => resolve(new Response("",{{status:499,statusText:"Released by Suaegi"}}))));
  }};
}}
return pass({{enabled:true,patterns:state.patterns}});
"#,
                encoded(&serde_json::Value::Array(patterns))?
            )
        }
        "intercept-disable" => r#"
const state = window.__suaegiIntercept;
if (state) {
  state.enabled = false;
  for (const release of state.pending.splice(0)) release();
}
return pass({enabled:false});
"#
        .into(),
        "intercept-list" => {
            "return pass({requests:[...(window.__suaegiIntercept?.requests||[])]});".into()
        }
        "console" | "network" => {
            let limit = params
                .get("limit")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(100)
                .clamp(1, 5000);
            let key = action;
            format!(
                "const rows=(window.__suaegiCapture?.{key}||[]).slice(-{limit}); return pass({{{key}:rows}});"
            )
        }
        "find" => {
            let locator = params
                .get("locator")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "browser find requires a locator".to_string())?;
            let value = params
                .get("value")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "browser find requires a value".to_string())?;
            let action = params
                .get("action")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "browser find requires an action".to_string())?;
            let text = params
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            format!(
                r#"
const locator = {};
const needle = {};
const action = {};
const actionText = {};
const all = [...document.querySelectorAll("*")];
const normalized = value => String(value || "").replace(/\s+/g, " ").trim();
const exact = value => normalized(value) === needle;
let el = null;
if (locator === "css") el = document.querySelector(needle);
else if (locator === "text") el = all.find(node => exact(node.innerText || node.textContent));
else if (locator === "role") el = all.find(node => node.getAttribute("role") === needle);
else if (locator === "label") el = all.find(node => exact(node.getAttribute("aria-label")) || (node.labels && [...node.labels].some(label => exact(label.innerText))));
else if (locator === "placeholder") el = all.find(node => exact(node.getAttribute("placeholder")));
else if (locator === "testid") el = document.querySelector(`[data-testid="${{CSS.escape(needle)}}"]`);
else if (locator === "title") el = all.find(node => exact(node.getAttribute("title")));
if (!el) return fail("browser_not_found", `No element matched ${{locator}}=${{needle}}`);
if (action === "click") el.click();
else if (action === "focus") el.focus();
else if (action === "fill") {{
  el.focus(); el.value = actionText;
  el.dispatchEvent(new InputEvent("input", {{bubbles:true,data:actionText}}));
  el.dispatchEvent(new Event("change", {{bubbles:true}}));
}} else if (action === "check") {{
  el.checked = true; el.dispatchEvent(new Event("change", {{bubbles:true}}));
}} else if (action === "uncheck") {{
  el.checked = false; el.dispatchEvent(new Event("change", {{bubbles:true}}));
}} else if (action !== "get") return fail("browser_invalid_action", `Unsupported find action: ${{action}}`);
return pass({{locator, value:needle, action, tag:el.tagName.toLowerCase(), text:normalized(el.innerText || el.textContent)}});
"#,
                encoded(&serde_json::Value::String(locator.to_string()))?,
                encoded(&serde_json::Value::String(value.to_string()))?,
                encoded(&serde_json::Value::String(action.to_string()))?,
                encoded(&serde_json::Value::String(text.to_string()))?,
            )
        }
        action
            if matches!(
                action,
                "storage-local-get"
                    | "storage-local-set"
                    | "storage-local-clear"
                    | "storage-session-get"
                    | "storage-session-set"
                    | "storage-session-clear"
            ) =>
        {
            let session = action.starts_with("storage-session");
            let storage = if session {
                "sessionStorage"
            } else {
                "localStorage"
            };
            if action.ends_with("-clear") {
                format!("{storage}.clear(); return pass({{cleared:true}});")
            } else {
                let key = params
                    .get("key")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "browser storage requires a key".to_string())?;
                let key = encoded(&serde_json::Value::String(key.to_string()))?;
                if action.ends_with("-get") {
                    format!("return pass({storage}.getItem({key}));")
                } else {
                    let value = params
                        .get("value")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| "browser storage set requires a value".to_string())?;
                    let value = encoded(&serde_json::Value::String(value.to_string()))?;
                    format!("{storage}.setItem({key},{value}); return pass({{key:{key},value:{value}}});")
                }
            }
        }
        "eval" => {
            let expression = params
                .get("expression")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "browser eval requires an expression".to_string())?;
            format!(
                "return Promise.resolve((0,eval)({})).then(pass);",
                encoded(&serde_json::Value::String(expression.to_string()))?
            )
        }
        "dialog-accept" | "dialog-dismiss" => {
            r#"return fail("dialog_native_required", "JavaScript dialogs are handled by the native WebKit delegate.");"#.to_string()
        }
        _ => return Err(format!("Unsupported browser action: {action}")),
    };
    Ok(format!(
        r#"(() => {{
const pass = result => ({{ok:true,result}});
const fail = (code, error) => ({{ok:false,code,error}});
try {{
{body}
}} catch (error) {{
  return fail("browser_script_error", String(error?.message || error));
}}
}})()"#
    ))
}

pub fn address_input_id() -> iced::widget::Id {
    iced::widget::Id::new("browser-address")
}

#[derive(Clone)]
pub struct CookieImportBundle(Vec<ImportedCookie>);

impl std::fmt::Debug for CookieImportBundle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CookieImportBundle")
            .field("cookies", &self.0.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
struct ImportedCookie {
    domain: String,
    path: String,
    secure: bool,
    http_only: bool,
    expires_unix: Option<i64>,
    name: String,
    value: String,
}

#[derive(Clone)]
pub struct DetectedBrowserProfile {
    label: String,
    cookies_path: std::path::PathBuf,
    source: DetectedCookieSource,
}

#[derive(Clone)]
enum DetectedCookieSource {
    Chromium { keychain_service: String },
    Firefox,
    Safari,
}

impl DetectedBrowserProfile {
    pub fn label(&self) -> &str {
        &self.label
    }
}

impl std::fmt::Debug for DetectedBrowserProfile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DetectedBrowserProfile")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

pub fn detected_browser_profiles() -> Vec<DetectedBrowserProfile> {
    #[cfg(target_os = "macos")]
    {
        let Some(home) = dirs::home_dir() else {
            return Vec::new();
        };
        let support = home.join("Library/Application Support");
        let browsers = [
            (
                "Chrome",
                support.join("Google/Chrome"),
                "Chrome Safe Storage",
            ),
            (
                "Edge",
                support.join("Microsoft Edge"),
                "Microsoft Edge Safe Storage",
            ),
            (
                "Brave",
                support.join("BraveSoftware/Brave-Browser"),
                "Brave Safe Storage",
            ),
            ("Arc", support.join("Arc/User Data"), "Arc Safe Storage"),
            ("Comet", support.join("Comet"), "Comet Safe Storage"),
        ];
        let mut detected = Vec::new();
        for (browser, root, service) in browsers {
            let Ok(entries) = std::fs::read_dir(&root) else {
                continue;
            };
            for entry in entries.flatten() {
                let file_name = entry.file_name().to_string_lossy().to_string();
                if file_name != "Default" && !file_name.starts_with("Profile ") {
                    continue;
                }
                let profile = entry.path();
                let cookies_path = [profile.join("Network/Cookies"), profile.join("Cookies")]
                    .into_iter()
                    .find(|path| path.is_file());
                if let Some(cookies_path) = cookies_path {
                    detected.push(DetectedBrowserProfile {
                        label: format!("{browser} · {file_name}"),
                        cookies_path,
                        source: DetectedCookieSource::Chromium {
                            keychain_service: service.into(),
                        },
                    });
                }
            }
        }
        let firefox_profiles = support.join("Firefox/Profiles");
        if let Ok(entries) = std::fs::read_dir(firefox_profiles) {
            for entry in entries.flatten() {
                let cookies_path = entry.path().join("cookies.sqlite");
                if !cookies_path.is_file() {
                    continue;
                }
                detected.push(DetectedBrowserProfile {
                    label: format!("Firefox · {}", entry.file_name().to_string_lossy()),
                    cookies_path,
                    source: DetectedCookieSource::Firefox,
                });
            }
        }
        let safari_candidates = [
            home.join("Library/Cookies/Cookies.binarycookies"),
            home.join(
                "Library/Containers/com.apple.Safari/Data/Library/Cookies/Cookies.binarycookies",
            ),
        ];
        if let Some(cookies_path) = safari_candidates.into_iter().find(|path| path.is_file()) {
            detected.push(DetectedBrowserProfile {
                label: "Safari · Default".into(),
                cookies_path,
                source: DetectedCookieSource::Safari,
            });
        }
        detected.sort_by(|left, right| left.label.cmp(&right.label));
        detected
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

pub async fn import_detected_browser_profile(
    profile: DetectedBrowserProfile,
) -> Result<Option<CookieImportBundle>, String> {
    tokio::task::spawn_blocking(move || match profile.source.clone() {
        #[cfg(target_os = "macos")]
        DetectedCookieSource::Chromium { .. } => import_chromium_cookies(profile),
        #[cfg(target_os = "macos")]
        DetectedCookieSource::Firefox => import_firefox_cookies(profile),
        #[cfg(target_os = "macos")]
        DetectedCookieSource::Safari => import_safari_cookies(profile),
        #[cfg(not(target_os = "macos"))]
        _ => Err("Browser profile import is not available on this platform.".into()),
    })
    .await
    .map_err(|error| format!("Browser cookie import task failed: {error}"))?
    .map(Some)
}

#[cfg(target_os = "macos")]
fn import_chromium_cookies(profile: DetectedBrowserProfile) -> Result<CookieImportBundle, String> {
    use serde::Deserialize;
    use sha1::Sha1;

    #[derive(Deserialize)]
    struct CookieRow {
        host_key: String,
        path: String,
        is_secure: i64,
        is_httponly: i64,
        expires_utc: i64,
        name: String,
        value: String,
        encrypted_hex: String,
    }

    let query = "SELECT host_key,path,is_secure,is_httponly,expires_utc,name,value,hex(encrypted_value) AS encrypted_hex FROM cookies";
    let output = std::process::Command::new("/usr/bin/sqlite3")
        .args(["-readonly", "-json"])
        .arg(&profile.cookies_path)
        .arg(query)
        .output()
        .map_err(|_| "Could not run sqlite3 to read the browser profile.".to_string())?;
    if !output.status.success() {
        return Err(
            "The browser cookie database could not be read. Close the browser and retry.".into(),
        );
    }
    let rows: Vec<CookieRow> = serde_json::from_slice(&output.stdout)
        .map_err(|_| "The browser cookie database returned invalid data.".to_string())?;

    let DetectedCookieSource::Chromium { keychain_service } = &profile.source else {
        return Err("The selected profile is not a Chromium profile.".into());
    };
    let safe_storage_password = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-w", "-s", keychain_service])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .trim_end_matches(['\r', '\n'])
                .as_bytes()
                .to_vec()
        });
    let key = safe_storage_password.as_deref().map(|password| {
        let mut key = [0_u8; 16];
        pbkdf2::pbkdf2_hmac::<Sha1>(password, b"saltysalt", 1003, &mut key);
        key
    });

    let mut cookies = Vec::new();
    let mut encrypted_unavailable = 0_usize;
    for row in rows {
        if row.host_key.is_empty() || row.name.is_empty() {
            continue;
        }
        let value = if !row.value.is_empty() {
            Some(row.value)
        } else if let Some(key) = key {
            if !row.encrypted_hex.starts_with("763130") && !row.encrypted_hex.starts_with("763131")
            {
                encrypted_unavailable += 1;
                continue;
            }
            decrypt_chromium_cookie(&row.host_key, &row.encrypted_hex, &key)
        } else {
            encrypted_unavailable += 1;
            None
        };
        let Some(value) = value else {
            continue;
        };
        let unix_seconds = row
            .expires_utc
            .checked_div(1_000_000)
            .and_then(|seconds| seconds.checked_sub(11_644_473_600))
            .filter(|seconds| *seconds > 0);
        cookies.push(ImportedCookie {
            domain: row.host_key,
            path: if row.path.starts_with('/') {
                row.path
            } else {
                "/".into()
            },
            secure: row.is_secure != 0,
            http_only: row.is_httponly != 0,
            expires_unix: unix_seconds,
            name: row.name,
            value,
        });
        if cookies.len() >= 20_000 {
            break;
        }
    }
    if cookies.is_empty() {
        return Err(if encrypted_unavailable > 0 {
            "No cookies could be decrypted. Allow Keychain access and retry.".into()
        } else {
            "No importable cookies were found in this browser profile.".into()
        });
    }
    Ok(CookieImportBundle(cookies))
}

#[cfg(target_os = "macos")]
fn import_firefox_cookies(profile: DetectedBrowserProfile) -> Result<CookieImportBundle, String> {
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct CookieRow {
        host: String,
        path: String,
        is_secure: i64,
        is_http_only: i64,
        expiry: i64,
        name: String,
        value: String,
    }

    let query = "SELECT host,path,isSecure AS is_secure,isHttpOnly AS is_http_only,expiry,name,value FROM moz_cookies";
    let output = std::process::Command::new("/usr/bin/sqlite3")
        .args(["-readonly", "-json"])
        .arg(&profile.cookies_path)
        .arg(query)
        .output()
        .map_err(|_| "Could not run sqlite3 to read the Firefox profile.".to_string())?;
    if !output.status.success() {
        return Err(
            "The Firefox cookie database could not be read. Close Firefox and retry.".into(),
        );
    }
    let rows: Vec<CookieRow> = serde_json::from_slice(&output.stdout)
        .map_err(|_| "The Firefox cookie database returned invalid data.".to_string())?;
    let mut cookies = Vec::new();
    for row in rows {
        if row.host.is_empty() || row.name.is_empty() {
            continue;
        }
        cookies.push(ImportedCookie {
            domain: row.host.chars().take(255).collect(),
            path: if row.path.starts_with('/') {
                row.path.chars().take(2048).collect()
            } else {
                "/".into()
            },
            secure: row.is_secure != 0,
            http_only: row.is_http_only != 0,
            expires_unix: (row.expiry > 0).then_some(row.expiry),
            name: row.name.chars().take(512).collect(),
            value: row.value.chars().take(16_384).collect(),
        });
        if cookies.len() >= 20_000 {
            break;
        }
    }
    if cookies.is_empty() {
        return Err("No importable cookies were found in this Firefox profile.".into());
    }
    Ok(CookieImportBundle(cookies))
}

#[cfg(target_os = "macos")]
fn import_safari_cookies(profile: DetectedBrowserProfile) -> Result<CookieImportBundle, String> {
    let bytes = std::fs::read(&profile.cookies_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            "macOS denied access to Safari cookies. Grant Full Disk Access to Suaegi in System Settings → Privacy & Security → Full Disk Access.".to_string()
        } else {
            "Could not read Safari cookies.".to_string()
        }
    })?;
    let bundle = decode_safari_binary_cookies(&bytes);
    if bundle.0.is_empty() {
        Err("No importable cookies were found in Safari.".into())
    } else {
        Ok(bundle)
    }
}

#[cfg(target_os = "macos")]
fn decode_safari_binary_cookies(bytes: &[u8]) -> CookieImportBundle {
    const MAC_EPOCH_DELTA: f64 = 978_307_200.0;

    fn u32_be(bytes: &[u8], offset: usize) -> Option<u32> {
        Some(u32::from_be_bytes(
            bytes.get(offset..offset + 4)?.try_into().ok()?,
        ))
    }
    fn u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
        Some(u32::from_le_bytes(
            bytes.get(offset..offset + 4)?.try_into().ok()?,
        ))
    }
    fn f64_le(bytes: &[u8], offset: usize) -> Option<f64> {
        Some(f64::from_le_bytes(
            bytes.get(offset..offset + 8)?.try_into().ok()?,
        ))
    }
    fn c_string(bytes: &[u8], offset: usize, end: usize) -> Option<String> {
        if offset >= end || end > bytes.len() {
            return None;
        }
        let relative_end = bytes.get(offset..end)?.iter().position(|byte| *byte == 0)?;
        std::str::from_utf8(bytes.get(offset..offset + relative_end)?)
            .ok()
            .map(str::to_string)
    }

    if bytes.get(..4) != Some(b"cook") {
        return CookieImportBundle(Vec::new());
    }
    let Some(page_count) = u32_be(bytes, 4).map(|count| count as usize) else {
        return CookieImportBundle(Vec::new());
    };
    let page_table_end = match page_count
        .checked_mul(4)
        .and_then(|size| 8_usize.checked_add(size))
    {
        Some(end) if end <= bytes.len() => end,
        _ => return CookieImportBundle(Vec::new()),
    };
    let mut page_sizes = Vec::with_capacity(page_count);
    for index in 0..page_count {
        let Some(size) = u32_be(bytes, 8 + index * 4).map(|size| size as usize) else {
            return CookieImportBundle(Vec::new());
        };
        page_sizes.push(size);
    }

    let mut cursor = page_table_end;
    let mut imported = Vec::new();
    for page_size in page_sizes {
        let Some(page_end) = cursor.checked_add(page_size) else {
            break;
        };
        let Some(page) = bytes.get(cursor..page_end) else {
            break;
        };
        cursor = page_end;
        if u32_be(page, 0) != Some(0x0000_0100) {
            continue;
        }
        let Some(cookie_count) = u32_le(page, 4).map(|count| count as usize) else {
            continue;
        };
        if cookie_count
            .checked_mul(4)
            .and_then(|size| 8_usize.checked_add(size))
            .is_none_or(|end| end > page.len())
        {
            continue;
        }
        for index in 0..cookie_count {
            let Some(offset) = u32_le(page, 8 + index * 4).map(|offset| offset as usize) else {
                continue;
            };
            let Some(record) = page.get(offset..) else {
                continue;
            };
            let Some(size) = u32_le(record, 0)
                .map(|size| (size as usize).min(record.len()))
                .filter(|size| *size >= 48)
            else {
                continue;
            };
            let flags = u32_le(record, 8).unwrap_or(0);
            let Some(domain) = u32_le(record, 16)
                .and_then(|value| c_string(record, value as usize, size))
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let Some(name) = u32_le(record, 20)
                .and_then(|value| c_string(record, value as usize, size))
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let path = u32_le(record, 24)
                .and_then(|value| c_string(record, value as usize, size))
                .filter(|value| value.starts_with('/'))
                .unwrap_or_else(|| "/".into());
            let value = u32_le(record, 28)
                .and_then(|value| c_string(record, value as usize, size))
                .unwrap_or_default();
            let expires_unix = f64_le(record, 40)
                .filter(|value| value.is_finite() && *value > 0.0)
                .map(|value| (value + MAC_EPOCH_DELTA).round() as i64);
            imported.push(ImportedCookie {
                domain: domain.chars().take(255).collect(),
                path: path.chars().take(2048).collect(),
                secure: flags & 1 != 0,
                http_only: flags & 4 != 0,
                expires_unix,
                name: name.chars().take(512).collect(),
                value: value.chars().take(16_384).collect(),
            });
            if imported.len() >= 20_000 {
                return CookieImportBundle(imported);
            }
        }
    }
    CookieImportBundle(imported)
}

#[cfg(target_os = "macos")]
fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            Some(((high << 4) | low) as u8)
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn decrypt_chromium_cookie(host: &str, encrypted_hex: &str, key: &[u8; 16]) -> Option<String> {
    use aes::Aes128;
    use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
    use sha2::{Digest, Sha256};

    let mut encrypted = decode_hex(encrypted_hex)?;
    if !encrypted.starts_with(b"v10") && !encrypted.starts_with(b"v11") {
        return None;
    }
    encrypted.drain(..3);
    let iv = [b' '; 16];
    let decrypted = cbc::Decryptor::<Aes128>::new(key.into(), (&iv).into())
        .decrypt_padded_mut::<Pkcs7>(&mut encrypted)
        .ok()?;
    let host_digest = Sha256::digest(host.as_bytes());
    let decrypted = if decrypted.len() >= 32 && decrypted[..32] == host_digest[..] {
        &decrypted[32..]
    } else {
        decrypted
    };
    String::from_utf8(decrypted.to_vec()).ok()
}

pub async fn pick_cookie_file() -> Result<Option<CookieImportBundle>, String> {
    let Some(path) = rfd::AsyncFileDialog::new()
        .add_filter("Cookie files", &["txt", "cookies"])
        .pick_file()
        .await
        .map(|handle| handle.path().to_path_buf())
    else {
        return Ok(None);
    };
    tokio::task::spawn_blocking(move || parse_netscape_cookie_file(&path))
        .await
        .map_err(|error| format!("Cookie import task failed: {error}"))?
        .map(Some)
}

fn parse_netscape_cookie_file(path: &Path) -> Result<CookieImportBundle, String> {
    const LIMIT: u64 = 16 * 1024 * 1024;
    let metadata = std::fs::metadata(path)
        .map_err(|_| "Could not read the selected cookie file.".to_string())?;
    if metadata.len() > LIMIT {
        return Err("Cookie file is larger than the 16 MB safety limit.".into());
    }
    let contents = std::fs::read_to_string(path)
        .map_err(|_| "Cookie file must be readable UTF-8 text.".to_string())?;
    let mut cookies = Vec::new();
    for raw in contents.lines() {
        let (http_only, line) = raw
            .strip_prefix("#HttpOnly_")
            .map_or((false, raw), |line| (true, line));
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 7 {
            continue;
        }
        let domain = fields[0].trim();
        let path = fields[2].trim();
        let name = fields[5].trim();
        if domain.is_empty() || name.is_empty() || !domain.contains('.') {
            continue;
        }
        cookies.push(ImportedCookie {
            domain: domain.chars().take(255).collect(),
            path: if path.starts_with('/') {
                path.chars().take(2048).collect()
            } else {
                "/".into()
            },
            secure: fields[3].eq_ignore_ascii_case("TRUE"),
            http_only,
            expires_unix: fields[4]
                .parse::<i64>()
                .ok()
                .filter(|timestamp| *timestamp > 0),
            name: name.chars().take(512).collect(),
            value: fields[6].chars().take(16_384).collect(),
        });
        if cookies.len() >= 20_000 {
            break;
        }
    }
    if cookies.is_empty() {
        return Err("No valid Netscape-format cookies were found.".into());
    }
    Ok(CookieImportBundle(cookies))
}

pub fn view(state: &AppState) -> Element<'_, Message> {
    let address = text_input("Search or enter address", state.browser_address_draft())
        .id(address_input_id())
        .on_input(Message::BrowserAddressChanged)
        .on_submit(Message::BrowserNavigateRequested)
        .padding([6, 10])
        .size(12)
        .width(Length::Fill);

    let nav_button = |label: &'static str, message| {
        button(text(label).size(14))
            .on_press(message)
            .padding([4, 7])
            .style(theme::ghost_button)
    };

    let navigation = container(
        row![
            nav_button("‹", Message::BrowserBack),
            nav_button("›", Message::BrowserForward),
            nav_button("↻", Message::BrowserReload),
            address,
            button(
                text(format!(
                    "{}%",
                    state.ui_settings().browser_default_zoom_percent
                ))
                .size(10)
            )
            .on_press(Message::BrowserZoomReset)
            .padding([5, 7])
            .style(theme::ghost_button),
            nav_button("↗", Message::BrowserOpenExternal),
            nav_button("×", Message::BrowserClosed),
        ]
        .spacing(4)
        .align_y(Alignment::Center),
    )
    .height(Length::Fixed(NAVIGATION_HEIGHT))
    .padding([5, 7])
    .style(theme::top_bar);

    // Keep the native browser toolbar bounded. A horizontally scrolling Row of
    // buttons can force Iced into an unbounded layout pass when a WKWebView is
    // attached below it. Show a small window around the active tab and expose
    // the immediately adjacent hidden tabs as paging controls.
    const MAX_VISIBLE_TABS: usize = 5;
    let all_tabs = state.browser_tabs();
    let active_index = state
        .active_browser_tab_id()
        .and_then(|active| all_tabs.iter().position(|tab| tab.id == active))
        .unwrap_or(0);
    let visible_count = all_tabs.len().min(MAX_VISIBLE_TABS);
    let mut visible_start = active_index.saturating_sub(visible_count / 2);
    visible_start = visible_start.min(all_tabs.len().saturating_sub(visible_count));
    let visible_end = visible_start + visible_count;

    let mut tabs = row![].spacing(2).align_y(Alignment::Center);
    if visible_start > 0 {
        let previous = &all_tabs[visible_start - 1];
        tabs = tabs.push(
            button(text(format!("‹ {}", visible_start)).size(10))
                .on_press(Message::BrowserTabSelected(previous.id.clone()))
                .padding([5, 7])
                .style(theme::ghost_button),
        );
    }
    for tab in &all_tabs[visible_start..visible_end] {
        let active = state.active_browser_tab_id() == Some(tab.id.as_str());
        let raw_label = if tab.title.trim().is_empty() {
            if tab.url == suaegi_browser_url::ORCA_BROWSER_BLANK_URL {
                "New tab"
            } else {
                tab.url.as_str()
            }
        } else {
            tab.title.as_str()
        };
        let mut label = raw_label.chars().take(12).collect::<String>();
        if raw_label.chars().count() > 12 {
            label.push('…');
        }
        let select = button(text(label).size(11))
            .on_press(Message::BrowserTabSelected(tab.id.clone()))
            .padding([5, 8])
            .style(if active {
                theme::selected_button
            } else {
                theme::ghost_button
            });
        let close = button(text("×").size(11))
            .on_press(Message::BrowserTabClosed(tab.id.clone()))
            .padding([5, 5])
            .style(theme::ghost_button);
        tabs = tabs.push(row![select, close].spacing(0).align_y(Alignment::Center));
    }
    if visible_end < all_tabs.len() {
        let next = &all_tabs[visible_end];
        tabs = tabs.push(
            button(text(format!("{} ›", all_tabs.len() - visible_end)).size(10))
                .on_press(Message::BrowserTabSelected(next.id.clone()))
                .padding([5, 7])
                .style(theme::ghost_button),
        );
    }
    tabs = tabs.push(
        button(text("+").size(14))
            .on_press(Message::BrowserTabCreated)
            .padding([4, 8])
            .style(theme::ghost_button),
    );
    let tab_strip = container(tabs)
        .height(Length::Fixed(TAB_STRIP_HEIGHT))
        .padding([2, 6])
        .style(theme::top_bar);

    let body: Element<'_, Message> = if let Some(error) = state.browser_error() {
        container(
            column![
                text("Unable to open page").size(16),
                text(error).size(11).color(theme::MUTED),
                button(text("Open in default browser").size(11))
                    .on_press(Message::BrowserOpenExternal)
                    .padding([6, 9])
                    .style(theme::ghost_button),
            ]
            .spacing(8)
            .align_x(Alignment::Center),
        )
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::editor_surface)
        .into()
    } else {
        // The native child view is positioned over this reserved region.
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::editor_surface)
            .into()
    };

    column![tab_strip, navigation, body]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

#[cfg(target_os = "macos")]
mod native {
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};
    use std::ffi::c_void;
    use std::ptr::NonNull;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Bool, NSObject, ProtocolObject};
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSBitmapImageFileType, NSBitmapImageRep, NSBitmapImageRepPropertyKey,
        NSModalResponse, NSModalResponseOK, NSOpenPanel,
    };
    use objc2_foundation::{
        NSArray, NSData, NSDictionary, NSError, NSJSONSerialization, NSJSONWritingOptions,
        NSObjectProtocol, NSString, NSURL,
    };
    use objc2_web_kit::{
        WKContentWorld, WKFrameInfo, WKMediaCaptureType, WKOpenPanelParameters,
        WKPermissionDecision, WKSecurityOrigin, WKUIDelegate, WKWebView,
    };
    use raw_window_handle::{
        AppKitWindowHandle, HandleError, HasWindowHandle, RawWindowHandle, WindowHandle,
    };
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    use wry::dpi::{LogicalPosition, LogicalSize};
    use wry::{
        Rect, WebView, WebViewBuilder, WebViewBuilderExtDarwin, WebViewExtDarwin, WebViewExtMacOS,
    };

    use super::DIALOG_INIT_SCRIPT;

    thread_local! {
        static BROWSER_VIEWS: RefCell<BrowserViews> = RefCell::new(BrowserViews::default());
        static DOWNLOADS: RefCell<Downloads> = RefCell::new(Downloads::default());
    }

    enum DialogCompletion {
        Alert(block2::RcBlock<dyn Fn()>),
        Confirm(block2::RcBlock<dyn Fn(Bool)>),
        Prompt(block2::RcBlock<dyn Fn(*mut NSString)>),
    }

    struct PendingDialog {
        dialog_type: &'static str,
        message: String,
        default_text: Option<String>,
        completion: DialogCompletion,
    }

    #[derive(Default)]
    struct BrowserDialogDelegateIvars {
        pending: RefCell<Option<PendingDialog>>,
    }

    define_class!(
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        #[ivars = BrowserDialogDelegateIvars]
        struct BrowserDialogDelegate;

        unsafe impl NSObjectProtocol for BrowserDialogDelegate {}

        unsafe impl WKUIDelegate for BrowserDialogDelegate {
            #[unsafe(method(webView:runJavaScriptAlertPanelWithMessage:initiatedByFrame:completionHandler:))]
            fn run_javascript_alert(
                &self,
                _webview: &WKWebView,
                message: &NSString,
                _frame: &WKFrameInfo,
                completion: &block2::DynBlock<dyn Fn()>,
            ) {
                self.replace_pending(PendingDialog {
                    dialog_type: "alert",
                    message: message.to_string(),
                    default_text: None,
                    completion: DialogCompletion::Alert(completion.copy()),
                });
            }

            #[unsafe(method(webView:runJavaScriptConfirmPanelWithMessage:initiatedByFrame:completionHandler:))]
            fn run_javascript_confirm(
                &self,
                _webview: &WKWebView,
                message: &NSString,
                _frame: &WKFrameInfo,
                completion: &block2::DynBlock<dyn Fn(Bool)>,
            ) {
                self.replace_pending(PendingDialog {
                    dialog_type: "confirm",
                    message: message.to_string(),
                    default_text: None,
                    completion: DialogCompletion::Confirm(completion.copy()),
                });
            }

            #[unsafe(method(webView:runJavaScriptTextInputPanelWithPrompt:defaultText:initiatedByFrame:completionHandler:))]
            fn run_javascript_prompt(
                &self,
                _webview: &WKWebView,
                prompt: &NSString,
                default_text: Option<&NSString>,
                _frame: &WKFrameInfo,
                completion: &block2::DynBlock<dyn Fn(*mut NSString)>,
            ) {
                self.replace_pending(PendingDialog {
                    dialog_type: "prompt",
                    message: prompt.to_string(),
                    default_text: default_text.map(ToString::to_string),
                    completion: DialogCompletion::Prompt(completion.copy()),
                });
            }

            #[unsafe(method(webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:))]
            fn request_media_capture_permission(
                &self,
                _webview: &WKWebView,
                _origin: &WKSecurityOrigin,
                _frame: &WKFrameInfo,
                _capture_type: WKMediaCaptureType,
                decision_handler: &block2::DynBlock<dyn Fn(WKPermissionDecision)>,
            ) {
                decision_handler.call((WKPermissionDecision::Grant,));
            }

            #[unsafe(method(webView:runOpenPanelWithParameters:initiatedByFrame:completionHandler:))]
            fn run_file_upload_panel(
                &self,
                _webview: &WKWebView,
                parameters: &WKOpenPanelParameters,
                _frame: &WKFrameInfo,
                completion: &block2::DynBlock<dyn Fn(*mut NSArray<NSURL>)>,
            ) {
                let Some(mtm) = MainThreadMarker::new() else {
                    completion.call((std::ptr::null_mut(),));
                    return;
                };
                let panel = NSOpenPanel::openPanel(mtm);
                panel.setCanChooseFiles(true);
                // SAFETY: WebKit supplies a live WKOpenPanelParameters object
                // for the duration of this delegate call.
                unsafe {
                    panel.setAllowsMultipleSelection(parameters.allowsMultipleSelection());
                    panel.setCanChooseDirectories(parameters.allowsDirectories());
                }
                let response: NSModalResponse = panel.runModal();
                if response == NSModalResponseOK {
                    let urls = panel.URLs();
                    completion.call((Retained::as_ptr(&urls).cast_mut(),));
                } else {
                    completion.call((std::ptr::null_mut(),));
                }
            }
        }
    );

    impl BrowserDialogDelegate {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let delegate = mtm
                .alloc::<Self>()
                .set_ivars(BrowserDialogDelegateIvars::default());
            // SAFETY: NSObject's init signature and the allocated class agree.
            unsafe { msg_send![super(delegate), init] }
        }

        fn replace_pending(&self, pending: PendingDialog) {
            if let Some(previous) = self.ivars().pending.replace(Some(pending)) {
                resolve_dialog_completion(previous.completion, false, None);
            }
        }

        fn resolve(&self, accept: bool, text: Option<&str>) -> Result<Value, String> {
            let pending = self
                .ivars()
                .pending
                .borrow_mut()
                .take()
                .ok_or_else(|| "No browser JavaScript dialog is currently open.".to_string())?;
            let prompt_text = (pending.dialog_type == "prompt" && accept).then(|| {
                text.unwrap_or_else(|| pending.default_text.as_deref().unwrap_or(""))
                    .to_string()
            });
            let result = json!({
                "handled": true,
                "type": pending.dialog_type,
                "message": pending.message,
                "defaultText": pending.default_text,
                "accepted": accept || pending.dialog_type == "alert",
                "text": prompt_text.clone(),
            });
            resolve_dialog_completion(pending.completion, accept, prompt_text.as_deref());
            Ok(result)
        }
    }

    fn resolve_dialog_completion(completion: DialogCompletion, accept: bool, text: Option<&str>) {
        match completion {
            DialogCompletion::Alert(completion) => completion.call(()),
            DialogCompletion::Confirm(completion) => completion.call((Bool::new(accept),)),
            DialogCompletion::Prompt(completion) => {
                if accept {
                    let text = NSString::from_str(text.unwrap_or_default());
                    completion.call((Retained::as_ptr(&text).cast_mut(),));
                } else {
                    completion.call((std::ptr::null_mut(),));
                }
            }
        }
    }

    type DownloadCallback = Box<dyn FnOnce(Result<std::path::PathBuf, String>)>;

    #[derive(Default)]
    struct Downloads {
        next_id: u64,
        queued: VecDeque<(u64, std::path::PathBuf, DownloadCallback)>,
        active: Vec<(u64, String, std::path::PathBuf, DownloadCallback)>,
    }

    #[derive(Default)]
    struct BrowserViews {
        active: Option<String>,
        views: HashMap<String, BrowserRuntime>,
    }

    struct BrowserRuntime {
        webview: WebView,
        dialog_delegate: Retained<BrowserDialogDelegate>,
        profile_id: String,
        requested_url: String,
        viewport_override: Option<(f32, f32)>,
    }

    struct ParentViewHandle(NonNull<c_void>);

    fn profile_store_identifier(profile_id: &str) -> [u8; 16] {
        let digest = Sha256::digest(profile_id.as_bytes());
        let mut identifier = [0_u8; 16];
        identifier.copy_from_slice(&digest[..16]);
        identifier
    }

    impl HasWindowHandle for ParentViewHandle {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let raw = RawWindowHandle::AppKit(AppKitWindowHandle::new(self.0));
            // SAFETY: the pointer is the retained contentView of the live
            // NSWindow. Wry only borrows it while attaching the WKWebView.
            Ok(unsafe { WindowHandle::borrow_raw(raw) })
        }
    }

    fn rect(left: f32, top: f32, width: f32, height: f32) -> Rect {
        Rect {
            position: LogicalPosition::new(left as f64, top as f64).into(),
            size: LogicalSize::new(width.max(1.0) as f64, height.max(1.0) as f64).into(),
        }
    }

    fn download_destination(suggested: &std::path::Path) -> Option<std::path::PathBuf> {
        let directory = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("suaegi")
            .join("downloads");
        std::fs::create_dir_all(&directory).ok()?;
        let file_name = suggested
            .file_name()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| std::ffi::OsStr::new("download"));
        let initial = directory.join(file_name);
        if !initial.exists() {
            return Some(initial);
        }
        let stem = initial
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("download");
        let extension = initial.extension().and_then(|value| value.to_str());
        for suffix in 1..10_000 {
            let name = extension.map_or_else(
                || format!("{stem} ({suffix})"),
                |extension| format!("{stem} ({suffix}).{extension}"),
            );
            let candidate = directory.join(name);
            if !candidate.exists() {
                return Some(candidate);
            }
        }
        None
    }

    pub fn ensure(
        page_id: &str,
        url: &str,
        profile_id: &str,
        bounds: super::BrowserBounds,
        zoom: f64,
    ) -> Result<(), String> {
        let bounds = rect(bounds.left, bounds.top, bounds.width, bounds.height);
        let mut stale = None;
        let already_exists = BROWSER_VIEWS.with(|slot| {
            let mut collection = slot.borrow_mut();
            if collection
                .views
                .get(page_id)
                .is_some_and(|runtime| runtime.profile_id != profile_id)
            {
                stale = collection.views.remove(page_id);
            }
            if collection.views.contains_key(page_id) {
                if collection.active.as_deref() != Some(page_id) {
                    if let Some(previous) = collection
                        .active
                        .as_ref()
                        .and_then(|active| collection.views.get(active))
                    {
                        previous
                            .webview
                            .set_visible(false)
                            .map_err(|error| error.to_string())?;
                    }
                }
                collection.active = Some(page_id.to_string());
                let runtime = collection
                    .views
                    .get_mut(page_id)
                    .expect("the browser view was checked above");
                let bounds = runtime.viewport_override.map_or(bounds, |(width, height)| {
                    rect(
                        bounds.position.to_logical::<f64>(1.0).x as f32,
                        bounds.position.to_logical::<f64>(1.0).y as f32,
                        width,
                        height,
                    )
                });
                runtime
                    .webview
                    .set_bounds(bounds)
                    .map_err(|error| error.to_string())?;
                runtime
                    .webview
                    .set_visible(true)
                    .map_err(|error| error.to_string())?;
                runtime
                    .webview
                    .zoom(zoom)
                    .map_err(|error| error.to_string())?;
                if runtime.requested_url != url {
                    runtime
                        .webview
                        .load_url(url)
                        .map_err(|error| error.to_string())?;
                    runtime.requested_url = url.to_string();
                }
                Ok::<bool, String>(true)
            } else {
                Ok::<bool, String>(false)
            }
        })?;
        // Dropping a WKWebView can dispatch AppKit work. Never do that while
        // the thread-local collection is mutably borrowed.
        drop(stale);
        if already_exists {
            return Ok(());
        }
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| "The browser must be created on the macOS main thread.".to_string())?;
        let app = NSApplication::sharedApplication(mtm);
        let window = app
            .keyWindow()
            .or_else(|| app.mainWindow())
            // A window can be visible and fully usable while neither
            // `keyWindow` nor `mainWindow` is set (for example immediately
            // after LaunchServices opens the app in the background). Iced has
            // already registered its NSWindow in the application list by this
            // point, so fall back to that list instead of permanently leaving
            // the browser surface in an "open but uninitialized" state.
            .or_else(|| app.windows().firstObject())
            .ok_or_else(|| "Suaegi window is not ready yet.".to_string())?;
        let content = window
            .contentView()
            .ok_or_else(|| "Suaegi window has no content view.".to_string())?;
        let parent = ParentViewHandle(NonNull::from(&*content).cast());

        let mut builder = WebViewBuilder::new()
            .with_url(url)
            .with_bounds(bounds)
            .with_accept_first_mouse(true)
            .with_initialization_script(DIALOG_INIT_SCRIPT)
            // WebKit's default destination is the protected user Downloads
            // folder. On recent macOS releases requesting a sandbox extension
            // for that folder can block the main run loop when the app has not
            // yet received Downloads-folder consent. Use the app-owned data
            // directory so downloads remain functional without freezing the UI.
            .with_download_started_handler(|url, destination| {
                let reserved =
                    DOWNLOADS.with(|downloads| downloads.borrow_mut().queued.pop_front());
                if let Some((id, path, callback)) = reserved {
                    *destination = path.clone();
                    DOWNLOADS.with(|downloads| {
                        downloads
                            .borrow_mut()
                            .active
                            .push((id, url, path, callback));
                    });
                    return true;
                }
                let Some(path) = download_destination(destination) else {
                    return false;
                };
                *destination = path;
                true
            })
            .with_download_completed_handler(|url, _path, success| {
                let completed = DOWNLOADS.with(|downloads| {
                    let mut downloads = downloads.borrow_mut();
                    let index = downloads
                        .active
                        .iter()
                        .position(|(_, active_url, _, _)| active_url == &url)?;
                    Some(downloads.active.remove(index))
                });
                if let Some((_, _, path, callback)) = completed {
                    callback(if success {
                        Ok(path)
                    } else {
                        Err("Browser download did not complete.".into())
                    });
                }
            })
            .with_devtools(cfg!(debug_assertions));
        if profile_id != "default" {
            builder = builder.with_data_store_identifier(profile_store_identifier(profile_id));
        }
        let webview = builder
            .build_as_child(&parent)
            .map_err(|error| error.to_string())?;
        let dialog_delegate = BrowserDialogDelegate::new(mtm);
        let native_webview = webview.webview();
        // SAFETY: both objects are retained for the BrowserRuntime lifetime and
        // WebKit invokes WKUIDelegate only on the main thread.
        unsafe {
            native_webview.setUIDelegate(Some(ProtocolObject::from_ref(&*dialog_delegate)));
        }
        webview.zoom(zoom).map_err(|error| error.to_string())?;
        webview.focus().map_err(|error| error.to_string())?;
        BROWSER_VIEWS.with(|slot| {
            let mut collection = slot.borrow_mut();
            if let Some(previous) = collection
                .active
                .as_ref()
                .and_then(|active| collection.views.get(active))
            {
                let _ = previous.webview.set_visible(false);
            }
            collection.views.insert(
                page_id.to_string(),
                BrowserRuntime {
                    webview,
                    dialog_delegate,
                    profile_id: profile_id.to_string(),
                    requested_url: url.to_string(),
                    viewport_override: None,
                },
            );
            collection.active = Some(page_id.to_string());
        });
        Ok(())
    }

    pub fn handle_dialog(accept: bool, text: Option<&str>) -> Result<Value, String> {
        BROWSER_VIEWS.with(|slot| {
            let collection = slot.borrow();
            let active = collection
                .active
                .as_ref()
                .ok_or_else(|| "Browser view is not initialized.".to_string())?;
            let runtime = collection
                .views
                .get(active)
                .ok_or_else(|| "Active browser view is missing.".to_string())?;
            runtime.dialog_delegate.resolve(accept, text)
        })
    }

    fn with_webview<T>(f: impl FnOnce(&WebView) -> Result<T, wry::Error>) -> Result<T, String> {
        BROWSER_VIEWS.with(|slot| {
            let collection = slot.borrow();
            let active = collection
                .active
                .as_ref()
                .ok_or_else(|| "Browser view is not initialized.".to_string())?
                .clone();
            let webview = &collection
                .views
                .get(&active)
                .ok_or_else(|| "Active browser view is missing.".to_string())?
                .webview;
            f(webview).map_err(|error| error.to_string())
        })
    }

    pub fn load(url: &str) -> Result<(), String> {
        BROWSER_VIEWS.with(|slot| {
            let mut collection = slot.borrow_mut();
            let active = collection
                .active
                .as_ref()
                .ok_or_else(|| "Browser view is not initialized.".to_string())?
                .clone();
            let runtime = collection
                .views
                .get_mut(&active)
                .ok_or_else(|| "Active browser view is missing.".to_string())?;
            runtime
                .webview
                .load_url(url)
                .map_err(|error| error.to_string())?;
            runtime.requested_url = url.to_string();
            Ok(())
        })
    }

    pub fn back() -> Result<(), String> {
        with_webview(|webview| webview.evaluate_script("history.back()"))
    }

    pub fn forward() -> Result<(), String> {
        with_webview(|webview| webview.evaluate_script("history.forward()"))
    }

    pub fn reload() -> Result<(), String> {
        with_webview(WebView::reload)
    }

    pub fn evaluate(
        script: String,
        callback: impl Fn(String) + Send + 'static,
    ) -> Result<(), String> {
        BROWSER_VIEWS.with(|slot| {
            let collection = slot.borrow();
            let active = collection
                .active
                .as_ref()
                .ok_or_else(|| "Browser view is not initialized.".to_string())?;
            let runtime = collection
                .views
                .get(active)
                .ok_or_else(|| "Active browser view is missing.".to_string())?;
            let webview = runtime.webview.webview();
            let mtm = MainThreadMarker::new().ok_or_else(|| {
                "Browser automation must run on the macOS main thread.".to_string()
            })?;
            let content_world = unsafe { WKContentWorld::pageWorld(mtm) };
            let function_body = NSString::from_str(&format!("return await ({script});"));
            let callback = std::sync::Mutex::new(callback);
            let handler =
                block2::RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
                    let encoded = if !error.is_null() {
                        // SAFETY: WebKit retains the NSError for the duration of
                        // the completion handler.
                        let description = unsafe { &*error }.localizedDescription().to_string();
                        serde_json::json!({
                            "ok": false,
                            "error": description,
                        })
                        .to_string()
                    } else if value.is_null() {
                        "null".to_string()
                    } else {
                        // `callAsyncJavaScript` bridges JavaScript values to
                        // Foundation objects. Serialize fragments as well as
                        // dictionaries/arrays so primitive eval results keep the
                        // same JSON contract as Wry's synchronous callback.
                        unsafe {
                            NSJSONSerialization::dataWithJSONObject_options_error(
                                &*value,
                                NSJSONWritingOptions::FragmentsAllowed,
                            )
                        }
                        .map(|data| String::from_utf8_lossy(&copy_data(&data)).into_owned())
                        .unwrap_or_else(|error| {
                            serde_json::json!({
                                "ok": false,
                                "error": error.localizedDescription().to_string(),
                            })
                            .to_string()
                        })
                    };
                    if let Ok(callback) = callback.lock() {
                        callback(encoded);
                    }
                });
            // SAFETY: `webview` and the page content world are live on the
            // main thread. WebKit copies the completion block and resolves
            // returned promises before invoking it.
            unsafe {
                webview.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
                    &function_body,
                    None::<&NSDictionary<NSString, AnyObject>>,
                    None,
                    &content_world,
                    Some(&handler),
                );
            }
            Ok(())
        })
    }

    fn copy_data(data: &NSData) -> Vec<u8> {
        let length = data.length();
        let mut bytes = vec![0_u8; length];
        if let Some(pointer) = NonNull::new(bytes.as_mut_ptr().cast()) {
            // SAFETY: `bytes` owns a writable allocation of exactly `length`
            // bytes for the duration of this copy.
            unsafe { data.getBytes_length(pointer, length) };
        }
        bytes
    }

    pub fn pdf(
        callback: impl FnOnce(Result<Vec<u8>, String>) + Send + 'static,
    ) -> Result<(), String> {
        let callback = std::sync::Mutex::new(Some(callback));
        BROWSER_VIEWS.with(|slot| {
            let collection = slot.borrow();
            let active = collection
                .active
                .as_ref()
                .ok_or_else(|| "Browser view is not initialized.".to_string())?;
            let runtime = collection
                .views
                .get(active)
                .ok_or_else(|| "Active browser view is missing.".to_string())?;
            let webview = runtime.webview.webview();
            let handler = block2::RcBlock::new(
                move |data: *mut NSData, _error: *mut objc2_foundation::NSError| {
                    let result = if data.is_null() {
                        Err("WebKit could not create a PDF for this page.".to_string())
                    } else {
                        // SAFETY: WebKit guarantees that a non-null `data`
                        // pointer remains valid for the completion block.
                        Ok(copy_data(unsafe { &*data }))
                    };
                    if let Some(callback) = callback.lock().ok().and_then(|mut slot| slot.take()) {
                        callback(result);
                    }
                },
            );
            // SAFETY: `webview` is a live WKWebView on the main thread and
            // WebKit copies the completion block for the asynchronous call.
            unsafe {
                webview.createPDFWithConfiguration_completionHandler(None, &handler);
            }
            Ok(())
        })
    }

    pub fn screenshot(
        format: &str,
        callback: impl FnOnce(Result<Vec<u8>, String>) + Send + 'static,
    ) -> Result<(), String> {
        let jpeg = format == "jpeg";
        let callback = std::sync::Mutex::new(Some(callback));
        BROWSER_VIEWS.with(|slot| {
            let collection = slot.borrow();
            let active = collection
                .active
                .as_ref()
                .ok_or_else(|| "Browser view is not initialized.".to_string())?;
            let runtime = collection
                .views
                .get(active)
                .ok_or_else(|| "Active browser view is missing.".to_string())?;
            let webview = runtime.webview.webview();
            let handler = block2::RcBlock::new(
                move |image: *mut objc2_app_kit::NSImage,
                      _error: *mut objc2_foundation::NSError| {
                    let result = if image.is_null() {
                        Err("WebKit could not capture the browser viewport.".to_string())
                    } else {
                        // SAFETY: WebKit owns the image for the duration of
                        // this completion block.
                        let image = unsafe { &*image };
                        image
                            .TIFFRepresentation()
                            .and_then(|tiff| NSBitmapImageRep::imageRepWithData(&tiff))
                            .and_then(|bitmap| {
                                let properties = NSDictionary::<
                                    NSBitmapImageRepPropertyKey,
                                    AnyObject,
                                >::dictionary();
                                // SAFETY: an empty properties dictionary is
                                // valid for both PNG and JPEG conversion.
                                unsafe {
                                    bitmap.representationUsingType_properties(
                                        if jpeg {
                                            NSBitmapImageFileType::JPEG
                                        } else {
                                            NSBitmapImageFileType::PNG
                                        },
                                        &properties,
                                    )
                                }
                            })
                            .map(|data| copy_data(&data))
                            .ok_or_else(|| "Could not encode the browser screenshot.".to_string())
                    };
                    if let Some(callback) = callback.lock().ok().and_then(|mut slot| slot.take()) {
                        callback(result);
                    }
                },
            );
            // SAFETY: `webview` is a live WKWebView on the main thread and
            // WebKit copies the completion block for the asynchronous call.
            unsafe {
                webview.takeSnapshotWithConfiguration_completionHandler(None, &handler);
            }
            Ok(())
        })
    }

    pub fn full_screenshot(
        format: &str,
        callback: impl FnOnce(Result<Vec<u8>, String>) + Send + 'static,
    ) -> Result<(), String> {
        let callback = std::sync::Arc::new(std::sync::Mutex::new(Some(callback)));
        let format = format.to_string();
        evaluate(
            "Math.max(document.documentElement.scrollHeight,document.body?.scrollHeight||0,window.innerHeight)"
                .to_string(),
            move |raw| {
                let height = serde_json::from_str::<f64>(&raw)
                    .ok()
                    .filter(|value| value.is_finite() && *value > 0.0)
                    .map(|value| value.min(32_000.0))
                    .unwrap_or(1.0);
                let original = with_webview(WebView::bounds);
                let original = match original {
                    Ok(bounds) => bounds,
                    Err(error) => {
                        if let Some(callback) =
                            callback.lock().ok().and_then(|mut slot| slot.take())
                        {
                            callback(Err(error));
                        }
                        return;
                    }
                };
                let logical = original.size.to_logical::<f64>(1.0);
                let expanded = Rect {
                    position: original.position,
                    size: LogicalSize::new(logical.width, height).into(),
                };
                let resized = with_webview(|webview| webview.set_bounds(expanded));
                if let Err(error) = resized {
                    if let Some(callback) = callback.lock().ok().and_then(|mut slot| slot.take()) {
                        callback(Err(error));
                    }
                    return;
                }
                let callback_after_layout = callback.clone();
                let format_after_layout = format.clone();
                if let Err(error) = evaluate(
                    "document.documentElement.offsetHeight".to_string(),
                    move |_| {
                        let callback_after_capture = callback_after_layout.clone();
                        if let Err(error) = screenshot(&format_after_layout, move |result| {
                            let _ = with_webview(|webview| webview.set_bounds(original));
                            if let Some(callback) = callback_after_capture
                                .lock()
                                .ok()
                                .and_then(|mut slot| slot.take())
                            {
                                callback(result);
                            }
                        }) {
                            let _ = with_webview(|webview| webview.set_bounds(original));
                            if let Some(callback) = callback_after_layout
                                .lock()
                                .ok()
                                .and_then(|mut slot| slot.take())
                            {
                                callback(Err(error));
                            }
                        }
                    },
                ) {
                    let _ = with_webview(|webview| webview.set_bounds(original));
                    if let Some(callback) = callback.lock().ok().and_then(|mut slot| slot.take()) {
                        callback(Err(error));
                    }
                }
            },
        )
    }

    pub fn find_in_page() -> Result<(), String> {
        with_webview(|webview| {
            webview.evaluate_script(
                "(() => { const q = window.prompt('Find in page'); if (q) window.find(q); })();",
            )
        })
    }

    pub fn set_zoom(zoom: f64) -> Result<(), String> {
        with_webview(|webview| webview.zoom(zoom))
    }

    pub fn current_url() -> Option<String> {
        BROWSER_VIEWS.with(|slot| {
            let mut collection = slot.borrow_mut();
            let active = collection.active.clone()?;
            let runtime = collection.views.get_mut(&active)?;
            let webview = runtime.webview.webview();
            // Wry's macOS `WebView::url` unwraps WKWebView.URL internally.
            // WebKit legitimately returns nil while converting a navigation
            // response into a download, so query the native property without
            // the unwrap to keep the browser location timer crash-safe.
            let url = unsafe { webview.URL() }?;
            let url = url.absoluteString().map(|value| value.to_string())?;
            runtime.requested_url.clone_from(&url);
            Some(url)
        })
    }

    pub fn current_title() -> Option<String> {
        BROWSER_VIEWS.with(|slot| {
            let collection = slot.borrow();
            let active = collection.active.as_ref()?;
            let webview = collection.views.get(active)?.webview.webview();
            // SAFETY: title is queried from the live WKWebView on the main
            // thread; the returned NSString is retained by objc2.
            unsafe { webview.title() }.map(|title| title.to_string())
        })
    }

    pub fn resize(left: f32, top: f32, width: f32, height: f32) -> Result<(), String> {
        BROWSER_VIEWS.with(|slot| {
            let collection = slot.borrow();
            let active = collection
                .active
                .as_ref()
                .ok_or_else(|| "Browser view is not initialized.".to_string())?;
            let runtime = collection
                .views
                .get(active)
                .ok_or_else(|| "Active browser view is missing.".to_string())?;
            let (width, height) = runtime.viewport_override.unwrap_or((width, height));
            runtime
                .webview
                .set_bounds(rect(left, top, width, height))
                .map_err(|error| error.to_string())
        })
    }

    pub fn set_viewport(width: f32, height: f32) -> Result<(), String> {
        if !width.is_finite() || !height.is_finite() || width < 1.0 || height < 1.0 {
            return Err("Browser viewport dimensions must be positive finite numbers.".into());
        }
        BROWSER_VIEWS.with(|slot| {
            let mut collection = slot.borrow_mut();
            let active = collection
                .active
                .clone()
                .ok_or_else(|| "Browser view is not initialized.".to_string())?;
            let runtime = collection
                .views
                .get_mut(&active)
                .ok_or_else(|| "Active browser view is missing.".to_string())?;
            let current = runtime
                .webview
                .bounds()
                .map_err(|error| error.to_string())?;
            let position = current.position.to_logical::<f64>(1.0);
            runtime.viewport_override = Some((width, height));
            runtime
                .webview
                .set_bounds(rect(position.x as f32, position.y as f32, width, height))
                .map_err(|error| error.to_string())
        })
    }

    pub fn begin_download(
        destination: std::path::PathBuf,
        callback: impl FnOnce(Result<std::path::PathBuf, String>) + 'static,
    ) -> Result<u64, String> {
        if !destination.is_absolute() {
            return Err("Browser download destination must be absolute.".into());
        }
        let parent = destination
            .parent()
            .ok_or_else(|| "Browser download destination has no parent directory.".to_string())?;
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create download directory: {error}"))?;
        if destination.exists() {
            return Err(format!(
                "Browser download destination already exists: {}",
                destination.display()
            ));
        }
        Ok(DOWNLOADS.with(|downloads| {
            let mut downloads = downloads.borrow_mut();
            downloads.next_id = downloads.next_id.saturating_add(1).max(1);
            let id = downloads.next_id;
            downloads
                .queued
                .push_back((id, destination, Box::new(callback)));
            id
        }))
    }

    pub fn cancel_download(id: u64, error: String) {
        let callback = DOWNLOADS.with(|downloads| {
            let mut downloads = downloads.borrow_mut();
            if let Some(index) = downloads
                .queued
                .iter()
                .position(|(candidate, _, _)| *candidate == id)
            {
                return downloads
                    .queued
                    .remove(index)
                    .map(|(_, _, callback)| callback);
            }
            downloads
                .active
                .iter()
                .position(|(candidate, _, _, _)| *candidate == id)
                .map(|index| downloads.active.remove(index).3)
        });
        if let Some(callback) = callback {
            callback(Err(error));
        }
    }

    pub fn set_visible(visible: bool) {
        BROWSER_VIEWS.with(|slot| {
            let collection = slot.borrow();
            for (page_id, runtime) in &collection.views {
                let show = visible && collection.active.as_ref() == Some(page_id);
                let _ = runtime.webview.set_visible(show);
                if !show {
                    let _ = runtime.webview.focus_parent();
                }
            }
        });
    }

    pub fn reset() {
        let stale = BROWSER_VIEWS.with(|slot| std::mem::take(&mut *slot.borrow_mut()));
        drop(stale);
        let downloads = DOWNLOADS.with(|slot| std::mem::take(&mut *slot.borrow_mut()));
        for (_, _, callback) in downloads.queued {
            callback(Err("Browser closed before the download started.".into()));
        }
        for (_, _, _, callback) in downloads.active {
            callback(Err("Browser closed before the download completed.".into()));
        }
    }

    pub fn remove_tab(page_id: &str) {
        let stale = BROWSER_VIEWS.with(|slot| {
            let mut collection = slot.borrow_mut();
            if collection.active.as_deref() == Some(page_id) {
                collection.active = None;
            }
            collection.views.remove(page_id)
        });
        drop(stale);
    }

    pub fn begin_remove_profile_data(
        profile_id: &str,
    ) -> futures::channel::oneshot::Receiver<Result<(), String>> {
        let (sender, receiver) = futures::channel::oneshot::channel();
        if profile_id == "default" {
            let _ = sender.send(Err(
                "The default browser profile data store cannot be removed.".into(),
            ));
            return receiver;
        }
        let identifier = profile_store_identifier(profile_id);
        WebView::remove_data_store(&identifier, move |result| {
            let _ = sender.send(result.map_err(|error| error.to_string()));
        });
        receiver
    }

    pub fn clear_browsing_data() -> Result<(), String> {
        with_webview(WebView::clear_all_browsing_data)
    }

    pub fn cookies(url: Option<&str>) -> Result<serde_json::Value, String> {
        with_webview(|webview| {
            // WKWebsiteDataStore's URL-filtered query has returned an empty
            // list on otherwise matching localhost cookies on several macOS
            // releases. Read the authoritative store and apply RFC-style
            // host/path filtering here so `cookie get --url` is dependable.
            let target = url.and_then(|value| url::Url::parse(value).ok());
            let cookies = webview.cookies()?;
            Ok(serde_json::json!({
                "cookies": cookies.into_iter().filter(|cookie| {
                    let Some(target) = target.as_ref() else { return true; };
                    let host = target.host_str().unwrap_or_default();
                    let domain = cookie.domain().unwrap_or_default().trim_start_matches('.');
                    let domain_matches = host == domain || host.ends_with(&format!(".{domain}"));
                    let path_matches = target.path().starts_with(cookie.path().unwrap_or("/"));
                    let secure_matches = !cookie.secure().unwrap_or(false) || target.scheme() == "https";
                    domain_matches && path_matches && secure_matches
                }).map(|cookie| serde_json::json!({
                    "name": cookie.name(),
                    "value": cookie.value(),
                    "domain": cookie.domain().unwrap_or_default(),
                    "path": cookie.path().unwrap_or("/"),
                    "secure": cookie.secure().unwrap_or(false),
                    "httpOnly": cookie.http_only().unwrap_or(false),
                    "sameSite": cookie.same_site().map(|value| format!("{value:?}").to_lowercase()),
                    "expires": cookie.expires_datetime().map(|value| value.unix_timestamp()),
                })).collect::<Vec<_>>()
            }))
        })
    }

    pub fn set_cookie(params: &serde_json::Value) -> Result<serde_json::Value, String> {
        let name = params
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "cookie set requires --name".to_string())?;
        let value = params
            .get("value")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "cookie set requires --value".to_string())?;
        let mut builder = wry::cookie::CookieBuilder::new(name.to_string(), value.to_string());
        let inferred_domain = params
            .get("domain")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                current_url()
                    .and_then(|url| url::Url::parse(&url).ok()?.host_str().map(str::to_string))
            });
        if let Some(domain) = inferred_domain {
            builder = builder.domain(domain);
        }
        builder = builder.path(
            params
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("/")
                .to_string(),
        );
        if params
            .get("secure")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            builder = builder.secure(true);
        }
        if params
            .get("httpOnly")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            builder = builder.http_only(true);
        }
        if let Some(expires) = params
            .get("expires")
            .and_then(serde_json::Value::as_i64)
            .and_then(|value| wry::cookie::time::OffsetDateTime::from_unix_timestamp(value).ok())
        {
            builder = builder.expires(expires);
        }
        let cookie = builder.build();
        with_webview(|webview| webview.set_cookie(&cookie))?;
        Ok(serde_json::json!({"success": true, "name": name}))
    }

    pub fn delete_cookie(params: &serde_json::Value) -> Result<serde_json::Value, String> {
        let name = params
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "cookie delete requires --name".to_string())?;
        let domain = params.get("domain").and_then(serde_json::Value::as_str);
        let url = params.get("url").and_then(serde_json::Value::as_str);
        let deleted = with_webview(|webview| {
            let candidates = if let Some(url) = url {
                webview.cookies_for_url(url)?
            } else {
                webview.cookies()?
            };
            let mut deleted = 0;
            for cookie in candidates.into_iter().filter(|cookie| {
                cookie.name() == name && domain.is_none_or(|domain| cookie.domain() == Some(domain))
            }) {
                webview.delete_cookie(&cookie)?;
                deleted += 1;
            }
            Ok(deleted)
        })?;
        Ok(serde_json::json!({"deleted": deleted, "name": name}))
    }

    pub fn import_cookies(bundle: super::CookieImportBundle) -> Result<usize, String> {
        with_webview(|webview| {
            let mut imported = 0;
            for source in bundle.0 {
                let mut builder = wry::cookie::CookieBuilder::new(source.name, source.value)
                    .domain(source.domain)
                    .path(source.path)
                    .secure(source.secure)
                    .http_only(source.http_only);
                if let Some(expires) = source.expires_unix.and_then(|value| {
                    wry::cookie::time::OffsetDateTime::from_unix_timestamp(value).ok()
                }) {
                    builder = builder.expires(expires);
                }
                webview.set_cookie(&builder.build())?;
                imported += 1;
            }
            Ok(imported)
        })
    }
}

#[cfg(not(target_os = "macos"))]
mod native {
    pub fn ensure(
        _page_id: &str,
        _url: &str,
        _profile_id: &str,
        _bounds: super::BrowserBounds,
        _zoom: f64,
    ) -> Result<(), String> {
        Err("The embedded browser is currently available on macOS.".to_string())
    }
    pub fn load(_url: &str) -> Result<(), String> {
        Err("The embedded browser is currently available on macOS.".to_string())
    }
    pub fn back() -> Result<(), String> {
        Ok(())
    }
    pub fn forward() -> Result<(), String> {
        Ok(())
    }
    pub fn reload() -> Result<(), String> {
        Ok(())
    }
    pub fn evaluate(
        _script: String,
        _callback: impl Fn(String) + Send + 'static,
    ) -> Result<(), String> {
        Err("Browser automation is not available on this platform.".into())
    }
    pub fn handle_dialog(_accept: bool, _text: Option<&str>) -> Result<serde_json::Value, String> {
        Err("Browser JavaScript dialogs are not available on this platform.".into())
    }
    pub fn pdf(
        _callback: impl FnOnce(Result<Vec<u8>, String>) + Send + 'static,
    ) -> Result<(), String> {
        Err("Browser PDF export is not available on this platform.".into())
    }
    pub fn screenshot(
        _format: &str,
        _callback: impl FnOnce(Result<Vec<u8>, String>) + Send + 'static,
    ) -> Result<(), String> {
        Err("Browser screenshots are not available on this platform.".into())
    }
    pub fn full_screenshot(
        _format: &str,
        _callback: impl FnOnce(Result<Vec<u8>, String>) + Send + 'static,
    ) -> Result<(), String> {
        Err("Full-page browser screenshots are not available on this platform.".into())
    }
    pub fn find_in_page() -> Result<(), String> {
        Err("Browser page search is not available on this platform.".into())
    }
    pub fn set_zoom(_zoom: f64) -> Result<(), String> {
        Ok(())
    }
    pub fn current_url() -> Option<String> {
        None
    }
    pub fn current_title() -> Option<String> {
        None
    }
    pub fn resize(_left: f32, _top: f32, _width: f32, _height: f32) -> Result<(), String> {
        Ok(())
    }
    pub fn set_viewport(_width: f32, _height: f32) -> Result<(), String> {
        Err("Browser viewport emulation is not available on this platform.".into())
    }
    pub fn begin_download(
        _destination: std::path::PathBuf,
        _callback: impl FnOnce(Result<std::path::PathBuf, String>) + 'static,
    ) -> Result<u64, String> {
        Err("Browser downloads are not available on this platform.".into())
    }
    pub fn cancel_download(_id: u64, _error: String) {}
    pub fn set_visible(_visible: bool) {}
    pub fn reset() {}
    pub fn remove_tab(_page_id: &str) {}
    pub fn begin_remove_profile_data(
        _profile_id: &str,
    ) -> futures::channel::oneshot::Receiver<Result<(), String>> {
        let (sender, receiver) = futures::channel::oneshot::channel();
        let _ = sender.send(Ok(()));
        receiver
    }
    pub fn clear_browsing_data() -> Result<(), String> {
        Ok(())
    }
    pub fn cookies(_url: Option<&str>) -> Result<serde_json::Value, String> {
        Err("Browser cookies are not available on this platform.".into())
    }
    pub fn set_cookie(_params: &serde_json::Value) -> Result<serde_json::Value, String> {
        Err("Browser cookies are not available on this platform.".into())
    }
    pub fn delete_cookie(_params: &serde_json::Value) -> Result<serde_json::Value, String> {
        Err("Browser cookies are not available on this platform.".into())
    }
    pub fn import_cookies(_bundle: super::CookieImportBundle) -> Result<usize, String> {
        Err("Browser cookie import is not available on this platform.".into())
    }
}

pub use native::*;

#[cfg(test)]
mod cookie_tests {
    use std::io::Write;
    use std::process::{Command, Stdio};

    use super::*;

    #[test]
    fn browser_automation_scripts_are_valid_and_escape_values() {
        let cases = [
            ("snapshot", serde_json::json!({})),
            ("click", serde_json::json!({"element":"@e1"})),
            ("dblclick", serde_json::json!({"element":"@e1"})),
            (
                "fill",
                serde_json::json!({"element":"@e2","value":"'; window.bad = true; //"}),
            ),
            ("type", serde_json::json!({"value":"hello"})),
            (
                "select",
                serde_json::json!({"element":"@e3","value":"option-a"}),
            ),
            ("check", serde_json::json!({"element":"@e4"})),
            ("uncheck", serde_json::json!({"element":"@e4"})),
            ("focus", serde_json::json!({"element":"@e1"})),
            ("clear", serde_json::json!({"element":"@e1"})),
            ("select-all", serde_json::json!({"element":"@e1"})),
            ("hover", serde_json::json!({"element":"@e1"})),
            ("scroll-into-view", serde_json::json!({"element":"@e1"})),
            ("drag", serde_json::json!({"from":"@e1","to":"@e2"})),
            (
                "upload",
                serde_json::json!({"element":"@e1","uploads":[{"name":"note.txt","type":"text/plain","data":"aGVsbG8="}]}),
            ),
            ("get", serde_json::json!({"what":"text","element":"@e1"})),
            ("is", serde_json::json!({"what":"visible","element":"@e1"})),
            ("insert-text", serde_json::json!({"value":"hello"})),
            ("highlight", serde_json::json!({"element":"@e1"})),
            (
                "find",
                serde_json::json!({"locator":"text","value":"Save","action":"click"}),
            ),
            ("mouse-move", serde_json::json!({"x":12,"y":34})),
            (
                "mouse-down",
                serde_json::json!({"x":12,"y":34,"button":"left"}),
            ),
            (
                "mouse-up",
                serde_json::json!({"x":12,"y":34,"button":"left"}),
            ),
            ("mouse-wheel", serde_json::json!({"dx":0,"dy":120})),
            (
                "geolocation",
                serde_json::json!({"latitude":37.5,"longitude":127.0,"accuracy":5}),
            ),
            (
                "viewport",
                serde_json::json!({"width":390,"height":844,"deviceScaleFactor":3,"mobile":true}),
            ),
            ("set-device", serde_json::json!({"name":"iPhone 15 Pro"})),
            ("set-offline", serde_json::json!({"state":true})),
            (
                "set-headers",
                serde_json::json!({"headers":{"X-Test":"safe"}}),
            ),
            (
                "set-credentials",
                serde_json::json!({"user":"user","pass":"secret"}),
            ),
            (
                "set-preferences",
                serde_json::json!({"colorScheme":"dark","reducedMotion":"reduce"}),
            ),
            ("clipboard-read", serde_json::json!({})),
            ("clipboard-write", serde_json::json!({"value":"clipboard"})),
            ("capture-start", serde_json::json!({})),
            ("capture-stop", serde_json::json!({})),
            (
                "intercept-enable",
                serde_json::json!({"patterns":["https://api.example.test/*"]}),
            ),
            ("intercept-disable", serde_json::json!({})),
            ("intercept-list", serde_json::json!({})),
            ("console", serde_json::json!({"limit":100})),
            ("network", serde_json::json!({"limit":100})),
            (
                "storage-local-set",
                serde_json::json!({"key":"k","value":"v"}),
            ),
            ("keypress", serde_json::json!({"key":"Enter"})),
            (
                "scroll",
                serde_json::json!({"direction":"down","amount":500}),
            ),
            ("eval", serde_json::json!({"expression":"document.title"})),
            ("dialog-accept", serde_json::json!({"text":"approved"})),
            ("dialog-dismiss", serde_json::json!({})),
        ];
        for (action, params) in cases {
            let script = automation_script(action, &params).expect(action);
            let Ok(mut child) = Command::new("node")
                .args(["--check", "-"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
            else {
                return;
            };
            child
                .stdin
                .take()
                .expect("node stdin")
                .write_all(script.as_bytes())
                .expect("write script");
            let output = child.wait_with_output().expect("node result");
            assert!(
                output.status.success(),
                "{action} generated invalid JavaScript: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    fn browser_eval_awaits_promises_before_serializing_the_result() {
        let script = automation_script(
            "eval",
            &serde_json::json!({"expression": "Promise.resolve(42)"}),
        )
        .unwrap();
        assert!(script.contains("Promise.resolve((0,eval)"));
        assert!(script.contains(".then(pass)"));
    }

    #[test]
    fn browser_dialog_broker_is_valid_initialization_javascript() {
        let Ok(mut child) = Command::new("node")
            .args(["--check", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
        else {
            return;
        };
        child
            .stdin
            .take()
            .expect("node stdin")
            .write_all(DIALOG_INIT_SCRIPT.as_bytes())
            .expect("write script");
        let output = child.wait_with_output().expect("node result");
        assert!(
            output.status.success(),
            "dialog initialization script is invalid: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn browser_device_profiles_match_common_orca_playwright_names() {
        assert_eq!(
            device_viewport("iPhone 15 Pro"),
            Some((393.0, 852.0, 3.0, true))
        );
        assert_eq!(
            device_viewport("Desktop Chrome"),
            Some((1280.0, 720.0, 1.0, false))
        );
        assert_eq!(device_viewport("Commodore 64"), None);
    }

    #[test]
    fn netscape_cookie_parser_keeps_flags_and_redacts_debug() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cookies.txt");
        std::fs::write(
            &path,
            "# Netscape HTTP Cookie File\n#HttpOnly_.example.com\tTRUE\t/\tTRUE\t2147483647\tsid\tsecret-value\n",
        )
        .unwrap();
        let bundle = parse_netscape_cookie_file(&path).unwrap();
        assert_eq!(bundle.0.len(), 1);
        assert!(bundle.0[0].secure);
        assert!(bundle.0[0].http_only);
        assert!(!format!("{bundle:?}").contains("secret-value"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn chromium_v10_cookie_decryption_handles_the_host_digest_prefix() {
        use aes::Aes128;
        use cbc::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
        use sha2::{Digest, Sha256};

        let host = ".example.com";
        let key = [9_u8; 16];
        let mut plaintext = Sha256::digest(host.as_bytes()).to_vec();
        plaintext.extend_from_slice(b"cookie-secret");
        let message_len = plaintext.len();
        plaintext.resize(message_len + 16, 0);
        let iv = [b' '; 16];
        let encrypted = cbc::Encryptor::<Aes128>::new((&key).into(), (&iv).into())
            .encrypt_padded_mut::<Pkcs7>(&mut plaintext, message_len)
            .unwrap();
        let mut frame = b"v10".to_vec();
        frame.extend_from_slice(encrypted);
        let encrypted_hex = frame
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<String>();

        assert_eq!(
            decrypt_chromium_cookie(host, &encrypted_hex, &key).as_deref(),
            Some("cookie-secret")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn safari_binary_cookie_parser_keeps_flags_and_mac_epoch() {
        fn put_u32_le(bytes: &mut [u8], offset: usize, value: u32) {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }

        let mut record = vec![0_u8; 112];
        let record_len = record.len() as u32;
        put_u32_le(&mut record, 0, record_len);
        put_u32_le(&mut record, 8, 5);
        put_u32_le(&mut record, 16, 48);
        put_u32_le(&mut record, 20, 64);
        put_u32_le(&mut record, 24, 72);
        put_u32_le(&mut record, 28, 80);
        record[40..48].copy_from_slice(&1_000.0_f64.to_le_bytes());
        record[48..61].copy_from_slice(b".example.com\0");
        record[64..68].copy_from_slice(b"sid\0");
        record[72..74].copy_from_slice(b"/\0");
        record[80..87].copy_from_slice(b"secret\0");

        let page_len = 12 + record.len();
        let mut page = vec![0_u8; page_len];
        page[..4].copy_from_slice(&0x0000_0100_u32.to_be_bytes());
        put_u32_le(&mut page, 4, 1);
        put_u32_le(&mut page, 8, 12);
        page[12..].copy_from_slice(&record);

        let mut file = Vec::new();
        file.extend_from_slice(b"cook");
        file.extend_from_slice(&1_u32.to_be_bytes());
        file.extend_from_slice(&(page.len() as u32).to_be_bytes());
        file.extend_from_slice(&page);
        let bundle = decode_safari_binary_cookies(&file);
        assert_eq!(bundle.0.len(), 1);
        assert!(bundle.0[0].secure);
        assert!(bundle.0[0].http_only);
        assert_eq!(bundle.0[0].expires_unix, Some(978_308_200));
        assert!(!format!("{bundle:?}").contains("secret"));
    }
}
