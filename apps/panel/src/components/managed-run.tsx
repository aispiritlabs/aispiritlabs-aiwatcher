import * as React from 'react';
import { Link } from '@tanstack/react-router';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Ban, FileCode2, Pause, Play, RotateCcw, SquareArrowOutUpRight } from 'lucide-react';

import {
  cancelExecution,
  getExecution,
  pauseExecution,
  openEditor,
  provideInput,
  resumeExecution,
  retryStep,
  runBlocks,
  stepContext,
} from '@/api/generated/sdk.gen';
import type { ContextSnapshot, RunAction, RunView, StateType } from '@/api/generated/types.gen';
import { openWorkflowStream } from '@/lib/live';
import { getNotebookRevision } from '@/lib/ml-pipeline';
import { answerOf, answerOrNone } from '@/lib/result';

import { Badge, Button, Card, IdChip, Refusal, Spinner } from './ui/primitives';

/**
 * A run the server owns, followed rather than driven.
 *
 * The stream is the **signal** and the run's own page is the **truth**. A frame
 * says something happened; the store's projection — written in the same
 * transaction as the decision that caused it — says what the state now is. That
 * split is why nothing here parses a frame's payload: the facts on the log are
 * what the workflow waterfall draws, and what this page needs is the per-step
 * state, which only the projection has.
 *
 * There is no `/executions/{id}/stream`. ADR_0026 put every fact of a managed
 * run on the log carrying the execution as its `workflow_run_id`, so the
 * workflow stream already follows one — a second route would be a second image
 * of one run.
 */
export function useManagedRun(executionId: string | undefined) {
  const queryClient = useQueryClient();
  const key = ['execution', executionId] as const;

  const run = useQuery({
    queryKey: key,
    enabled: Boolean(executionId),
    // A stale link and a pruned run are the same 404 and neither is worth
    // three more requests: the card offers to forget it instead. Everything
    // else *is* a failure and is not offered that — telling somebody their run
    // was forgotten because a session expired is the thing R6 is about.
    retry: false,
    queryFn: async () =>
      answerOrNone(
        await getExecution({ path: { execution_id: executionId ?? '' } }),
        'That run could not be read.',
      ),
  });

  React.useEffect(() => {
    if (!executionId) return undefined;
    return openWorkflowStream(executionId, undefined, {
      onEvent: () => void queryClient.invalidateQueries({ queryKey: ['execution', executionId] }),
      // The phase says whether the stream caught up or went live, which matters
      // to a waterfall and not to a state that is re-read either way.
      onPhase: () => {},
    });
  }, [executionId, queryClient]);

  return run;
}

/**
 * Which authored blocks this run's steps came from, asked once.
 *
 * A plan is immutable, so this answer cannot change while the run does — which
 * is why it is a separate request with no refetch rather than a field on the
 * run's own page, re-sent on every frame and on every command.
 *
 * The mapping is the server's and is never worked out here: three source and
 * transform boxes fold into one Flow query, and a browser deciding which is
 * which would decide it from the draft on screen rather than from what the run
 * compiled.
 */
export function useManagedBlocks(executionId: string | undefined) {
  return useQuery({
    queryKey: ['execution', executionId, 'blocks'],
    enabled: Boolean(executionId),
    retry: false,
    // Immutable. Asking again would be asking a question whose answer is
    // already known to be the same one.
    staleTime: Infinity,
    queryFn: async () =>
      answerOrNone(
        await runBlocks({ path: { execution_id: executionId ?? '' } }),
        'The blocks this run came from could not be read.',
      ),
  });
}

/** What a state is called, and how loudly. */
function tone(state: StateType) {
  switch (state) {
    case 'completed':
      return 'success' as const;
    case 'failed':
    case 'crashed':
    case 'cancelled':
      return 'danger' as const;
    case 'running':
      return 'running' as const;
    case 'paused':
    case 'awaiting_input':
      return 'warning' as const;
    default:
      return 'neutral' as const;
  }
}

/** The word and the icon for a run-level action. The server decides *whether*. */
const RUN_ACTIONS: Record<RunAction, { label: string; icon: typeof Pause }> = {
  pause: { label: 'Pause', icon: Pause },
  resume: { label: 'Resume', icon: Play },
  cancel: { label: 'Cancel', icon: Ban },
};

/**
 * The run, its steps, what may be done to it, and where to watch it properly.
 *
 * Deliberately not a waterfall: that view exists, it is fed by the same events,
 * and drawing a second one here would be two pictures of one run that can
 * disagree while one of them is a frame behind.
 *
 * **Which buttons exist is not decided here.** `RunView.allowed` and
 * `ContextSnapshot.allowed` come from the server, each computed exactly where
 * `decide` would accept the command — so a button that 409s is not something
 * this component can render by having a stale idea of the rules.
 */
