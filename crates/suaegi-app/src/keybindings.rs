//! App boundary for Orca-compatible `keybindings.json`.
//!
//! The parsing, validation, conflict detection and atomic writer live in
//! `suaegi-keys`. This module owns only the platform path and the live snapshot
//! used by the iced event adapter.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

use suaegi_keys::{
    keybinding_matches_action, keybinding_matches_input, match_keybinding_digit_index,
    normalize_keybinding, read_keybinding_file, KeybindingContext, KeybindingFileSnapshot,
    KeybindingInput, KeybindingMatchOptions, KeybindingOverrides, KeybindingPlatform, Scope,
    TerminalShortcutPolicy, KEYBINDING_DEFINITIONS,
};

use crate::keybinding_adapter::keybinding_input_from_iced;
use crate::state::Message;

pub fn host_platform() -> KeybindingPlatform {
    if cfg!(target_os = "macos") {
        KeybindingPlatform::Darwin
    } else if cfg!(target_os = "windows") {
        KeybindingPlatform::Win32
    } else {
        KeybindingPlatform::Linux
    }
}

pub fn path() -> PathBuf {
    match dirs::config_dir() {
        Some(config) => config.join("suaegi").join("keybindings.json"),
        None => dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".suaegi")
            .join("keybindings.json"),
    }
}

pub fn load() -> KeybindingFileSnapshot {
    read_keybinding_file(&path(), host_platform())
}

fn live_overrides() -> &'static RwLock<KeybindingOverrides> {
    static LIVE: OnceLock<RwLock<KeybindingOverrides>> = OnceLock::new();
    LIVE.get_or_init(|| RwLock::new(KeybindingOverrides::new()))
}

pub fn publish(snapshot: &KeybindingFileSnapshot) {
    if let Ok(mut live) = live_overrides().write() {
        *live = snapshot.overrides.clone();
    }
}

pub fn overrides() -> KeybindingOverrides {
    live_overrides()
        .read()
        .map(|value| value.clone())
        .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PluginShortcut {
    plugin_key: String,
    command_id: String,
    context: String,
    bindings: Vec<String>,
}

fn live_plugin_shortcuts() -> &'static RwLock<Vec<PluginShortcut>> {
    static LIVE: OnceLock<RwLock<Vec<PluginShortcut>>> = OnceLock::new();
    LIVE.get_or_init(|| RwLock::new(Vec::new()))
}

pub fn publish_plugin_shortcuts(plugins: &[crate::plugins::PluginEntry]) {
    let mut candidates = plugins
        .iter()
        .filter(|plugin| {
            plugin.status == crate::plugins::PluginStatus::Idle
                && plugin.blocked_by_kill_list.is_none()
        })
        .flat_map(|plugin| {
            plugin.commands.iter().map(|command| PluginShortcut {
                plugin_key: plugin.plugin_key.clone(),
                command_id: command.id.clone(),
                context: command.context.as_deref().unwrap_or("global").to_string(),
                bindings: plugin
                    .keybindings
                    .iter()
                    .filter(|binding| binding.command == command.id)
                    .map(|binding| binding.key.clone())
                    .collect(),
            })
        })
        .collect::<Vec<_>>();
    let mut conflicted = std::collections::HashSet::new();
    for left in 0..candidates.len() {
        for right in (left + 1)..candidates.len() {
            if candidates[left].plugin_key == candidates[right].plugin_key
                || (candidates[left].context != "global"
                    && candidates[right].context != "global"
                    && candidates[left].context != candidates[right].context)
            {
                continue;
            }
            let overlaps = candidates[left].bindings.iter().any(|first| {
                let first = normalize_keybinding(first)
                    .canonical()
                    .map(str::to_ascii_lowercase);
                candidates[right].bindings.iter().any(|second| {
                    first
                        == normalize_keybinding(second)
                            .canonical()
                            .map(str::to_ascii_lowercase)
                })
            });
            if overlaps {
                conflicted.insert(candidates[left].plugin_key.clone());
                conflicted.insert(candidates[right].plugin_key.clone());
            }
        }
    }
    candidates.retain(|shortcut| !conflicted.contains(&shortcut.plugin_key));
    if let Ok(mut live) = live_plugin_shortcuts().write() {
        *live = candidates;
    }
}

