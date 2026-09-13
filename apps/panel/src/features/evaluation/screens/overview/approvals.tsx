/**
 * The operator's half of durable evidence: which pairs this instance admits.
 *
 * A pair is a variant and the context it was measured in, and admitting one is
 * what lets every later repetition publish on its own (ADR_0030). It was an API
 * call and a directory on the server's host; the bundle now arrives over the
 * same API, so this is the whole act.
 *
 * Nothing here computes an approval ID. It is a digest over the canonicalised
 * declaration, and a second implementation of that rule in TypeScript would be
 * a second answer to what a pair is — the server addresses it.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import * as React from 'react';

import {
  addressApproval,
  approveSource,
  listApprovals,
  listBundle,
  stageBundle,
  withdrawApproval,
} from '@/api/generated/sdk.gen';
import type { Approval, EvaluationManifest } from '@/api/generated/types.gen';
import { needsRole, useRoleDecision } from '@/shared/lib/auth';
import { answerOf, ApiFailure } from '@/shared/lib/result';
import {
  Badge,
  Button,
  Card,
  EmptyState,
  IdChip,
  Spinner,
} from '@/shared/components/ui/primitives';
import { pinchId } from '@/shared/lib/utils';

/** A deadline is a date; so is an approval. `formatTime` is time of day. */
function on(seconds: number): string {
  return new Date(seconds * 1000).toLocaleDateString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  });
}

export function Approvals() {
  const admin = useRoleDecision('admin');
  const approvals = useQuery({
    queryKey: ['evaluation-approvals'],
    queryFn: async () => answerOf(await listApprovals(), 'could not read the approvals'),
    retry: false,
  });
  const failure = approvals.error instanceof ApiFailure ? approvals.error : undefined;
  const rows = approvals.data?.approvals ?? [];
  return (
    <Card className="flex flex-col gap-3 p-4">
      <div>
        <h2 className="text-sm font-semibold">Approvals</h2>
        <p className="text-xs text-muted-foreground">
          Which variant and context this instance admits evidence for. Every repetition of an
          admitted pair publishes on its own; withdrawing one hides every result measured under it.
        </p>
      </div>
      <Admit disabled={admin === false} />
      {failure ? (
        <p className="text-xs text-danger">
          {failure.status === 501
            ? 'This instance keeps no durable evidence, so it admits no pairs.'
            : `Could not read the approvals: ${failure.message}`}
        </p>
      ) : approvals.isLoading ? (
        <Spinner />
      ) : rows.length === 0 ? (
        <EmptyState
          title="No pair admitted yet"
          hint="Stage a bundle above. Until a pair is admitted, publishing evidence for it is refused."
        />
      ) : (
        <ul className="flex flex-col divide-y divide-border/40">
          {rows.map((approval) => (
            <ApprovalRow key={approval.record.approval_id} approval={approval} admin={admin} />
          ))}
        </ul>
      )}
    </Card>
  );
}

function ApprovalRow({ approval, admin }: { approval: Approval; admin: boolean | undefined }) {
  const { record, withdrawn } = approval;
  const queries = useQueryClient();
  const [confirming, setConfirming] = React.useState(false);
  const staged = useQuery({
    queryKey: ['evaluation-bundle', record.approval_id],
    queryFn: async () =>
      answerOf(
        await listBundle({ path: { approval_id: record.approval_id } }),
        'could not read the staged bundle',
      ),
    retry: false,
  });
  const withdraw = useMutation({
    mutationFn: async () =>
      answerOf(
        await withdrawApproval({ path: { approval_id: record.approval_id } }),
        'could not withdraw this approval',
      ),
    onSuccess: () => {
      setConfirming(false);
      void queries.invalidateQueries({ queryKey: ['evaluation-approvals'] });
      void queries.invalidateQueries({ queryKey: ['evaluation-evidence'] });
    },
  });
  return (
    <li className="flex flex-wrap items-center justify-between gap-2 py-2 text-xs">
      <div className="flex min-w-0 flex-col gap-1">
        <div className="flex flex-wrap items-center gap-2">
          <IdChip
            label="variant"
            value={pinchId(record.variant_id, 8, 6)}
            full={record.variant_id}
          />
          <IdChip
            label="context"
            value={pinchId(record.context_id, 8, 6)}
            full={record.context_id}
          />
          {withdrawn ? (
            <Badge tone="warning">withdrawn</Badge>
          ) : (
            <Badge tone="primary">admits</Badge>
          )}
        </div>
        <span className="text-muted-foreground">
          {withdrawn
            ? `Withdrawn by ${withdrawn.withdrawn_by} on ${on(withdrawn.withdrawn_at)} — final for this pair.`
            : `Admitted by ${record.approved_by} on ${on(record.approved_at)}`}
          {staged.data ? ` · ${staged.data.length} staged` : ''}
        </span>
      </div>
      {withdrawn ? null : confirming ? (
        <span className="flex items-center gap-2">
          <span className="text-danger">
            Hide every result of this pair? This cannot be undone.
          </span>
          <Button
            size="sm"
            variant="outline"
            onClick={() => withdraw.mutate()}
            disabled={withdraw.isPending}
          >
            Withdraw
          </Button>
          <Button size="sm" variant="outline" onClick={() => setConfirming(false)}>
            Keep
          </Button>
        </span>
      ) : (
        <Button
          size="sm"
          variant="outline"
          disabled={admin === false}
          title={admin === false ? needsRole('admin') : undefined}
          onClick={() => setConfirming(true)}
        >
          Withdraw…
        </Button>
      )}
      {withdraw.error ? (
        <span className="text-danger">{(withdraw.error as Error).message}</span>
      ) : null}
    </li>
  );
}