export function ManagedRunCard({
  run,
  executionId,
  pending,
  missing,
  failure,
  onForget,
}: {
  run?: RunView;
  executionId?: string;
  pending: boolean;
  /** The server answered, and there is no such run. Not "the read failed". */
  missing?: boolean;
  /** The read did not get an answer it could use. Never rendered as absence. */
  failure?: unknown;
  onForget: () => void;
}) {
  const queryClient = useQueryClient();
  const [open, setOpen] = React.useState<string>();
  const invalidate = () =>
    void queryClient.invalidateQueries({ queryKey: ['execution', executionId] });

  const command = useMutation({
    // Read through `answerOf`, because the generated client resolves on a
    // refusal: without it a 403 or a 409 ran `onSuccess`, the run was
    // re-read, nothing had changed and nothing said why.
    mutationFn: async (action: RunAction) => {
      const path = { execution_id: executionId ?? '' };
      const refused = `That ${action} was refused.`;
      if (action === 'pause') return answerOf(await pauseExecution({ path }), refused);
      if (action === 'resume') return answerOf(await resumeExecution({ path }), refused);
      return answerOf(
        await cancelExecution({ path, body: { reason: 'stopped from the pipeline view' } }),
        refused,
      );
    },
    onSuccess: invalidate,
  });

  if (failure) {
    // Not the missing card, and deliberately without "Forget it": the run may
    // be perfectly well there, and offering to drop it from the address bar
    // would be inviting somebody to lose the only link to it over an outage.
    return (
      <Card className="flex flex-col gap-2 p-3 text-sm">
        <Refusal error={failure} fallback="That run could not be read." className="text-sm" />
        <span className="text-xs text-muted-foreground">
          The run is on the server either way. This page comes back to it by the id in the address
          bar.
        </span>
      </Card>
    );
  }

  if (missing) {
    // A link to a run that is not there. Two ways to get here and neither is a
    // failure: retention forgot a finished one, or the link came from
    // somewhere else entirely.
    return (
      <Card className="flex flex-wrap items-center gap-2 p-3 text-sm">
        <span className="text-muted-foreground">
          No run under this id. It may have finished long enough ago to be forgotten.
        </span>
        <Button variant="outline" size="sm" className="ml-auto" onClick={onForget}>
          Forget it
        </Button>
      </Card>
    );
  }

  if (!run) {
    return (
      <Card className="p-3 text-sm">
        {pending ? (
          <span className="flex items-center gap-2 text-muted-foreground">
            <Spinner /> Starting a run on the server…
          </span>
        ) : null}
      </Card>
    );
  }

  const { execution } = run;

  return (
    <Card className="flex flex-col gap-2 p-3 text-sm">
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone={tone(execution.state.state_type)}>
          {execution.state.name || execution.state.state_type}
        </Badge>
        <span className="text-muted-foreground">running on the server as</span>
        <IdChip
          label="execution"
          value={execution.execution_id.slice(0, 12)}
          full={execution.execution_id}
        />
        <div className="ml-auto flex items-center gap-2">
          {run.allowed.map((action) => {
            const { label, icon: Icon } = RUN_ACTIONS[action];
            return (
              <Button
                key={action}
                variant="outline"
                size="sm"
                disabled={command.isPending}
                onClick={() => command.mutate(action)}
              >
                <Icon className="mr-1 h-3 w-3" /> {label}
              </Button>
            );
          })}
          <Link
            to="/workflows"
            search={{ workflow: execution.definition_name, execution: execution.execution_id }}
            className="text-xs text-primary hover:underline"
          >
            Watch the waterfall →
          </Link>
        </div>
      </div>

      {command.isError ? (
        <Refusal error={command.error} fallback="That command was refused." />
      ) : null}

      <ul className="flex flex-col gap-1">
        {execution.steps.map((step) => (
          <li key={step.step_id} className="flex flex-col gap-1">
            <button
              type="button"
              className="flex items-center gap-2 text-left text-xs hover:underline"
              onClick={() =>
                setOpen((current) => (current === step.step_id ? undefined : step.step_id))
              }
            >
              <Badge tone={tone(step.state.state_type)}>{step.state.state_type}</Badge>
              <span className="font-medium">{step.step_id}</span>
              <span className="text-muted-foreground">{step.runtime}</span>
              {step.current_attempt > 1 ? (
                <span className="text-muted-foreground">attempt {step.current_attempt}</span>
              ) : null}
              {step.state.name ? (
                <span className="min-w-0 truncate text-muted-foreground">{step.state.name}</span>
              ) : null}
            </button>
            {open === step.step_id && executionId ? (
              <StepActions
                executionId={executionId}
                stepId={step.step_id}
                onDone={() => {
                  setOpen(undefined);
                  invalidate();
                }}
              />
            ) : null}
          </li>
        ))}
      </ul>

      <p className="text-xs text-muted-foreground">
        This run belongs to the server. Closing the tab does not stop it, and this page comes back
        to it by the id in the address bar.
      </p>
    </Card>
  );
}