fn plugin_shortcut_message(input: &KeybindingInput) -> Option<Message> {
    live_plugin_shortcuts()
        .read()
        .ok()?
        .iter()
        .find_map(|shortcut| {
            shortcut
                .bindings
                .iter()
                .any(|binding| keybinding_matches_input(binding, input, host_platform()))
                .then(|| {
                    Message::PluginCommandInvoked(
                        shortcut.plugin_key.clone(),
                        shortcut.command_id.clone(),
                    )
                })
        })
}

static TERMINAL_FIRST: AtomicBool = AtomicBool::new(false);

pub fn set_terminal_shortcut_policy(value: &str) {
    TERMINAL_FIRST.store(value == "terminal-first", Ordering::Relaxed);
}

fn terminal_options() -> KeybindingMatchOptions {
    KeybindingMatchOptions {
        context: Some(KeybindingContext::Terminal),
        terminal_shortcut_policy: Some(if TERMINAL_FIRST.load(Ordering::Relaxed) {
            TerminalShortcutPolicy::TerminalFirst
        } else {
            TerminalShortcutPolicy::OrcaFirst
        }),
    }
}

/// The terminal widget calls this before it publishes bytes to the PTY. The
/// global event subscription then dispatches the same captured event as an app
/// action. Terminal-scoped actions stay with the terminal's native handler.
pub fn should_capture_in_terminal(
    key: &iced::keyboard::Key,
    physical_key: &iced::keyboard::key::Physical,
    modifiers: &iced::keyboard::Modifiers,
    platform: crate::terminal::input::Platform,
) -> bool {
    let input = keybinding_input_from_iced(key, physical_key, modifiers);
    let overrides = overrides();
    let options = terminal_options();
    let platform = match platform {
        crate::terminal::input::Platform::Mac => KeybindingPlatform::Darwin,
        crate::terminal::input::Platform::Other if cfg!(target_os = "windows") => {
            KeybindingPlatform::Win32
        }
        crate::terminal::input::Platform::Other => KeybindingPlatform::Linux,
    };
    if KEYBINDING_DEFINITIONS.iter().any(|definition| {
        definition.scope == Scope::Terminal
            && keybinding_matches_action(
                definition.id,
                &input,
                platform,
                Some(&overrides),
                &options,
            )
    }) {
        return false;
    }
    KEYBINDING_DEFINITIONS.iter().any(|definition| {
        definition.scope != Scope::Terminal
            && keybinding_matches_action(
                definition.id,
                &input,
                platform,
                Some(&overrides),
                &options,
            )
    })
}

