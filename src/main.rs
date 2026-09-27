// musl's allocator is slow under multithreaded load (static Linux release build).
#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    if let Err(err) = mx::run(std::env::args_os()) {
        mx::output::fatal(&err);
    }
}
