//! Host emulator discovery and launch support matching Orca's settings contract.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use wait_timeout::ChildExt;

const MAX_FRAME_BYTES: usize = 24 * 1024 * 1024;
const PANE_SCREEN_WIDTH: f32 = 390.0;
const PANE_SCREEN_HEIGHT: f32 = 680.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmulatorPlatform {
    Ios,
    Android,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmulatorDevice {
    pub name: String,
    pub id: String,
    pub state: String,
    pub runtime: String,
    pub available: bool,
    pub platform: EmulatorPlatform,
}

impl EmulatorDevice {
    pub fn label(&self) -> String {
        if !self.available {
            format!("{} (Unavailable)", self.name)
        } else if self.state.eq_ignore_ascii_case("shutdown") || self.state.is_empty() {
            self.name.clone()
        } else {
            format!("{} ({})", self.name, self.state)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolStatus {
    pub ok: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidStatus {
    pub sdk_found: bool,
    pub sdk_path: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmulatorAvailability {
    pub platform: String,
    pub available: bool,
    pub devices: Vec<EmulatorDevice>,
    pub simctl: ToolStatus,
    pub serve_sim: ToolStatus,
    pub android: AndroidStatus,
    pub message: String,
}

fn command_output(program: &Path, args: &[&str]) -> Result<String, String> {
    let mut stdout_file = tempfile::tempfile().map_err(|error| error.to_string())?;
    let mut stderr_file = tempfile::tempfile().map_err(|error| error.to_string())?;
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::from(
            stdout_file.try_clone().map_err(|error| error.to_string())?,
        ))
        .stderr(Stdio::from(
            stderr_file.try_clone().map_err(|error| error.to_string())?,
        ))
        .spawn()
        .map_err(|error| error.to_string())?;
    let status = match child
        .wait_timeout(Duration::from_secs(8))
        .map_err(|error| error.to_string())?
    {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{} timed out.", program.display()));
        }
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    std::io::Seek::rewind(&mut stdout_file).map_err(|error| error.to_string())?;
    std::io::Seek::rewind(&mut stderr_file).map_err(|error| error.to_string())?;
    std::io::Read::read_to_end(&mut stdout_file, &mut stdout).map_err(|error| error.to_string())?;
    std::io::Read::read_to_end(&mut stderr_file, &mut stderr).map_err(|error| error.to_string())?;
    if status.success() {
        Ok(String::from_utf8_lossy(&stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!("{} exited with {status}", program.display())
        } else {
            stderr
        })
    }
}

fn command_bytes(program: &Path, args: &[&str], limit: usize) -> Result<Vec<u8>, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("Could not run {}: {error}", program.display()))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if message.is_empty() {
            format!("{} exited with {}", program.display(), output.status)
        } else {
            message
        });
    }
    if output.stdout.len() > limit {
        return Err(format!(
            "{} returned more than {} MB.",
            program.display(),
            limit / (1024 * 1024)
        ));
    }
    Ok(output.stdout)
}

fn inspect_ios() -> (Vec<EmulatorDevice>, ToolStatus, ToolStatus) {
    if !cfg!(target_os = "macos") {
        return (
            Vec::new(),
            ToolStatus {
                ok: false,
                message: None,
            },
            ToolStatus {
                ok: false,
                message: None,
            },
        );
    }
    let xcrun = Path::new("/usr/bin/xcrun");
    let (devices, simctl) = match command_output(xcrun, &["simctl", "list", "devices", "-j"]) {
        Ok(json) => {
            let mut rows = Vec::new();
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) {
                if let Some(runtimes) = value.get("devices").and_then(|value| value.as_object()) {
                    for (runtime, entries) in runtimes {
                        let runtime_label = runtime
                            .strip_prefix("com.apple.CoreSimulator.SimRuntime.")
                            .unwrap_or(runtime)
                            .replace('-', " ");
                        for entry in entries.as_array().into_iter().flatten() {
                            let Some(id) = entry.get("udid").and_then(|value| value.as_str())
                            else {
                                continue;
                            };
                            rows.push(EmulatorDevice {
                                name: entry
                                    .get("name")
                                    .and_then(|value| value.as_str())
                                    .unwrap_or("iOS Simulator")
                                    .to_string(),
                                id: id.to_string(),
                                state: entry
                                    .get("state")
                                    .and_then(|value| value.as_str())
                                    .unwrap_or("Unknown")
                                    .to_string(),
                                runtime: runtime_label.clone(),
                                available: entry
                                    .get("isAvailable")
                                    .and_then(serde_json::Value::as_bool)
                                    .unwrap_or(true),
                                platform: EmulatorPlatform::Ios,
                            });
                        }
                    }
                }
            }
            let status = if rows.is_empty() {
                ToolStatus {
                    ok: false,
                    message: Some(
                        "No iOS simulators found. Add one in Xcode Settings > Platforms."
                            .to_string(),
                    ),
                }
            } else {
                ToolStatus {
                    ok: true,
                    message: None,
                }
            };
            (rows, status)
        }
        Err(error) => (
            Vec::new(),
            ToolStatus {
                ok: false,
                message: Some(format!("xcrun simctl is unavailable: {error}")),
            },
        ),
    };
    let serve_sim = match command_output(Path::new("serve-sim"), &["--help"]) {
        Ok(_) => ToolStatus {
            ok: true,
            message: None,
        },
        _ => ToolStatus {
            ok: false,
            message: Some(
                "serve-sim is unavailable. Install it to stream and control iOS simulators."
                    .to_string(),
            ),
        },
    };
    (devices, simctl, serve_sim)
}

