import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { getWorkflowExecution, listDatasets, listExports } from '@/api/generated';

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