/**
 * Stage a bundle and admit the pair it declares, in one act.
 *
 * The order is the server's: the bytes go up first, then the approval resolves
 * the whole bundle and records its digest. Staging on its own admits nothing,
 * so a half-finished upload leaves an instance exactly as it was.
 */
function Admit({ disabled }: { disabled: boolean }) {
  const queries = useQueryClient();
  const [files, setFiles] = React.useState<File[]>([]);
  // The declaration's own word that its judge reads the archive, read off the
  // chosen manifest.json so the admin hears it before admitting — never worked
  // out here.
  const [readsArchive, setReadsArchive] = React.useState(false);
  const [acknowledged, setAcknowledged] = React.useState(false);
  const input = React.useRef<HTMLInputElement>(null);
  const admit = useMutation({
    mutationFn: async (chosen: File[]) => {
      const declaration = chosen.find((file) => nameOf(file) === 'manifest.json');
      if (!declaration) throw new Error('The bundle needs a manifest.json: it is the declaration.');
      const manifest = JSON.parse(await declaration.text()) as EvaluationManifest;
      const address = answerOf(
        await addressApproval({ body: manifest }),
        'this declaration is not one this instance can address',
      );
      for (const file of chosen) {
        answerOf(
          await stageBundle({
            path: { approval_id: address.approval_id, name: nameOf(file) },
            body: new Uint8Array(await file.arrayBuffer()) as unknown as Array<number>,
          }),
          `could not stage ${nameOf(file)}`,
        );
      }
      return answerOf(
        await approveSource({ body: manifest }),
        'the bundle was staged, and admitting the pair was refused',
      );
    },
    onSuccess: () => {
      setFiles([]);
      setReadsArchive(false);
      setAcknowledged(false);
      if (input.current) input.current.value = '';
      void queries.invalidateQueries({ queryKey: ['evaluation-approvals'] });
      void queries.invalidateQueries({ queryKey: ['evaluation-evidence'] });
    },
  });
  return (
    <form
      className="flex flex-wrap items-center gap-2 rounded border border-border p-3 text-xs"
      onSubmit={(event) => {
        event.preventDefault();
        admit.mutate(files);
      }}
    >
      <label className="flex items-center gap-2">
        Bundle
        <input
          ref={input}
          type="file"
          multiple
          aria-label="Bundle files"
          onChange={(event) => {
            const chosen = Array.from(event.target.files ?? []);
            setFiles(chosen);
            setAcknowledged(false);
            const declaration = chosen.find((file) => nameOf(file) === 'manifest.json');
            setReadsArchive(false);
            void declaration
              ?.text()
              .then((text) => {
                const manifest = JSON.parse(text) as EvaluationManifest;
                // Any of these says the archive's words leave it: a judge that
                // reads it, a scorer service measuring a conversation cohort, or
                // one held against people judged on conversation evidence.
                setReadsArchive(
                  manifest.context?.judge?.reads_archive === true ||
                    manifest.context?.external_calibration?.reads_archive === true ||
                    (manifest.context?.dataset.kind === 'conversations' &&
                      (manifest.context?.metrics ?? []).some((metric) => metric.measured_by)),
                );
              })
              // Unreadable here is refused by the server on submit, with why.
              .catch(() => setReadsArchive(false));
          }}
          className="rounded border border-border bg-background p-1"
        />
      </label>
      {readsArchive ? (
        <div role="alert" className="flex w-full flex-col gap-1 text-danger">
          <p>
            This declaration sends words from the conversation archive to a judge or a scorer
            service. Admitting it lets every run of this pair send them, outside the archive&apos;s
            encryption, retention and erasure.
          </p>
          <label className="flex items-center gap-2 text-foreground">
            <input
              type="checkbox"
              aria-label="Acknowledge what this pair sends"
              checked={acknowledged}
              onChange={(event) => setAcknowledged(event.target.checked)}
            />
            I understand what this pair sends.
          </label>
        </div>
      ) : null}
      <Button
        size="sm"
        type="submit"
        disabled={
          disabled || files.length === 0 || admit.isPending || (readsArchive && !acknowledged)
        }
      >
        {admit.isPending ? 'Staging…' : 'Stage and admit'}
      </Button>
      {disabled ? <span className="text-muted-foreground">{needsRole('admin')}</span> : null}
      {admit.error ? <span className="text-danger">{(admit.error as Error).message}</span> : null}
      {admit.isSuccess ? (
        <span className="text-muted-foreground">
          Admitted. Every repetition of this pair now publishes without a step here.
        </span>
      ) : null}
    </form>
  );
}

/** A model's artifacts live in the one folder a bundle has. */
function nameOf(file: File): string {
  const path = (file as File & { webkitRelativePath?: string }).webkitRelativePath ?? '';
  return path.includes('model-artifacts/') ? `model-artifacts/${file.name}` : file.name;
}