fn android_sdk_dir(configured: &str) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if !configured.trim().is_empty() {
        candidates.push(PathBuf::from(configured.trim()));
    }
    for variable in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(value) = std::env::var_os(variable) {
            candidates.push(PathBuf::from(value));
        }
    }
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join("Library/Android/sdk"));
        candidates.push(home.join("Android/Sdk"));
    }
    candidates
        .into_iter()
        .find(|path| path.join("platform-tools/adb").is_file())
}

fn inspect_android(configured: &str) -> (Vec<EmulatorDevice>, AndroidStatus) {
    let Some(sdk) = android_sdk_dir(configured) else {
        return (
            Vec::new(),
            AndroidStatus {
                sdk_found: false,
                sdk_path: None,
                message:
                    "Android SDK not found. Install Android Studio, then create a Virtual Device."
                        .to_string(),
            },
        );
    };
    let adb = sdk.join("platform-tools/adb");
    let emulator = sdk.join("emulator/emulator");
    let mut devices = Vec::new();
    if let Ok(output) = command_output(&adb, &["devices"]) {
        for line in output.lines().skip(1) {
            let mut parts = line.split_whitespace();
            let (Some(id), Some(state)) = (parts.next(), parts.next()) else {
                continue;
            };
            if !id.starts_with("emulator-") {
                continue;
            }
            let name = command_output(&adb, &["-s", id, "emu", "avd", "name"])
                .ok()
                .and_then(|output| output.lines().next().map(str::to_string))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| id.to_string());
            devices.push(EmulatorDevice {
                name,
                id: id.to_string(),
                state: if state == "device" {
                    "Booted".to_string()
                } else {
                    state.to_string()
                },
                runtime: "Android".to_string(),
                available: state == "device",
                platform: EmulatorPlatform::Android,
            });
        }
    }
    if emulator.is_file() {
        if let Ok(output) = command_output(&emulator, &["-list-avds"]) {
            for name in output
                .lines()
                .map(str::trim)
                .filter(|name| !name.is_empty())
            {
                if devices.iter().any(|device| device.name == name) {
                    continue;
                }
                devices.push(EmulatorDevice {
                    name: name.to_string(),
                    id: name.to_string(),
                    state: "Shutdown".to_string(),
                    runtime: "Android".to_string(),
                    available: true,
                    platform: EmulatorPlatform::Android,
                });
            }
        }
    }
    let message = if devices.is_empty() {
        "Android SDK found, but no emulator devices are configured.".to_string()
    } else {
        "Ready".to_string()
    };
    (
        devices,
        AndroidStatus {
            sdk_found: true,
            sdk_path: Some(sdk.to_string_lossy().into_owned()),
            message,
        },
    )
}

