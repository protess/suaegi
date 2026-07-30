//! Native window appearance hooks that Iced does not expose dynamically.

pub const APP_ICON_IDS: [&str; 3] = ["classic", "watercolor", "blue"];

pub fn normalize_app_icon_id(value: &str) -> &'static str {
    APP_ICON_IDS
        .into_iter()
        .find(|candidate| *candidate == value)
        .unwrap_or("classic")
}

pub fn app_icon_bytes(value: &str) -> &'static [u8] {
    match normalize_app_icon_id(value) {
        "watercolor" => {
            include_bytes!("../../../packaging/macos/AppIcon-watercolor.png").as_slice()
        }
        "blue" => include_bytes!("../../../packaging/macos/AppIcon-blue.png").as_slice(),
        _ => include_bytes!("../../../packaging/macos/AppIcon.png").as_slice(),
    }
}

#[cfg(target_os = "macos")]
pub fn set_app_icon(value: &str) -> Result<(), String> {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let mtm = MainThreadMarker::new()
        .ok_or_else(|| "The app icon must be updated on the macOS main thread.".to_string())?;
    let data = NSData::with_bytes(app_icon_bytes(value));
    let image = NSImage::initWithData(NSImage::alloc(), &data)
        .ok_or_else(|| "The selected app icon could not be decoded.".to_string())?;
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: `image` is a decoded, retained NSImage and AppKit accepts it as
    // the process-wide application icon for the lifetime of this call.
    unsafe {
        app.setApplicationIconImage(Some(&image));
    }
    persist_app_icon_metadata(normalize_app_icon_id(value));
    Ok(())
}

#[cfg(target_os = "macos")]
fn persist_app_icon_metadata(icon_id: &'static str) {
    let Some(bundle_path) = mac_app_bundle_path() else {
        return;
    };
    let resource_path = if icon_id == "classic" {
        None
    } else {
        Some(
            bundle_path
                .join("Contents")
                .join("Resources")
                .join(format!("AppIcon-{icon_id}.png")),
        )
    };
    if resource_path.as_ref().is_some_and(|path| !path.is_file()) {
        return;
    }

    // Finder resolves the icon of a stopped app from file metadata, while the
    // AppKit call above only changes the live Dock/window-switcher tile.
    std::thread::spawn(move || {
        let result = if let Some(resource_path) = resource_path {
            run_app_icon_script(
                &[
                    "use framework \"AppKit\"",
                    "use scripting additions",
                    "set appPath to system attribute \"SUAEGI_APP_BUNDLE_PATH\"",
                    "set iconPath to system attribute \"SUAEGI_APP_ICON_PATH\"",
                    "set image to current application's NSImage's alloc()'s initWithContentsOfFile:iconPath",
                    "if image is missing value then error \"Suaegi app icon image could not be loaded\"",
                    "set ok to current application's NSWorkspace's sharedWorkspace()'s setIcon:image forFile:appPath options:0",
                    "if ok is false then error \"Suaegi app icon could not be persisted\"",
                ],
                &bundle_path,
                Some(&resource_path),
            )
        } else {
            run_app_icon_script(
                &[
                    "use framework \"AppKit\"",
                    "use scripting additions",
                    "set appPath to system attribute \"SUAEGI_APP_BUNDLE_PATH\"",
                    "set ok to current application's NSWorkspace's sharedWorkspace()'s setIcon:(missing value) forFile:appPath options:0",
                    "if ok is false then error \"Suaegi app icon could not be cleared\"",
                ],
                &bundle_path,
                None,
            )
        };
        if let Err(error) = result {
            eprintln!("[app-icon] {error}");
        }
    });
}

#[cfg(target_os = "macos")]
fn mac_app_bundle_path() -> Option<std::path::PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let bundle = executable.parent()?.parent()?.parent()?;
    (bundle.extension().and_then(|value| value.to_str()) == Some("app"))
        .then(|| bundle.to_path_buf())
}

