//! The aiwatcher binary.
//!
//! One binary that runs an instance and talks to one. Called with no arguments
//! it runs the server, because that is the container image's entry point and a
//! deployment must not start printing usage text on an upgrade; `aiwatcher
//! help` is the help. See [`aiwatcher_cli`].

fn main() -> std::process::ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("aiwatcher: cannot start an async runtime: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };

    match runtime.block_on(aiwatcher_cli::run(std::env::args().skip(1))) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            // The whole chain, not just the outermost line. `anyhow`'s Display
            // prints only the last context added, so a failure to bind reads as
            // "binding 127.0.0.1:8080" with the reason — already in use, or not
            // permitted — dropped on the floor. That is the one sentence
            // somebody needs.
            eprintln!("aiwatcher: {error}");
            let mut cause = std::error::Error::source(&error);
            while let Some(inner) = cause {
                eprintln!("  caused by: {inner}");
                cause = std::error::Error::source(inner);
            }
            // Distinguished rather than a flat failure, so a wrapper script can
            // retry a temporary one and never retry a mistyped one.
            std::process::ExitCode::from(
                u8::try_from(error.exit_code()).unwrap_or(1),
            )
        }
    }
}
