use std::sync::atomic::{AtomicBool, Ordering};

static QUIET: AtomicBool = AtomicBool::new(false);
static INSECURE: AtomicBool = AtomicBool::new(false);

pub fn configure(quiet: bool, insecure: bool) {
    QUIET.store(quiet, Ordering::SeqCst);
    INSECURE.store(insecure, Ordering::SeqCst);
}

pub fn quiet() -> bool {
    QUIET.load(Ordering::SeqCst)
}

pub fn insecure() -> bool {
    INSECURE.load(Ordering::SeqCst)
}

pub fn print_plain(message: &str) {
    if !quiet() {
        println!("{message}");
    }
}
