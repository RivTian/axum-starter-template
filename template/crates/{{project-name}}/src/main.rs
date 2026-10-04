//! Entry point of the service binary.

/// With the `mimalloc` feature, mimalloc replaces the system allocator; it can help a service
/// that allocates a lot from many threads. Measure before and after.
#[cfg(feature = "mimalloc")]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> std::process::ExitCode {
    svc_app::main()
}