pub async fn inspect(configured_android_sdk: String) -> EmulatorAvailability {
    tokio::task::spawn_blocking(move || {
        let (mut ios_devices, simctl, serve_sim) = inspect_ios();
        let (android_devices, android) = inspect_android(&configured_android_sdk);
        // Screenshot streaming uses `simctl io` directly. `serve-sim` adds
        // low-latency iOS input, but its absence must not hide an otherwise
        // usable embedded device view.
        let ios_available = simctl.ok && !ios_devices.is_empty();
        let android_available = android.sdk_found && !android_devices.is_empty();
        ios_devices.extend(android_devices);
        let available = ios_available || android_available;
        let message = if available {
            if ios_available && !serve_sim.ok {
                "Ready for embedded viewing · install serve-sim for iOS input".to_string()
            } else {
                "Ready".to_string()
            }
        } else {
            simctl
                .message
                .clone()
                .or_else(|| serve_sim.message.clone())
                .filter(|_| cfg!(target_os = "macos"))
                .unwrap_or_else(|| {
                    if android.message.is_empty() {
                        "Mobile Emulator is not available.".to_string()
                    } else {
                        android.message.clone()
                    }
                })
        };
        EmulatorAvailability {
            platform: std::env::consts::OS.to_string(),
            available,
            devices: ios_devices,
            simctl,
            serve_sim,
            android,
            message,
        }
    })
    .await
    .unwrap_or_else(|error| EmulatorAvailability {
        platform: std::env::consts::OS.to_string(),
        available: false,
        devices: Vec::new(),
        simctl: ToolStatus {
            ok: false,
            message: None,
        },
        serve_sim: ToolStatus {
            ok: false,
            message: None,
        },
        android: AndroidStatus {
            sdk_found: false,
            sdk_path: None,
            message: String::new(),
        },
        message: format!("Could not inspect emulator availability: {error}"),
    })
}

pub async fn launch(device: EmulatorDevice, configured_android_sdk: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || match device.platform {
        EmulatorPlatform::Ios => {
            if !device.state.eq_ignore_ascii_case("booted") {
                command_output(Path::new("/usr/bin/xcrun"), &["simctl", "boot", &device.id])
                    .map_err(|error| format!("Could not boot iOS simulator: {error}"))?;
            }
            Command::new("open")
                .args(["-a", "Simulator"])
                .spawn()
                .map_err(|error| error.to_string())?;
            Ok(())
        }
        EmulatorPlatform::Android => {
            let sdk = android_sdk_dir(&configured_android_sdk)
                .ok_or_else(|| "Android SDK is unavailable.".to_string())?;
            if device.id.starts_with("emulator-") {
                return Ok(());
            }
            Command::new(sdk.join("emulator/emulator"))
                .args(["-avd", &device.id])
                .spawn()
                .map_err(|error| format!("Could not launch Android emulator: {error}"))?;
            Ok(())
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

fn android_adb(configured_android_sdk: &str) -> Result<PathBuf, String> {
    android_sdk_dir(configured_android_sdk)
        .map(|sdk| sdk.join("platform-tools/adb"))
        .ok_or_else(|| "Android SDK is unavailable.".to_string())
}

fn normalized_coordinate(value: f32) -> Result<f32, String> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(value)
    } else {
        Err("Emulator coordinates must be finite values from 0 to 1.".to_string())
    }
}

fn parse_android_display_size(output: &str) -> Result<(u32, u32), String> {
    output
        .lines()
        .find_map(|line| {
            let dimensions = line.split_once(':')?.1.trim();
            let (width, height) = dimensions.split_once('x')?;
            Some((width.trim().parse().ok()?, height.trim().parse().ok()?))
        })
        .filter(|(width, height)| *width > 0 && *height > 0)
        .ok_or_else(|| "Could not determine the Android display size.".to_string())
}

fn android_pixel(value: f32, extent: u32) -> u32 {
    (value * extent.saturating_sub(1) as f32).round() as u32
}

fn android_device_args<'a>(device: &'a EmulatorDevice, tail: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec!["-s", device.id.as_str()];
    args.extend_from_slice(tail);
    args
}

fn ensure_available_device(device: &EmulatorDevice) -> Result<(), String> {
    if device.id.trim().is_empty() || device.id.len() > 512 || device.id.contains('\0') {
        return Err("Emulator device id is invalid.".to_string());
    }
    if !device.available {
        return Err(format!("{} is unavailable.", device.name));
    }
    Ok(())
}

fn android_size(
    device: &EmulatorDevice,
    configured_android_sdk: &str,
) -> Result<(u32, u32), String> {
    let adb = android_adb(configured_android_sdk)?;
    let args = android_device_args(device, &["shell", "wm", "size"]);
    parse_android_display_size(&command_output(&adb, &args)?)
}

/// Captures a bounded PNG frame for the embedded emulator pane.
pub async fn screenshot(
    device: EmulatorDevice,
    configured_android_sdk: String,
) -> Result<Vec<u8>, String> {
    tokio::task::spawn_blocking(move || {
        ensure_available_device(&device)?;
        let bytes = match device.platform {
            EmulatorPlatform::Ios => command_bytes(
                Path::new("/usr/bin/xcrun"),
                &[
                    "simctl",
                    "io",
                    device.id.as_str(),
                    "screenshot",
                    "--type=png",
                    "-",
                ],
                MAX_FRAME_BYTES,
            )?,
            EmulatorPlatform::Android => {
                let adb = android_adb(&configured_android_sdk)?;
                command_bytes(
                    &adb,
                    &["-s", device.id.as_str(), "exec-out", "screencap", "-p"],
                    MAX_FRAME_BYTES,
                )?
            }
        };
        if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err("The emulator returned an invalid screenshot.".to_string());
        }
        Ok(bytes)
    })
    .await
    .map_err(|error| format!("Emulator screenshot task failed: {error}"))?
}

