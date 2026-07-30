fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut process_args = std::env::args_os();
    let argv0 = process_args.next().unwrap_or_default();
    let collected = process_args.collect::<Vec<_>>();
    let mut args = collected.clone().into_iter();
    match args.next().as_deref() {
        Some(value) if value == std::ffi::OsStr::new("--pty-daemon") => {
            if args.next().as_deref() != Some(std::ffi::OsStr::new("--runtime-dir")) {
                return Err("expected --runtime-dir after --pty-daemon".into());
            }
            let runtime_dir = args.next().ok_or("missing daemon runtime directory")?;
            suaegi_term::daemon::run(std::path::Path::new(&runtime_dir))?;
            return Ok(());
        }
        Some(value) if value == std::ffi::OsStr::new("--runtime-terminal-bridge") => {
            let config_path = suaegi_app::runtime_terminal_bridge::config_path_from_args(args)?;
            suaegi_app::runtime_terminal_bridge::run(&config_path)?;
            return Ok(());
        }
        _ => {}
    }

    suaegi_term::daemon::configure(suaegi_term::daemon::DaemonConfiguration {
        executable: std::env::current_exe()?,
        runtime_dir: suaegi_term::daemon::default_runtime_dir(),
    })?;

    if suaegi_app::cli::should_handle(&argv0, &collected) {
        return match suaegi_app::cli::run(collected) {
            Ok(code) => {
                if code != 0 {
                    std::process::exit(code);
                }
                Ok(())
            }
            Err(error) => {
                eprintln!("suaegi: {error}");
                std::process::exit(1);
            }
        };
    }

    suaegi_app::run()?;
    Ok(())
}
