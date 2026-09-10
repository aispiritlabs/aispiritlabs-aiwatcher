# Collector components that use OTTL

OTTL is supported across a wide range of Collector components, not just the transform and filter processors.

## Processors

| Component | Use case |
|-----------|----------|
| transform | Modify, enrich, or redact telemetry (set attributes, rename fields, truncate values) |
| filter | Drop telemetry entirely (discard metrics by name, drop spans by status, remove noisy logs) |
| attributes | Insert, update, delete, or hash resource and record attributes |
| span | Rename spans and set span status based on attribute values |
| tailsampling | Sample traces based on OTTL conditions (e.g., keep error traces, drop health checks) |
| cumulativetodelta | Convert cumulative metrics to delta temporality with OTTL-based metric selection |
| logdedup | Deduplicate log records using OTTL conditions |
| lookup | Enrich telemetry by looking up values from external tables using OTTL expressions |

## Connectors

| Component | Use case |
|-----------|----------|
| routing | Route telemetry to different pipelines based on OTTL conditions |
| count | Count spans, metrics, or logs matching OTTL conditions and emit as metrics |
| sum | Sum numeric values from telemetry matching OTTL conditions and emit as metrics |
| signaltometrics | Generate metrics from spans or logs using OTTL expressions for attribute extraction |

## Receivers

| Component | Use case |
|-----------|----------|
| hostmetrics | Filter host metrics at collection time using OTTL conditions |