pub async fn tap(
    device: EmulatorDevice,
    x: f32,
    y: f32,
    configured_android_sdk: String,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        ensure_available_device(&device)?;
        let x = normalized_coordinate(x)?;
        let y = normalized_coordinate(y)?;
        match device.platform {
            EmulatorPlatform::Ios => command_output(
                Path::new("serve-sim"),
                &[
                    "tap",
                    &x.to_string(),
                    &y.to_string(),
                    "-d",
                    device.id.as_str(),
                ],
            )
            .map(|_| ()),
            EmulatorPlatform::Android => {
                let adb = android_adb(&configured_android_sdk)?;
                let (width, height) = android_size(&device, &configured_android_sdk)?;
                let x = android_pixel(x, width).to_string();
                let y = android_pixel(y, height).to_string();
                command_output(
                    &adb,
                    &android_device_args(
                        &device,
                        &["shell", "input", "tap", x.as_str(), y.as_str()],
                    ),
                )
                .map(|_| ())
            }
        }
    })
    .await
    .map_err(|error| format!("Emulator tap task failed: {error}"))?
}

pub async fn type_text(
    device: EmulatorDevice,
    value: String,
    configured_android_sdk: String,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        ensure_available_device(&device)?;
        if value.len() > 64 * 1024 || value.contains('\0') {
            return Err("Emulator text input is invalid or too large.".to_string());
        }
        match device.platform {
            EmulatorPlatform::Ios => command_output(
                Path::new("serve-sim"),
                &["type", value.as_str(), "-d", device.id.as_str()],
            )
            .map(|_| ()),
            EmulatorPlatform::Android => {
                let adb = android_adb(&configured_android_sdk)?;
                let encoded = value.replace(' ', "%s");
                command_output(
                    &adb,
                    &android_device_args(&device, &["shell", "input", "text", encoded.as_str()]),
                )
                .map(|_| ())
            }
        }
    })
    .await
    .map_err(|error| format!("Emulator text task failed: {error}"))?
}

fn android_button_key(name: &str) -> Option<&'static str> {
    match name {
        "back" => Some("4"),
        "home" => Some("3"),
        "recents" => Some("187"),
        "power" => Some("26"),
        "volume-up" => Some("24"),
        "volume-down" => Some("25"),
        _ => None,
    }
}

pub async fn button(
    device: EmulatorDevice,
    name: String,
    configured_android_sdk: String,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        ensure_available_device(&device)?;
        match device.platform {
            EmulatorPlatform::Ios => {
                if !matches!(
                    name.as_str(),
                    "home" | "lock" | "volume-up" | "volume-down" | "siri"
                ) {
                    return Err("Unsupported iOS emulator button.".to_string());
                }
                command_output(
                    Path::new("serve-sim"),
                    &["button", name.as_str(), "-d", device.id.as_str()],
                )
                .map(|_| ())
            }
            EmulatorPlatform::Android => {
                let key = android_button_key(&name)
                    .ok_or_else(|| "Unsupported Android emulator button.".to_string())?;
                let adb = android_adb(&configured_android_sdk)?;
                command_output(
                    &adb,
                    &android_device_args(&device, &["shell", "input", "keyevent", key]),
                )
                .map(|_| ())
            }
        }
    })
    .await
    .map_err(|error| format!("Emulator button task failed: {error}"))?
}

pub async fn rotate(
    device: EmulatorDevice,
    orientation: String,
    configured_android_sdk: String,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        ensure_available_device(&device)?;
        match device.platform {
            EmulatorPlatform::Ios => {
                if !matches!(
                    orientation.as_str(),
                    "left" | "right" | "portrait" | "landscape"
                ) {
                    return Err("Unsupported iOS emulator orientation.".to_string());
                }
                command_output(
                    Path::new("serve-sim"),
                    &["rotate", orientation.as_str(), "-d", device.id.as_str()],
                )
                .map(|_| ())
            }
            EmulatorPlatform::Android => {
                let rotation = match orientation.as_str() {
                    "portrait" => "0",
                    "landscape" | "left" => "1",
                    "right" => "3",
                    _ => return Err("Unsupported Android emulator orientation.".to_string()),
                };
                let adb = android_adb(&configured_android_sdk)?;
                command_output(
                    &adb,
                    &android_device_args(
                        &device,
                        &[
                            "shell",
                            "settings",
                            "put",
                            "system",
                            "accelerometer_rotation",
                            "0",
                        ],
                    ),
                )?;
                command_output(
                    &adb,
                    &android_device_args(
                        &device,
                        &[
                            "shell",
                            "settings",
                            "put",
                            "system",
                            "user_rotation",
                            rotation,
                        ],
                    ),
                )
                .map(|_| ())
            }
        }
    })
    .await
    .map_err(|error| format!("Emulator rotation task failed: {error}"))?
}

