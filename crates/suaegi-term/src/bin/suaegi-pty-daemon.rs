fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--pty-daemon"))
        || args.next().as_deref() != Some(std::ffi::OsStr::new("--runtime-dir"))
    {
        return Err("usage: suaegi-pty-daemon --pty-daemon --runtime-dir <path>".into());
    }
    let runtime_dir = args.next().ok_or("missing runtime directory")?;
    suaegi_term::daemon::run(std::path::Path::new(&runtime_dir))?;
    Ok(())
}
