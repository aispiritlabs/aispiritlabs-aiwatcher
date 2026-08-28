//! Writes the OpenAPI document to `contracts/openapi.json`.
//!
//! The panel's TypeScript client is generated from that file, so it must never
//! drift from the Rust routes. `just openapi` regenerates it and CI runs
//! `just openapi-check`, which fails if the committed copy is stale — turning a
//! forgotten regeneration into a red build instead of a runtime `undefined`.

use std::io::Write;

use anyhow::{Context, Result};

use aiwatcher_api::ApiDoc;

fn main() -> Result<()> {
    let json = ApiDoc::to_json().context("serialising the OpenAPI document")?;

    match std::env::args().nth(1) {
        Some(path) => {
            if let Some(parent) = std::path::Path::new(&path).parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            // Trailing newline so the file is diff-friendly.
            std::fs::write(&path, format!("{json}\n"))
                .with_context(|| format!("writing {path}"))?;
            eprintln!("wrote {path}");
        }
        None => {
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            writeln!(handle, "{json}").context("writing to stdout")?;
        }
    }
    Ok(())
}
