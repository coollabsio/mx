//! Output helpers. Global settings live in [`crate::globals`].

pub fn quiet() -> bool {
    crate::globals::quiet()
}

pub fn insecure() -> bool {
    crate::globals::insecure()
}

pub fn print_plain(message: &str) {
    if !quiet() {
        println!("{message}");
    }
}