#[cfg(target_os = "macos")]
fn run_app_icon_script(
    script: &[&str],
    bundle_path: &std::path::Path,
    icon_path: Option<&std::path::Path>,
) -> Result<(), String> {
    use std::process::{Command, Stdio};
    use std::time::Duration;
    use wait_timeout::ChildExt;

    let mut command = Command::new("/usr/bin/osascript");
    for line in script {
        command.arg("-e").arg(line);
    }
    command
        .env("SUAEGI_APP_BUNDLE_PATH", bundle_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(icon_path) = icon_path {
        command.env("SUAEGI_APP_ICON_PATH", icon_path);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start app-icon persistence: {error}"))?;
    match child
        .wait_timeout(Duration::from_secs(10))
        .map_err(|error| format!("could not wait for app-icon persistence: {error}"))?
    {
        Some(status) if status.success() => Ok(()),
        Some(status) => Err(format!("app-icon persistence exited with {status}")),
        None => {
            let _ = child.kill();
            let _ = child.wait();
            Err("app-icon persistence timed out".to_string())
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set_app_icon(_value: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "macos")]
thread_local! {
    static STATUS_ITEM: std::cell::RefCell<Option<objc2::rc::Retained<objc2_app_kit::NSStatusItem>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(target_os = "macos")]
pub fn set_menu_bar_item(enabled: bool) -> Result<(), String> {
    use objc2::{sel, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSStatusBar, NSVariableStatusItemLength};
    use objc2_foundation::NSString;

    let mtm = MainThreadMarker::new()
        .ok_or_else(|| "The menu bar item must be updated on the macOS main thread.".to_string())?;
    let app = NSApplication::sharedApplication(mtm);
    STATUS_ITEM.with(|slot| {
        let mut slot = slot.borrow_mut();
        if enabled && slot.is_none() {
            let bar = NSStatusBar::systemStatusBar();
            let item = bar.statusItemWithLength(NSVariableStatusItemLength);
            let button = item
                .button(mtm)
                .ok_or_else(|| "macOS did not provide a menu bar button.".to_string())?;
            button.setTitle(&NSString::from_str("🐬"));
            // SAFETY: `arrangeInFront:` is an AppKit action with the standard
            // single sender argument, and NSApplication remains alive for the
            // process lifetime.
            unsafe {
                button.setTarget(Some(&app));
                button.setAction(Some(sel!(arrangeInFront:)));
            }
            *slot = Some(item);
        } else if !enabled {
            if let Some(item) = slot.take() {
                NSStatusBar::systemStatusBar().removeStatusItem(&item);
            }
        }
        Ok(())
    })
}

#[cfg(not(target_os = "macos"))]
pub fn set_menu_bar_item(_enabled: bool) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn set_background_blur(enabled: bool) -> Result<(), String> {
    use std::ffi::c_void;
    use std::ptr::NonNull;

    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSColor};
    use raw_window_handle::{
        AppKitWindowHandle, HandleError, HasWindowHandle, RawWindowHandle, WindowHandle,
    };
    use window_vibrancy::{NSVisualEffectMaterial, NSVisualEffectState};

    struct ContentViewHandle(NonNull<c_void>);

    impl HasWindowHandle for ContentViewHandle {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let raw = RawWindowHandle::AppKit(AppKitWindowHandle::new(self.0));
            // SAFETY: the pointer belongs to the live NSWindow content view and
            // the returned handle cannot outlive this stack wrapper.
            Ok(unsafe { WindowHandle::borrow_raw(raw) })
        }
    }

    let mtm = MainThreadMarker::new()
        .ok_or_else(|| "Window blur must be updated on the macOS main thread.".to_string())?;
    let app = NSApplication::sharedApplication(mtm);
    let window = app
        .keyWindow()
        .or_else(|| app.mainWindow())
        .ok_or_else(|| "Suaegi window is not ready yet.".to_string())?;
    let content = window
        .contentView()
        .ok_or_else(|| "Suaegi window has no content view.".to_string())?;
    let content_handle = ContentViewHandle(NonNull::from(&*content).cast());

    let _ = window_vibrancy::clear_vibrancy(&content_handle);
    if enabled {
        window.setOpaque(false);
        let clear = NSColor::clearColor();
        window.setBackgroundColor(Some(&clear));
        window_vibrancy::apply_vibrancy(
            &content_handle,
            NSVisualEffectMaterial::UnderWindowBackground,
            Some(NSVisualEffectState::FollowsWindowActiveState),
            Some(10.0),
        )
        .map_err(|error| format!("Could not enable window blur: {error}"))?;
    } else {
        window.setOpaque(true);
        let background = NSColor::windowBackgroundColor();
        window.setBackgroundColor(Some(&background));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn set_background_blur(_enabled: bool) -> Result<(), String> {
    Err("Window background blur is available on macOS only.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_icon_ids_are_normalized_to_classic() {
        assert_eq!(normalize_app_icon_id("watercolor"), "watercolor");
        assert_eq!(normalize_app_icon_id("blue"), "blue");
        assert_eq!(normalize_app_icon_id("missing"), "classic");
        assert!(!app_icon_bytes("missing").is_empty());
    }

    #[test]
    fn every_app_icon_png_carries_an_alpha_channel() {
        for icon_id in APP_ICON_IDS {
            let bytes = app_icon_bytes(icon_id);
            assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
            assert_eq!(&bytes[12..16], b"IHDR");
            assert_eq!(bytes[25], 6, "{icon_id} must be encoded as RGBA");
        }
    }
}
