import * as React from 'react';
import { useQuery } from '@tanstack/react-query';
import { ScrollText } from 'lucide-react';

import { artifactContent, runArtifacts } from '@/api/generated/sdk.gen';
import type { CatalogedArtifact } from '@/api/generated/types.gen';
import { ApiFailure, answerOf } from '@/shared/lib/result';

import { Button, Spinner } from '@/shared/components/ui/primitives';

/**
 * What a step's pod printed, read back.
 *
 * A step that runs in a pod of its own (ADR_0029) has its last 256 KiB kept as
 * a `Log` artifact against the attempt that printed it. This is the other end
 * of that: "reading why a stage failed" is the scenario the requirement is
 * written around, and until there was a route to list what a run produced the
 * log was something this system kept and nobody could open.
 *
 * One component for both places a managed run is watched — the curation
 * pipeline's run card and the Workflows view — for `AnswerGate`'s reason:
 * which logs belong to this step is one question, and two copies of it would
 * be free to disagree.
 *
 * **Every attempt, newest first.** A retry that succeeded is the uninteresting
 * one; the question is almost always what the attempt before it said. They are
 * separate artifacts because each is named by its own bytes, and the catalog
 * row is what says which attempt printed it.
 *
 * **The text is fetched on a click, one at a time.** The list is metadata —
 * digests, sizes, which attempt — and costs a step view nothing; the bytes are
 * a quarter of a megabyte each and belong to whichever one somebody opened.
 */
export function StepLog({ executionId, stepId }: { executionId: string; stepId: string }) {
  const produced = useQuery({
    queryKey: ['execution', executionId, 'artifacts'],
    retry: false,
    queryFn: async (): Promise<CatalogedArtifact[]> =>
      answerOf(
        await runArtifacts({ path: { execution_id: executionId } }),
        'What this run produced could not be read.',
      ),
  });

  if (produced.isPending) return null;
  if (produced.isError) return <UnreadableLog error={produced.error} />;

  const logs = produced.data
    .filter((row) => row.artifact.kind === 'log' && row.produced_by?.step_id === stepId)
    .sort((a, b) => (b.produced_by?.attempt ?? 0) - (a.produced_by?.attempt ?? 0));
  // A step that ran in this process printed into this process's own log and
  // has no artifact of its own, which is the ordinary case and not a gap.
  if (logs.length === 0) return null;

  return (
    <div className="flex flex-col gap-1">
      {logs.map((row) => (
        <OneLog
          key={row.artifact.digest}
          executionId={executionId}
          artifact={row.artifact}
          attempt={row.produced_by?.attempt ?? 0}
        />
      ))}
    </div>
  );
}

/**
 * Why there is no log to offer, when the reason is not "there is none".
 *
 * Said rather than drawn as an empty state — a 501 from a deployment with no
 * object store and a 503 from one whose store is down are the server saying
 * nothing usable, and rendering either as "this step printed nothing" is the
 * failure R6 is about. Not `role="alert"`, though: nobody issued a refusal
 * here and nothing on this card stopped working, so it is a sentence and not
 * an announcement.
 */
function UnreadableLog({ error }: { error: unknown }) {
  const message =
    error instanceof ApiFailure && error.status === 501
      ? error.message
      : 'This step’s log could not be read.';
  return <p className="text-xs text-muted-foreground">{message}</p>;
}

function OneLog({
  executionId,
  artifact,
  attempt,
}: {
  executionId: string;
  artifact: CatalogedArtifact['artifact'];
  attempt: number;
}) {
  const [open, setOpen] = React.useState(false);
  const content = useQuery({
    queryKey: ['execution', executionId, 'artifact', artifact.digest],
    enabled: open,
    retry: false,
    queryFn: async () =>
      answerOf(
        await artifactContent({
          path: { execution_id: executionId, digest: artifact.digest },
        }),
        'That log could not be read.',
      ),
  });

  return (
    <div className="flex flex-col gap-1">
      <div>
        <Button variant="outline" size="sm" onClick={() => setOpen(!open)}>
          <ScrollText className="mr-1 h-3 w-3" />
          {open ? 'Hide' : 'Read'} the log from attempt {attempt}
          {artifact.size_bytes ? ` · ${kib(artifact.size_bytes)}` : null}
        </Button>
      </div>
      {open ? (
        content.isPending ? (
          <span className="flex items-center gap-2 text-xs text-muted-foreground">
            <Spinner /> Reading it…
          </span>
        ) : content.isError ? (
          <p className="text-xs text-muted-foreground">
            {content.error instanceof Error ? content.error.message : 'That log could not be read.'}
          </p>
        ) : (
          // Bounded where it was written — the last 256 KiB, and the stored
          // object's own first line says how many bytes came before it, so
          // nothing here has to explain the bound a second time.
          <pre className="max-h-80 overflow-auto whitespace-pre-wrap rounded border border-border bg-muted/40 p-2 font-mono text-xs">
            {content.data.text}
          </pre>
        )
      ) : null}
    </div>
  );
}

function kib(bytes: number): string {
  return bytes < 1024 ? `${bytes} B` : `${Math.round(bytes / 1024)} KiB`;
}