fn event_message(
    event: iced::Event,
    status: iced::event::Status,
    _window: iced::window::Id,
) -> Option<Message> {
    static VOICE_HELD_KEY: OnceLock<Mutex<Option<iced::keyboard::key::Physical>>> = OnceLock::new();
    let held_key = VOICE_HELD_KEY.get_or_init(|| Mutex::new(None));
    let (key, physical_key, modifiers, repeat) = match event {
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key,
            physical_key,
            modifiers,
            repeat,
            ..
        }) => (key, physical_key, modifiers, repeat),
        iced::Event::Keyboard(iced::keyboard::Event::KeyReleased { physical_key, .. }) => {
            if let Ok(mut held) = held_key.lock() {
                if held.as_ref() == Some(&physical_key) {
                    *held = None;
                    return Some(Message::VoiceDictationShortcutReleased);
                }
            }
            return None;
        }
        _ => return None,
    };
    if repeat {
        return None;
    }
    let input = keybinding_input_from_iced(&key, &physical_key, &modifiers);
    let platform = host_platform();
    let overrides = overrides();
    let options = if status == iced::event::Status::Captured {
        terminal_options()
    } else {
        KeybindingMatchOptions::default()
    };
    if status == iced::event::Status::Captured {
        for definition in KEYBINDING_DEFINITIONS
            .iter()
            .filter(|definition| definition.scope == Scope::Terminal)
        {
            let index = match_keybinding_digit_index(
                definition.id,
                &input,
                platform,
                Some(&overrides),
                &options,
            );
            if index.is_some()
                || keybinding_matches_action(
                    definition.id,
                    &input,
                    platform,
                    Some(&overrides),
                    &options,
                )
            {
                if definition.id == suaegi_keys::KeybindingActionId::VoiceDictation {
                    if let Ok(mut held) = held_key.lock() {
                        *held = Some(physical_key);
                    }
                    return Some(Message::VoiceDictationShortcutPressed);
                }
                return Some(Message::KeybindingShortcut(definition.id, index));
            }
        }
    }
    if status != iced::event::Status::Captured {
        if let Some(message) = plugin_shortcut_message(&input) {
            return Some(message);
        }
    }
    for definition in KEYBINDING_DEFINITIONS {
        let index = match_keybinding_digit_index(
            definition.id,
            &input,
            platform,
            Some(&overrides),
            &options,
        );
        if index.is_some()
            || keybinding_matches_action(
                definition.id,
                &input,
                platform,
                Some(&overrides),
                &options,
            )
        {
            if definition.id == suaegi_keys::KeybindingActionId::VoiceDictation {
                if let Ok(mut held) = held_key.lock() {
                    *held = Some(physical_key);
                }
                return Some(Message::VoiceDictationShortcutPressed);
            }
            return Some(Message::KeybindingShortcut(definition.id, index));
        }
    }
    None
}

pub fn subscription() -> iced::Subscription<Message> {
    iced::event::listen_with(event_message)
}

#[cfg(test)]
mod plugin_tests {
    use super::*;

    fn plugin(key: &str, binding: &str) -> crate::plugins::PluginEntry {
        crate::plugins::PluginEntry {
            plugin_key: key.into(),
            root: PathBuf::from("/tmp/plugin"),
            content_hash: None,
            name: key.into(),
            version: "1.0.0".into(),
            publisher: key.split_once('.').unwrap().0.into(),
            description: String::new(),
            status: crate::plugins::PluginStatus::Idle,
            error: None,
            is_dev: true,
            consent_fingerprint: Some("approved".into()),
            capabilities: Vec::new(),
            panels: Vec::new(),
            commands: vec![crate::plugins::PluginCommand {
                id: "open".into(),
                title: "Open".into(),
                context: Some("global".into()),
                action: Some("view.tasks".into()),
            }],
            events: Vec::new(),
            language_packs: Vec::new(),
            language_pack_catalogs: Vec::new(),
            keybindings: vec![crate::plugins::PluginKeybinding {
                command: "open".into(),
                key: binding.into(),
                when: Some("global".into()),
            }],
            vm_recipes: Vec::new(),
            vm_recipe_specs: Vec::new(),
            agents: Vec::new(),
            has_worker: false,
            main_entry: None,
            rollback_available: false,
            blocked_by_kill_list: None,
        }
    }

    #[test]
    fn approved_plugin_shortcuts_run_before_app_defaults_and_conflicts_fail_closed() {
        let first = plugin("acme.notes", "Mod+Shift+P");
        publish_plugin_shortcuts(std::slice::from_ref(&first));
        let input = KeybindingInput {
            key: "p".into(),
            code: "KeyP".into(),
            meta: cfg!(target_os = "macos"),
            control: !cfg!(target_os = "macos"),
            shift: true,
            ..KeybindingInput::default()
        };
        assert!(matches!(
            plugin_shortcut_message(&input),
            Some(Message::PluginCommandInvoked(plugin, command))
                if plugin == "acme.notes" && command == "open"
        ));

        let second = plugin("other.notes", "Mod+Shift+P");
        publish_plugin_shortcuts(&[first, second]);
        assert!(plugin_shortcut_message(&input).is_none());
    }
}