pub async fn gesture(
    device: EmulatorDevice,
    points: Vec<(f32, f32)>,
    configured_android_sdk: String,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        ensure_available_device(&device)?;
        if points.len() < 2 || points.len() > 1_000 {
            return Err("A gesture requires between 2 and 1000 points.".to_string());
        }
        for (x, y) in &points {
            normalized_coordinate(*x)?;
            normalized_coordinate(*y)?;
        }
        match device.platform {
            EmulatorPlatform::Ios => {
                Err("iOS gesture input requires an active serve-sim stream endpoint.".to_string())
            }
            EmulatorPlatform::Android => {
                let adb = android_adb(&configured_android_sdk)?;
                let (width, height) = android_size(&device, &configured_android_sdk)?;
                let first = points.first().expect("validated non-empty");
                let last = points.last().expect("validated non-empty");
                let values = [
                    android_pixel(first.0, width).to_string(),
                    android_pixel(first.1, height).to_string(),
                    android_pixel(last.0, width).to_string(),
                    android_pixel(last.1, height).to_string(),
                ];
                command_output(
                    &adb,
                    &android_device_args(
                        &device,
                        &[
                            "shell", "input", "swipe", &values[0], &values[1], &values[2],
                            &values[3], "300",
                        ],
                    ),
                )
                .map(|_| ())
            }
        }
    })
    .await
    .map_err(|error| format!("Emulator gesture task failed: {error}"))?
}

pub async fn raw_exec(
    device: EmulatorDevice,
    command: String,
    configured_android_sdk: String,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        ensure_available_device(&device)?;
        if command.len() > 64 * 1024 || command.contains('\0') {
            return Err("Emulator command is invalid or too large.".to_string());
        }
        let arguments = command.split_whitespace().collect::<Vec<_>>();
        if arguments.is_empty() {
            return Err("Emulator command is empty.".to_string());
        }
        match device.platform {
            EmulatorPlatform::Ios => {
                let mut args = arguments;
                args.extend(["-d", device.id.as_str()]);
                command_output(Path::new("serve-sim"), &args)
            }
            EmulatorPlatform::Android => {
                let adb = android_adb(&configured_android_sdk)?;
                let mut args = vec!["-s", device.id.as_str(), "shell"];
                args.extend(arguments);
                command_output(&adb, &args)
            }
        }
    })
    .await
    .map_err(|error| format!("Emulator command task failed: {error}"))?
}

pub async fn stop_helper(
    device: EmulatorDevice,
    configured_android_sdk: String,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        ensure_available_device(&device)?;
        match device.platform {
            EmulatorPlatform::Ios => command_output(
                Path::new("serve-sim"),
                &["--kill", "-q", device.id.as_str()],
            )
            .map(|_| ()),
            EmulatorPlatform::Android => {
                let adb = android_adb(&configured_android_sdk)?;
                command_output(
                    &adb,
                    &android_device_args(&device, &["forward", "--remove-all"]),
                )
                .map(|_| ())
            }
        }
    })
    .await
    .map_err(|error| format!("Emulator helper stop task failed: {error}"))?
}

pub async fn shutdown(
    device: EmulatorDevice,
    configured_android_sdk: String,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        ensure_available_device(&device)?;
        match device.platform {
            EmulatorPlatform::Ios => command_output(
                Path::new("/usr/bin/xcrun"),
                &["simctl", "shutdown", device.id.as_str()],
            )
            .map(|_| ()),
            EmulatorPlatform::Android => {
                let adb = android_adb(&configured_android_sdk)?;
                command_output(&adb, &android_device_args(&device, &["emu", "kill"])).map(|_| ())
            }
        }
    })
    .await
    .map_err(|error| format!("Emulator shutdown task failed: {error}"))?
}

