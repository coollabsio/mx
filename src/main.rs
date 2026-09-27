fn main() {
    if let Err(err) = mx::run(std::env::args_os()) {
        mx::output::fatal(&err);
    }
}
