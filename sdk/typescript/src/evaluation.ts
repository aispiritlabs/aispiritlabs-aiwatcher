/** Wire contracts only. The Evaluation facade validates and computes IDs.
 * Constructing this object does not store a result or verify artifact access.
 * ADR_0030; kept separate from the best-effort telemetry entry point.
 */

export interface ArtifactReference {
  name: string;
  uri: string;
  digest: string;
  size_bytes: number;
  content_type?: string;
  kind?: 'rows' | 'code' | 'preview' | 'log' | 'model' | 'report' | 'blob';
  schema_ref?: string | null;
}

export interface VersionReference {
  name: string;
  version: string;
}

export interface DatasetReference {
  kind: 'curation' | 'annotations' | 'conversations' | 'external' | 'assessments';
  name: string;
  version: string;
}

/** A metric a scorer framework measured, through the deployment's scorer service. */
export interface ExternalMeasure {
  adapter: VersionReference;
  metric: string;
  /** The model that graded it; its numbers are not reproduced by re-reading. */
  model?: VersionReference | null;
}

export interface MetricDefinition {
  name: string;
  unit: string;
  direction: 'higher' | 'lower' | 'none';
  aggregation: 'mean' | 'sum' | 'min' | 'max' | 'rate' | 'none';
  /** Derived by the server for a framework's metric; absent otherwise. */
  measured_by?: ExternalMeasure | null;
}

export interface JudgeConfiguration {
  provider: string;
  model: VersionReference;
  configuration: ArtifactReference;
  calibration_dataset: DatasetReference;
  /** The judge is sent words from the conversation archive. Derived by the server; absent when false. */
  reads_archive?: boolean;
}

export interface EvaluationContext {
  dataset: DatasetReference;
  case_manifest: ArtifactReference;
  case_count: number;
  split: string;
  suite: VersionReference;
  scorer: VersionReference;
  input_schema: ArtifactReference;
  expectations_schema: ArtifactReference;
  judge?: JudgeConfiguration | null;
  metrics: MetricDefinition[];
}

export interface VariantManifest {
  schema_version: 1;
  experiment_id: string;
  dataset: DatasetReference;
  model?: VersionReference | null;
  prompt?: VersionReference | null;
  code: ArtifactReference;
  generation_config: ArtifactReference;
  response_schema?: ArtifactReference | null;
  tools?: ArtifactReference | null;
  workflow?: VersionReference | null;
}

export interface EvaluationOrigin {
  evaluation_id: string;
  repetition_id: string;
  execution_id?: string | null;
  step_id?: string | null;
}

export interface EvaluationManifest {
  schema_version: 1;
  origin: EvaluationOrigin;
  variant: VariantManifest;
  context: EvaluationContext;
}