pub async fn install_android(
    device: EmulatorDevice,
    apk_path: PathBuf,
    reinstall: bool,
    configured_android_sdk: String,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        ensure_android_device(&device)?;
        let apk = apk_path
            .canonicalize()
            .map_err(|error| format!("Could not open APK: {error}"))?;
        if apk.extension().and_then(|value| value.to_str()) != Some("apk") {
            return Err("The install file must be an APK.".to_string());
        }
        let adb = android_adb(&configured_android_sdk)?;
        let mut args = vec!["-s", device.id.as_str(), "install"];
        if reinstall {
            args.push("-r");
        }
        let apk_text = apk.to_string_lossy();
        args.push(apk_text.as_ref());
        command_output(&adb, &args).and_then(|output| {
            if output.contains("Failure") || output.contains("Error") {
                Err("adb install reported a failure.".to_string())
            } else {
                Ok(())
            }
        })
    })
    .await
    .map_err(|error| format!("Android install task failed: {error}"))?
}

pub async fn launch_android_app(
    device: EmulatorDevice,
    package: String,
    activity: Option<String>,
    configured_android_sdk: String,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        ensure_android_device(&device)?;
        validate_android_identifier(&package, "package")?;
        let adb = android_adb(&configured_android_sdk)?;
        let mut args = vec!["-s", device.id.as_str(), "shell"];
        if let Some(activity) = activity.filter(|value| !value.trim().is_empty()) {
            validate_android_identifier(&activity, "activity")?;
            let component = format!("{package}/{activity}");
            args.extend(["am", "start", "-n", component.as_str()]);
            command_output(&adb, &args).map(|_| ())
        } else {
            args.extend([
                "monkey",
                "-p",
                package.as_str(),
                "-c",
                "android.intent.category.LAUNCHER",
                "1",
            ]);
            command_output(&adb, &args).map(|_| ())
        }
    })
    .await
    .map_err(|error| format!("Android app launch task failed: {error}"))?
}

pub async fn android_permission(
    device: EmulatorDevice,
    operation: String,
    package: String,
    permission: Option<String>,
    configured_android_sdk: String,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        ensure_android_device(&device)?;
        let adb = android_adb(&configured_android_sdk)?;
        let mut args = vec!["-s", device.id.as_str(), "shell", "pm"];
        match operation.as_str() {
            "reset" => args.push("reset-permissions"),
            "grant" | "revoke" => {
                validate_android_identifier(&package, "package")?;
                let permission = permission
                    .as_deref()
                    .ok_or_else(|| format!("pm {operation} requires a permission name"))?;
                validate_android_identifier(permission, "permission")?;
                args.extend([operation.as_str(), package.as_str(), permission]);
            }
            _ => return Err("Permission operation must be grant, revoke, or reset.".to_string()),
        }
        command_output(&adb, &args).map(|_| ())
    })
    .await
    .map_err(|error| format!("Android permission task failed: {error}"))?
}

pub async fn accessibility_tree(
    device: EmulatorDevice,
    configured_android_sdk: String,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        ensure_android_device(&device)?;
        let adb = android_adb(&configured_android_sdk)?;
        command_output(
            &adb,
            &android_device_args(
                &device,
                &["shell", "uiautomator", "dump", "/sdcard/window_dump.xml"],
            ),
        )?;
        command_output(
            &adb,
            &android_device_args(&device, &["shell", "cat", "/sdcard/window_dump.xml"]),
        )
    })
    .await
    .map_err(|error| format!("Accessibility tree task failed: {error}"))?
}

pub async fn logcat(
    device: EmulatorDevice,
    lines: Option<usize>,
    configured_android_sdk: String,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        ensure_android_device(&device)?;
        let adb = android_adb(&configured_android_sdk)?;
        let line_count = lines.unwrap_or(200).clamp(1, 10_000).to_string();
        command_output(
            &adb,
            &android_device_args(
                &device,
                &[
                    "logcat",
                    "-d",
                    "-v",
                    "threadtime",
                    "-t",
                    line_count.as_str(),
                ],
            ),
        )
    })
    .await
    .map_err(|error| format!("Android logcat task failed: {error}"))?
}

fn ensure_android_device(device: &EmulatorDevice) -> Result<(), String> {
    ensure_available_device(device)?;
    if device.platform == EmulatorPlatform::Android && device.id.starts_with("emulator-") {
        Ok(())
    } else {
        Err("This operation requires a running Android emulator.".to_string())
    }
}

fn validate_android_identifier(value: &str, label: &str) -> Result<(), String> {
    let valid = !value.is_empty()
        && value.len() <= 512
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '.' | '_' | '$' | '/' | '-' | ':')
        });
    if valid {
        Ok(())
    } else {
        Err(format!("Android {label} is invalid."))
    }
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (width > 0 && height > 0).then_some((width, height))
}

