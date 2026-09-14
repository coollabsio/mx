fn main() {
    if let Err(err) = mx::run(std::env::args_os()) {
        let name = std::env::args_os()
            .next()
            .and_then(|path| std::path::PathBuf::from(path).file_name().map(Into::into))
            .and_then(|name: std::ffi::OsString| name.into_string().ok())
            .unwrap_or_else(|| "mx".to_string());
        eprintln!("{name}: {err:#}");
        std::process::exit(1);
    }
}
