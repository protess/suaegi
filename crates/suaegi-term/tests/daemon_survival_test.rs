#![cfg(unix)]

use std::io::Read;
use std::time::{Duration, Instant};

use suaegi_term::daemon::{configure, DaemonClientSession, DaemonConfiguration, SpawnSpec};

#[test]
fn detached_daemon_survives_disconnect_and_warmly_reattaches() {
    let runtime_dir = tempfile::tempdir().unwrap().keep();
    configure(DaemonConfiguration {
        executable: env!("CARGO_BIN_EXE_suaegi-pty-daemon").into(),
        runtime_dir: runtime_dir.clone(),
    })
    .unwrap();
    let spec = SpawnSpec {
        program: "/bin/sh".to_string(),
        args: vec![
            "-c".to_string(),
            "printf 'survival-marker\\n'; IFS= read -r line; printf 'after:%s\\n' \"$line\""
                .to_string(),
        ],
        cwd: None,
        env: Vec::new(),
        rows: 24,
        cols: 80,
    };

    let (first, mut first_reader, is_new) =
        DaemonClientSession::create_or_attach("survival-test".to_string(), spec.clone()).unwrap();
    assert!(is_new);
    let mut buffer = [0_u8; 1024];
    let read = first_reader.read(&mut buffer).unwrap();
    assert!(String::from_utf8_lossy(&buffer[..read]).contains("survival-marker"));
    first.disconnect();
    drop(first_reader);
    drop(first);
    std::thread::sleep(Duration::from_millis(100));

    let (second, mut second_reader, is_new) =
        DaemonClientSession::create_or_attach("survival-test".to_string(), spec).unwrap();
    assert!(!is_new);
    let read = second_reader.read(&mut buffer).unwrap();
    assert!(String::from_utf8_lossy(&buffer[..read]).contains("survival-marker"));
    second.write(b"still-alive\n").unwrap();

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut output = Vec::new();
    while Instant::now() < deadline {
        let read = second_reader.read(&mut buffer).unwrap();
        if read == 0 {
            break;
        }
        output.extend_from_slice(&buffer[..read]);
        if String::from_utf8_lossy(&output).contains("after:still-alive") {
            break;
        }
    }
    assert!(String::from_utf8_lossy(&output).contains("after:still-alive"));
    second.kill().unwrap();

    let pid_text = std::fs::read_to_string(runtime_dir.join("pty-v1.pid")).unwrap();
    let daemon_pid = pid_text
        .split_whitespace()
        .next()
        .unwrap()
        .parse::<libc::pid_t>()
        .unwrap();
    assert_eq!(unsafe { libc::getsid(daemon_pid) }, daemon_pid);
    unsafe {
        libc::kill(daemon_pid, libc::SIGTERM);
    }
}