fn normalized_tap(point: iced::Point, dimensions: (u32, u32)) -> Option<(f32, f32)> {
    let image_aspect = dimensions.0 as f32 / dimensions.1 as f32;
    let viewport_aspect = PANE_SCREEN_WIDTH / PANE_SCREEN_HEIGHT;
    let (width, height) = if image_aspect > viewport_aspect {
        (PANE_SCREEN_WIDTH, PANE_SCREEN_WIDTH / image_aspect)
    } else {
        (PANE_SCREEN_HEIGHT * image_aspect, PANE_SCREEN_HEIGHT)
    };
    let left = (PANE_SCREEN_WIDTH - width) / 2.0;
    let top = (PANE_SCREEN_HEIGHT - height) / 2.0;
    if point.x < left || point.y < top || point.x > left + width || point.y > top + height {
        return None;
    }
    Some(((point.x - left) / width, (point.y - top) / height))
}

pub(crate) fn tap_coordinates(frame: &[u8], point: iced::Point) -> Option<(f32, f32)> {
    normalized_tap(point, png_dimensions(frame)?)
}

pub fn view(state: &crate::state::AppState) -> iced::Element<'_, crate::state::Message> {
    use iced::widget::{button, column, container, image, mouse_area, row, text_input, Space};
    use iced::{Alignment, ContentFit, Length};

    let Some(device) = state.emulator_active_device() else {
        return container(crate::i18n::text("No emulator session is active."))
            .center(Length::Fill)
            .into();
    };
    let platform = match device.platform {
        EmulatorPlatform::Ios => "iOS",
        EmulatorPlatform::Android => "Android",
    };
    let header = row![
        column![
            crate::i18n::text(device.name.clone()).size(14),
            crate::i18n::text(format!("{platform} · {}", device.runtime))
                .size(11)
                .color(crate::theme::MUTED)
        ],
        Space::new().width(Length::Fill),
        button(crate::i18n::text("Refresh").size(11))
            .on_press(crate::state::Message::EmulatorFrameTick)
            .padding([5, 9])
            .style(crate::theme::ghost_button),
        button(crate::i18n::text("×").size(15))
            .on_press(crate::state::Message::EmulatorClosed)
            .padding([3, 8])
            .style(crate::theme::ghost_button)
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let screen: iced::Element<'_, crate::state::Message> =
        if let Some(frame) = state.emulator_frame() {
            mouse_area(
                image(iced::widget::image::Handle::from_bytes(frame.to_vec()))
                    .width(Length::Fixed(PANE_SCREEN_WIDTH))
                    .height(Length::Fixed(PANE_SCREEN_HEIGHT))
                    .content_fit(ContentFit::Contain),
            )
            .on_move(crate::state::Message::EmulatorPointerMoved)
            .on_press(crate::state::Message::EmulatorTapRequested)
            .into()
        } else {
            container(crate::i18n::text("Connecting to the device…").size(13))
                .center_x(Length::Fixed(PANE_SCREEN_WIDTH))
                .center_y(Length::Fixed(PANE_SCREEN_HEIGHT))
                .into()
        };

    let mut hardware = row![].spacing(5).align_y(Alignment::Center);
    if device.platform == EmulatorPlatform::Android {
        hardware = hardware
            .push(
                button(crate::i18n::text("Back").size(11))
                    .on_press(crate::state::Message::EmulatorControlRequested(
                        "back".into(),
                    ))
                    .style(crate::theme::ghost_button),
            )
            .push(
                button(crate::i18n::text("Recents").size(11))
                    .on_press(crate::state::Message::EmulatorControlRequested(
                        "recents".into(),
                    ))
                    .style(crate::theme::ghost_button),
            );
    }
    hardware = hardware
        .push(
            button(crate::i18n::text("Home").size(11))
                .on_press(crate::state::Message::EmulatorControlRequested(
                    "home".into(),
                ))
                .style(crate::theme::ghost_button),
        )
        .push(
            button(crate::i18n::text("Power").size(11))
                .on_press(crate::state::Message::EmulatorControlRequested(
                    if device.platform == EmulatorPlatform::Ios {
                        "lock"
                    } else {
                        "power"
                    }
                    .into(),
                ))
                .style(crate::theme::ghost_button),
        )
        .push(
            button(crate::i18n::text("↺").size(13))
                .on_press(crate::state::Message::EmulatorControlRequested(
                    "rotate-left".into(),
                ))
                .style(crate::theme::ghost_button),
        )
        .push(
            button(crate::i18n::text("Vol −").size(11))
                .on_press(crate::state::Message::EmulatorControlRequested(
                    "volume-down".into(),
                ))
                .style(crate::theme::ghost_button),
        )
        .push(
            button(crate::i18n::text("Vol +").size(11))
                .on_press(crate::state::Message::EmulatorControlRequested(
                    "volume-up".into(),
                ))
                .style(crate::theme::ghost_button),
        );
    let input = row![
        text_input("Type on device", state.emulator_text_draft())
            .on_input(crate::state::Message::EmulatorTextChanged)
            .on_submit(crate::state::Message::EmulatorTextSubmitted)
            .padding([7, 9])
            .size(12)
            .width(Length::Fill),
        button(crate::i18n::text("Send").size(11))
            .on_press(crate::state::Message::EmulatorTextSubmitted)
            .padding([7, 11])
            .style(crate::theme::primary_dark_button)
    ]
    .spacing(6);
    let status = state.emulator_status().unwrap_or_default();
    container(
        column![
            header,
            container(screen).style(crate::theme::card),
            hardware,
            input,
            crate::i18n::text(status.to_string())
                .size(11)
                .color(crate::theme::MUTED)
        ]
        .spacing(8)
        .align_x(Alignment::Center),
    )
    .padding(12)
    .width(Length::Fill)
    .height(Length::Fill)
    .center_x(Length::Fill)
    .into()
}

