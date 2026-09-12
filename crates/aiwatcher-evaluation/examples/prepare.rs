//! Validate and fingerprint a manifest locally; this writes no registry data.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: prepare <manifest.json>")?;
    let file = std::fs::File::open(path)?;
    use std::io::Read;
    let mut bytes = Vec::new();
    file.take(aiwatcher_evaluation::MAX_MANIFEST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > aiwatcher_evaluation::MAX_MANIFEST_BYTES {
        return Err("manifest exceeds 256 KiB".into());
    }
    let manifest = serde_json::from_slice(&bytes)?;
    let prepared = aiwatcher_evaluation::Evaluation::prepare(manifest)?;
    println!("{}", serde_json::to_string_pretty(&prepared)?);
    // The address of the approval that would admit this pair, which is also the
    // name of the directory an operator stages its bundle in.
    println!(
        "approval_id: {}",
        aiwatcher_evaluation::approval_id(prepared.variant_id(), prepared.context_id())?
    );
    Ok(())
}
