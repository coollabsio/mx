fn main() {
    if let Err(err) = mx::run(std::env::args_os()) {
        eprintln!("mx: {err:#}");
        std::process::exit(1);
    }
}