/**
 * What may be done with one step, asked for when somebody opens it.
 *
 * One request per intent rather than one per poll: the context is a load and a
 * replay on the server, and asking for every step on every frame would be that
 * work for a card nobody is looking at. It is also what the route was built
 * for — reopening a block as it was.
 */
function StepActions({
  executionId,
  stepId,
  onDone,
}: {
  executionId: string;
  stepId: string;
  onDone: () => void;
}) {
  const context = useQuery({
    queryKey: ['execution', executionId, 'step', stepId],
    retry: false,
    queryFn: async (): Promise<ContextSnapshot> =>
      answerOf(
        await stepContext({ path: { execution_id: executionId, step_id: stepId } }),
        'That step could not be read.',
      ),
  });

  const [answer, setAnswer] = React.useState('');
  const waiting = context.data?.state?.awaiting;
  // The attempt that asked, named rather than assumed. An answer typed against
  // a question a retry has since replaced is refused by name — which is the
  // point of sending it, so it must be the attempt this context was read at.
  const attempt = context.data?.state?.current_attempt ?? 0;

  const retry = useMutation({
    mutationFn: async () =>
      answerOf(
        await retryStep({ path: { execution_id: executionId, step_id: stepId } }),
        'That step could not be taken again.',
      ),
    onSuccess: onDone,
  });

  const answerIt = useMutation({
    mutationFn: async (response: string) =>
      answerOf(
        await provideInput({
          path: { execution_id: executionId, step_id: stepId },
          // The answer and the attempt only; `answered_by` comes from the
          // session and the body refuses an unknown field rather than
          // ignoring it.
          body: { attempt, response },
        }),
        'That answer was refused.',
      ),
    onSuccess: onDone,
  });

  if (context.isPending) {
    return (
      <span className="flex items-center gap-2 pl-2 text-xs text-muted-foreground">
        <Spinner /> Reading this step…
      </span>
    );
  }
  if (context.isError || !context.data) {
    return (
      <Refusal error={context.error} fallback="That step could not be read." className="pl-2" />
    );
  }

  const allowed = context.data.allowed;
  const runtime = context.data.runtime;
  return (
    <div className="flex flex-col gap-2 border-l border-border pl-2 text-xs">
      <div className="flex flex-wrap items-center gap-2 text-muted-foreground">
        <span>context</span>
        <IdChip
          label="key"
          value={context.data.context_id.slice(0, 24)}
          full={context.data.context_id}
        />
      </div>

      {runtime.runtime === 'marimo' ? (
        <>
          <PinnedNotebook notebook={runtime.notebook} revision={runtime.code_revision} />
          <OpenEditor executionId={executionId} stepId={stepId} />
        </>
      ) : null}

      {allowed.includes('retry') ? (
        <div className="flex flex-col gap-1">
          <div>
            <Button
              variant="outline"
              size="sm"
              disabled={retry.isPending}
              onClick={() => retry.mutate()}
            >
              <RotateCcw className="mr-1 h-3 w-3" /> Take this step again
            </Button>
          </div>
          {/* `allowed` is computed where `decide` would accept the command, so
              a refusal here is a state that moved between the read and the
              press — which is exactly the sentence worth showing rather than a
              button that appears to do nothing. */}
          {retry.isError ? (
            <Refusal error={retry.error} fallback="That step could not be taken again." />
          ) : null}
        </div>
      ) : null}

      {allowed.includes('answer') && waiting ? (
        <div className="flex flex-col gap-2">
          <p>{waiting.prompt}</p>
          {waiting.choices?.length ? (
            // Buttons rather than a text box: `decide` refuses anything that is
            // not one of these by name, so a free-typed answer would be a 409
            // somebody had to read the error of to discover.
            <div className="flex flex-wrap gap-2">
              {waiting.choices.map((choice) => (
                <Button
                  key={choice}
                  variant="outline"
                  size="sm"
                  disabled={answerIt.isPending}
                  onClick={() => answerIt.mutate(choice)}
                >
                  {choice}
                </Button>
              ))}
            </div>
          ) : (
            <form
              className="flex flex-wrap items-center gap-2"
              onSubmit={(event) => {
                event.preventDefault();
                answerIt.mutate(answer);
              }}
            >
              <input
                className="h-8 min-w-48 flex-1 rounded-md border border-border bg-background px-2"
                placeholder="Your answer"
                value={answer}
                onChange={(event) => setAnswer(event.target.value)}
              />
              <Button type="submit" size="sm" disabled={answerIt.isPending || !answer.trim()}>
                Answer
              </Button>
            </form>
          )}
          {answerIt.isError ? (
            <Refusal error={answerIt.error} fallback="That answer was refused." />
          ) : null}
        </div>
      ) : null}

      {!allowed.includes('retry') && !allowed.includes('answer') ? (
        // Not `allowed.length === 0`. The server also offers the ad-hoc
        // actions — validate, test, open the editor — and those belong to the
        // block inspector, where somebody is editing. This card follows a run,
        // so what it can offer is the two that are *commands*, and saying
        // nothing when the other three came back would be an empty panel with
        // no explanation.
        <span className="text-muted-foreground">No command applies to this step right now.</span>
      ) : null}
    </div>
  );
}

