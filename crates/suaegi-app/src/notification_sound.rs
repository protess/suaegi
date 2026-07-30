//! Orca-compatible desktop notification sound selection and playback.

use std::path::{Path, PathBuf};

pub const SOUND_IDS: [&str; 11] = [
    "system", "two-tone", "bong", "thump", "blip", "sonar", "blop", "ding", "clack", "beep",
    "custom",
];

const CUSTOM_EXTENSIONS: [&str; 6] = ["ogg", "mp3", "wav", "m4a", "aac", "flac"];

pub fn normalize_sound_id(value: &str) -> &'static str {
    SOUND_IDS
        .into_iter()
        .find(|candidate| *candidate == value)
        .unwrap_or("system")
}

pub fn is_supported_custom_sound(path: &Path) -> bool {
    path.is_absolute()
        && path.is_file()
        && path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| {
                CUSTOM_EXTENSIONS
                    .iter()
                    .any(|candidate| extension.eq_ignore_ascii_case(candidate))
            })
}

fn bundled_sound_path(sound_id: &str) -> Option<PathBuf> {
    let sound_id = normalize_sound_id(sound_id);
    if matches!(sound_id, "system" | "custom") {
        return None;
    }
    let filename = format!("{sound_id}.mp3");
    if let Ok(executable) = std::env::current_exe() {
        if let Some(bundle) = executable.parent().and_then(Path::parent) {
            let candidate = bundle
                .join("Resources")
                .join("notification-sounds")
                .join(&filename);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let development = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packaging/macos/notification-sounds")
        .join(filename);
    development.is_file().then_some(development)
}

fn selected_sound_path(sound_id: &str, custom_path: Option<&Path>) -> Option<PathBuf> {
    match normalize_sound_id(sound_id) {
        "system" => None,
        "custom" => custom_path
            .filter(|path| is_supported_custom_sound(path))
            .map(Path::to_path_buf),
        builtin => bundled_sound_path(builtin),
    }
}

#[cfg(all(target_os = "macos", not(test)))]
fn show_with_title(
    title: &str,
    body: &str,
    sound_id: &str,
    custom_path: Option<&Path>,
    volume: u8,
) -> bool {
    use std::process::{Command, Stdio};

    let sound_id = normalize_sound_id(sound_id);
    // Keep untrusted notification copy out of AppleScript source. Plugin
    // titles and bodies are argv values, so quotes/newlines cannot escape the
    // string expression or inject another AppleScript statement.
    let script = if sound_id == "system" {
        r#"on run argv
display notification (item 2 of argv) with title (item 1 of argv) sound name "default"
end run"#
    } else {
        r#"on run argv
display notification (item 2 of argv) with title (item 1 of argv)
end run"#
    };
    let delivered = Command::new("/usr/bin/osascript")
        .args(["-e", script, "--", title, body])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok();

    if volume == 0 || sound_id == "system" {
        return delivered;
    }
    if let Some(path) = selected_sound_path(sound_id, custom_path) {
        let _ = Command::new("/usr/bin/afplay")
            .arg("-v")
            .arg(format!("{:.2}", f32::from(volume.min(100)) / 100.0))
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    delivered
}

#[cfg(all(target_os = "macos", not(test)))]
pub fn show(body: &str, sound_id: &str, custom_path: Option<&Path>, volume: u8) {
    let _ = show_with_title("Suaegi", body, sound_id, custom_path, volume);
}

#[cfg(all(target_os = "macos", not(test)))]
pub fn show_plugin(title: &str, body: &str) -> bool {
    show_with_title(title, body, "system", None, 100)
}

#[cfg(any(not(target_os = "macos"), test))]
pub fn show(_body: &str, _sound_id: &str, _custom_path: Option<&Path>, _volume: u8) {}

#[cfg(any(not(target_os = "macos"), test))]
pub fn show_plugin(_title: &str, _body: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_sound_ids_fall_back_to_system() {
        assert_eq!(normalize_sound_id("sonar"), "sonar");
        assert_eq!(normalize_sound_id("unknown"), "system");
    }

    #[test]
    fn custom_sound_requires_an_existing_absolute_supported_file() {
        let directory = tempfile::tempdir().unwrap();
        let supported = directory.path().join("tone.MP3");
        std::fs::write(&supported, b"audio").unwrap();
        let unsupported = directory.path().join("tone.txt");
        std::fs::write(&unsupported, b"audio").unwrap();

        assert!(is_supported_custom_sound(&supported));
        assert!(!is_supported_custom_sound(&unsupported));
        assert!(!is_supported_custom_sound(Path::new("tone.mp3")));
        assert_eq!(
            selected_sound_path("custom", Some(&supported)),
            Some(supported)
        );
    }

    #[test]
    fn all_built_in_sounds_are_packaged_for_development() {
        for sound in SOUND_IDS
            .into_iter()
            .filter(|sound| !matches!(*sound, "system" | "custom"))
        {
            assert!(
                bundled_sound_path(sound).is_some(),
                "missing bundled sound: {sound}"
            );
        }
    }
}
