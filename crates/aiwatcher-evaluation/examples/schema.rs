//! The wire shape for SDK authors; semantic validation stays in Evaluation.

use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(components(schemas(aiwatcher_evaluation::EvaluationManifest)))]
struct Contract;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let document = serde_json::to_value(Contract::openapi())?;
    let schemas = serde_json::to_string(&document["components"]["schemas"])?;
    let definitions: serde_json::Value =
        serde_json::from_str(&schemas.replace("#/components/schemas/", "#/$defs/"))?;
    let schema = serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Evaluation manifest v1",
        "$ref": "#/$defs/EvaluationManifest",
        "$defs": definitions
    });
    println!("{}", serde_json::to_string_pretty(&schema)?);
    Ok(())
}