/**
 * The code this step ran, not the code that is there now.
 *
 * The runtime keeps every source it has been given, named by its own digest, so
 * this reads the pin rather than the head — which is what makes reopening a
 * step of last month's execution show last month's notebook however many times
 * it has been edited since. Read-only for the same reason: editing here would
 * be editing history.
 *
 * Absent rather than a failure when the runtime is not running. This is a
 * detail beside the two commands, and a chain of source and transform blocks
 * works perfectly well without that service.
 */
function PinnedNotebook({ notebook, revision }: { notebook: string; revision: string }) {
  const [open, setOpen] = React.useState(false);
  const source = useQuery({
    queryKey: ['ml-pipeline', 'notebook', notebook, revision],
    enabled: open,
    retry: false,
    // A revision is immutable; asking twice asks the same question.
    staleTime: Infinity,
    queryFn: () => getNotebookRevision(notebook, revision),
  });

  return (
    <div className="flex flex-col gap-1">
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          className="flex items-center gap-2 text-left text-muted-foreground hover:underline"
          onClick={() => setOpen((current) => !current)}
        >
          <FileCode2 className="h-3 w-3" />
          {open ? 'Hide' : 'Show'} the code this step ran
        </button>
        <IdChip label={notebook} value={revision.slice(0, 12)} full={revision} />
      </div>
      {open && source.isPending ? (
        <span className="flex items-center gap-2 text-muted-foreground">
          <Spinner /> Reading it…
        </span>
      ) : null}
      {open && source.isError ? (
        <span className="text-muted-foreground">
          The notebook runtime is not answering, so the source this step ran cannot be read from
          here. It is still what the run executed.
        </span>
      ) : null}
      {open && source.data ? (
        <pre className="max-h-72 overflow-auto rounded-md border border-border bg-muted/30 p-2 text-[0.7rem] leading-relaxed">
          {source.data.source}
        </pre>
      ) : null}
    </div>
  );
}

/**
 * Open the notebook's live app on the rows this step read.
 *
 * The server resolves it, and without a token: the runtime it would be
 * presented to has no authentication to check one against. What exists instead is the gate on the route: aiwatcher decides
 * who may open which run's rows, reads them from its own object store and
 * stages them.
 *
 * A button rather than something that happens on open, because it *changes*
 * what the notebook's live app shows for everybody looking at it — which is
 * also why the route asks for the editor role rather than the viewer one.
 */
function OpenEditor({ executionId, stepId }: { executionId: string; stepId: string }) {
  const open = useMutation({
    mutationFn: async () =>
      answerOf(
        await openEditor({ path: { execution_id: executionId, step_id: stepId } }),
        'The editor could not be opened on this step.',
      ),
  });

  if (open.data) {
    return (
      <div className="flex flex-col gap-1">
        <a
          href={open.data.app_url}
          target="_blank"
          rel="noreferrer"
          className="flex items-center gap-1 text-primary hover:underline"
        >
          <SquareArrowOutUpRight className="h-3 w-3" /> Open the notebook on{' '}
          {open.data.rows === 0 ? 'no rows' : `${open.data.rows} rows`} →
        </a>
        {/* Said rather than hidden: marimo serves the notebook's head, so the
            widgets move against this run's rows under today's code. The code
            that ran is the panel above, read by its digest. */}
        <span className="text-muted-foreground">
          Those are the rows this attempt read. The app runs the notebook as it is now — the code
          this step ran is above.
        </span>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-1">
      <div>
        <Button variant="outline" size="sm" disabled={open.isPending} onClick={() => open.mutate()}>
          {open.isPending ? <Spinner /> : <SquareArrowOutUpRight className="mr-1 h-3 w-3" />} Open
          the editor on this step&apos;s rows
        </Button>
      </div>
      {open.isError ? (
        <Refusal error={open.error} fallback="The editor could not be opened on this step." />
      ) : null}
    </div>
  );
}