pub fn pick_default(devices: &[EmulatorDevice]) -> Option<&EmulatorDevice> {
    let available = devices.iter().filter(|device| device.available);
    available
        .clone()
        .find(|device| {
            device.state.eq_ignore_ascii_case("booted")
                && device.name.to_ascii_lowercase().contains("iphone")
        })
        .or_else(|| {
            available
                .clone()
                .find(|device| device.state.eq_ignore_ascii_case("booted"))
        })
        .or_else(|| {
            available
                .clone()
                .find(|device| device.name.to_ascii_lowercase().contains("iphone"))
        })
        .or_else(|| available.into_iter().next())
        .or_else(|| devices.first())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str, state: &str, platform: EmulatorPlatform) -> EmulatorDevice {
        EmulatorDevice {
            name: name.to_string(),
            id: name.to_string(),
            state: state.to_string(),
            runtime: String::new(),
            available: true,
            platform,
        }
    }

    #[test]
    fn default_picker_matches_orca_priority() {
        let devices = vec![
            device("Pixel", "Booted", EmulatorPlatform::Android),
            device("iPhone 16", "Booted", EmulatorPlatform::Ios),
        ];
        assert_eq!(pick_default(&devices).unwrap().name, "iPhone 16");
    }

    #[test]
    fn labels_hide_shutdown_but_show_active_state() {
        assert_eq!(
            device("iPhone 16", "Shutdown", EmulatorPlatform::Ios).label(),
            "iPhone 16"
        );
        assert_eq!(
            device("Pixel", "Booted", EmulatorPlatform::Android).label(),
            "Pixel (Booted)"
        );
    }

    #[test]
    fn android_display_size_and_normalized_coordinates_are_bounded() {
        assert_eq!(
            parse_android_display_size("Physical size: 1080x2400\nOverride size: 540x1200")
                .unwrap(),
            (1080, 2400)
        );
        assert_eq!(android_pixel(0.0, 1080), 0);
        assert_eq!(android_pixel(0.5, 1080), 540);
        assert_eq!(android_pixel(1.0, 1080), 1079);
        assert!(normalized_coordinate(f32::NAN).is_err());
        assert!(normalized_coordinate(-0.1).is_err());
        assert!(normalized_coordinate(1.1).is_err());
    }

    #[test]
    fn android_hardware_buttons_match_orca_keycodes() {
        assert_eq!(android_button_key("back"), Some("4"));
        assert_eq!(android_button_key("home"), Some("3"));
        assert_eq!(android_button_key("recents"), Some("187"));
        assert_eq!(android_button_key("power"), Some("26"));
        assert_eq!(android_button_key("volume-up"), Some("24"));
        assert_eq!(android_button_key("volume-down"), Some("25"));
        assert_eq!(android_button_key("siri"), None);
    }

    #[test]
    fn pane_taps_ignore_letterbox_and_normalize_device_pixels() {
        let dimensions = (1170, 2532);
        let center =
            normalized_tap(iced::Point::new(195.0, 340.0), dimensions).expect("image center");
        assert!((center.0 - 0.5).abs() < 0.01);
        assert!((center.1 - 0.5).abs() < 0.01);
        assert!(normalized_tap(iced::Point::new(1.0, 340.0), dimensions).is_none());
        assert_eq!(png_dimensions(b"not a png"), None);
    }
}
