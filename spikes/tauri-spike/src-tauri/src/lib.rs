use std::io::{Read, Write};
use std::sync::Mutex;

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{Manager, State};

struct PtySession {
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
}

struct AppState {
    session: Mutex<Option<PtySession>>,
}

#[tauri::command]
fn start_pty(
    state: State<AppState>,
    on_data: Channel<InvokeResponseBody>,
    rows: u16,
    cols: u16,
) -> Result<(), String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;

    #[cfg(not(windows))]
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    #[cfg(windows)]
    let shell = "cmd.exe".to_string();

    let mut cmd = CommandBuilder::new(shell);
    cmd.env("TERM", "xterm-256color");
    pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;

    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    *state.session.lock().unwrap() = Some(PtySession {
        writer: Mutex::new(writer),
        master: Mutex::new(pair.master),
    });

    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if on_data
                        .send(InvokeResponseBody::Raw(buf[..n].to_vec()))
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    });

    Ok(())
}

#[tauri::command]
fn pty_write(state: State<AppState>, data: String) {
    if let Some(session) = state.session.lock().unwrap().as_ref() {
        let _ = session.writer.lock().unwrap().write_all(data.as_bytes());
    }
}

#[tauri::command]
fn pty_resize(state: State<AppState>, rows: u16, cols: u16) {
    if let Some(session) = state.session.lock().unwrap().as_ref() {
        let _ = session.master.lock().unwrap().resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            app.manage(AppState {
                session: Mutex::new(None),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![start_pty, pty_write, pty_resize])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
