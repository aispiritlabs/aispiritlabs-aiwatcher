import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { getWorkflowExecution, listDatasets, listExports, listModels } from '@/api/generated';
import { IdChip } from '@/shared/components/ui/primitives';

/**
 * A reference from one area's record into another's, resolved before it is
 * drawn.
 *
 * Every one of these is a join that exists in the data and that nothing could
 * click: a training run names a dataset whose spelling belongs to one of two
 * registries, an evaluation names an execution, and a model call names a
 * version of a registered model. The rule they share is in `DatasetReference`
 * below and it is why this file exists rather than each page writing an
 * `<a href>`: **resolve the target, or render the fact**. A link that 404s is
 * worse than a string somebody has to look up.
 *
 * The prompt half of the same idea lives in `prompt-bits.tsx`, beside the
 * registry rules it enforces — `PromptRefLink` is this file's fourth member in
 * everything but location.
 */

export function splitReference(reference: string, version?: string | null) {
  const at = reference.lastIndexOf('@');
  if (version && at >= 0 && reference.slice(at + 1) !== version) return undefined;
  return version
    ? { name: at < 0 ? reference : reference.slice(0, at), version }
    : at > 0 && at < reference.length - 1
      ? { name: reference.slice(0, at), version: reference.slice(at + 1) }
      : undefined;
}

/** Resolve the registry before making a link; the spelling alone is ambiguous. */
export function DatasetReference({
  reference,
  kind,
  version,
}: {
  reference: string;
  kind?: string | null;
  version?: string | null;
}) {
  const target = splitReference(reference, version);
  const curation = useQuery({
    queryKey: ['lineage-datasets'],
    enabled: Boolean(target) && (!kind || kind === 'curation'),
    retry: false,
    queryFn: async () => (await listDatasets({ throwOnError: true })).data,
    staleTime: 60_000,
  });
  const annotations = useQuery({
    queryKey: ['lineage-exports', target?.name],
    enabled: Boolean(target) && (!kind || kind === 'annotations'),
    retry: false,
    queryFn: async () =>
      (await listExports({ throwOnError: true, query: { name: target!.name } })).data,
    staleTime: 60_000,
  });
  const isCuration = curation.data?.datasets.some(
    (dataset) =>
      dataset.name === target?.name &&
      dataset.versions.some((entry) => entry.version === target?.version),
  );
  const isAnnotation = annotations.data?.exports.some((entry) => entry.export === target?.version);
  // With no declared kind, wait for both registries to answer. An unavailable
  // registry cannot prove that the same reference is unambiguous.
  const resolved =
    kind ||
    (curation.isSuccess &&
      (annotations.isSuccess || (annotations.error as { code?: string })?.code === 'not_found'));
  if (target && resolved && isCuration && !isAnnotation)
    return (
      <Link
        className="text-primary underline"
        to="/datasets"
        search={{ dataset: target.name, version: target.version }}
      >
        {reference} · curation
      </Link>
    );
  if (target && resolved && isAnnotation && !isCuration)
    return (
      <Link
        className="text-primary underline"
        to="/annotations/exports"
        search={{ project: target.name, export: target.version }}
      >
        {reference} · annotations
      </Link>
    );
  return <span title="No unambiguous retained registry target confirmed">{reference}</span>;
}

export function ExecutionReference({
  executionId,
  stepId,
}: {
  executionId: string;
  stepId?: string | null;
}) {
  const detail = useQuery({
    queryKey: ['lineage-execution', executionId],
    retry: false,
    queryFn: async () =>
      (await getWorkflowExecution({ throwOnError: true, path: { workflow_run_id: executionId } }))
        .data,
  });
  if (!detail.data)
    return (
      <span>
        execution {executionId}
        {stepId ? ` · step ${stepId}` : ''}
      </span>
    );
  return (
    <Link
      className="text-primary underline"
      to="/workflows"
      search={{
        workflow: detail.data.summary.workflow_id,
        execution: executionId,
        node: detail.data.nodes.some((node) => node.node_id === stepId)
          ? (stepId ?? undefined)
          : undefined,
        window: 0,
      }}
    >
      execution {executionId}
      {stepId ? ` · step ${stepId}` : ''}
    </Link>
  );
}

/** A registered model version is `sha256` of what makes it that version. */
const VERSION_ID = /^[0-9a-f]{64}$/;

/**
 * The registry version a call ran on, as a link into the model registry.
 *
 * `aiwatcher.model.version` comes from a `model_version` field on the call,
 * and two producers fill it with different things. The serving profile sends
 * the registry's version — a digest — beside `gen_ai.request.model`, which is
 * then the registry's model name, and that pair is the join this makes
 * clickable: from a span that answered badly to the run that trained it, the
 * export it learned from and the images behind that. A gateway sends the model
 * the provider actually served, which is a *name*, and is a perfectly good
 * fact about the call and no version of anything.
 *
 * So the digest is the admission: 64 hex, or this is a fact rather than a
 * link — the same guard, for the same reason, as `PromptRefLink`'s. The
 * registry is then asked whether it holds that name at all, because it is
 * optional (`RegistryDisabled`) and a deployment without one would otherwise
 * link every call to a 404.
 */
export function ModelVersionReference({ model, version }: { model: unknown; version: unknown }) {
  const usable = typeof version === 'string' && VERSION_ID.test(version);
  const name = typeof model === 'string' && model.length > 0 ? model : undefined;
  const registry = useQuery({
    queryKey: ['lineage-models'],
    enabled: usable && name !== undefined,
    retry: false,
    queryFn: async () => (await listModels({ throwOnError: true })).data,
    staleTime: 60_000,
  });

  if (!usable) {
    // Not a version this registry could hold. Shown, never linked, and never
    // hidden: what the provider served is worth reading.
    return <span>{typeof version === 'string' ? version : ''}</span>;
  }
  const short = (version as string).slice(0, 12);
  if (!name || !registry.data?.models.some((held) => held.name === name)) {
    return <IdChip value={short} full={version as string} label="model version" />;
  }
  return (
    <Link
      to="/training/models"
      search={{ model: name, version: version as string }}
      className="id text-primary hover:underline"
      title={`${name} @ ${version as string}`}
    >
      {name}@{short}
    </Link>
  );
}
