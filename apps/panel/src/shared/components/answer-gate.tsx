import * as React from 'react';
import { useMutation } from '@tanstack/react-query';

import { provideInput } from '@/api/generated/sdk.gen';
import type { InputRequest, Role, RuntimeBinding } from '@/api/generated/types.gen';
import { needsRole, useRoleDecision } from '@/shared/lib/auth';
import { answerOf } from '@/shared/lib/result';

import { Button, Refusal } from '@/shared/components/ui/primitives';

/**
 * The question a parked step is holding, and the only way to answer it.
 *
 * One component for both places a managed run is watched — the curation
 * pipeline's run card and the Workflows view — because both compile a gate to
 * the same binding and answer it through the same route. Two copies of "which
 * answers may be pressed, and by whom" would be two ideas of what a gate is.
 *
 * **The answers are the step's own.** `decide` refuses anything the question
 * did not offer, by name, so a free-text box beside a declared list would be a
 * way to type a 409. An empty list is the other half of the same rule: the
 * answer is then whatever somebody writes.
 *
 * **Who asked is shown, because the two are different news.** An authored gate
 * is a box somebody drew: a person decided at authoring time that this needs a
 * signature, and answering it is the process working. A step that stopped in
 * the middle of its own work is the running code deciding to ask — nobody
 * planned it, and it is the case where somebody should look harder before
 * pressing approve. The questions render identically otherwise, so without a
 * line saying which, the reader cannot tell an expected sign-off from an agent
 * doing something nobody anticipated.
 */
export function AnswerGate({
  executionId,
  stepId,
  attempt,
  question,
  authored,
  onAnswered,
}: {
  executionId: string;
  stepId: string;
  /**
   * The attempt that asked, named rather than assumed. An answer typed against
   * a question a retry has since replaced is refused by name — which is the
   * point of sending it, so it must be the attempt this context was read at.
   */
  attempt: number;
  question: InputRequest;
  /**
   * Whether the plan declared this question, rather than the running code
   * choosing to ask it.
   *
   * Read from the pinned plan's binding by the caller, which is the only place
   * that knows: a `human_input` step *is* the question, and anything else that
   * is waiting parked in the middle of its own work.
   *
   * `undefined` is a third state and not a default — a context that has not
   * been read yet, or one from a build that did not send its binding. Nothing
   * is said then, because the wrong sentence here is worse than none: telling
   * somebody the code decided to ask when the plan declared it would put a
   * warning on a routine sign-off, and the other way round hides the case this
   * exists for.
   */
  authored?: boolean;
  onAnswered: () => void;
}) {
  const [answer, setAnswer] = React.useState('');
  // Whether this caller holds the role the question named. The server is the
  // check — `provide_input` refuses the rest — and this is so that finding out
  // costs a sentence rather than a round trip and a red box. `undefined` while
  // the session is still being read, and the controls stay up through it: a
  // refusal shown before anybody has answered is a refusal nobody issued.
  const mayAnswer = useRoleDecision(roleOf(question));

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
    onSuccess: onAnswered,
  });

  if (mayAnswer === false) {
    // The question, and why the answer is somebody else's. Not hidden: knowing
    // a run is stopped on a decision you may not make is the whole of what
    // somebody needs in order to go and find who can.
    return (
      <div className="flex flex-col gap-1 text-xs">
        <p>{question.prompt}</p>
        <Asker authored={authored} />
        <Deadline at={question.deadline} />
        <span className="text-muted-foreground">{needsRole(roleOf(question))}</span>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-2 text-xs">
      <p>{question.prompt}</p>
      <Asker authored={authored} />
      <Deadline at={question.deadline} />
      {question.choices?.length ? (
        // Named as a group because what is in it is the whole answer: every
        // button here is one the step declared, and there is no other way to
        // answer it.
        <div role="group" aria-label="Answers" className="flex flex-wrap gap-2">
          {question.choices.map((choice) => (
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
  );
}

/**
 * Where this question came from.
 *
 * Two sentences that read very differently on purpose. An authored gate is
 * routine — somebody drew the box, and the run stopping here is the process
 * working. A step that asked from inside its own work is not: nothing planned
 * this, and it is exactly the moment to read the question twice before
 * approving it.
 *
 * Said in words rather than as a badge, because a badge is a thing you learn to
 * stop seeing and this is the sentence that changes what somebody does next.
 */
function Asker({ authored }: { authored?: boolean }) {
  if (authored === undefined) return null;
  return (
    <span className="text-muted-foreground">
      {authored
        ? 'This gate is part of the definition.'
        : 'This step stopped in the middle of its own work to ask.'}
    </span>
  );
}

/**
 * When this question stops being answerable, when it does.
 *
 * Absent for most gates, and that absence is the honest default: a decision
 * somebody has to think about waits as long as it takes. Where there is a
 * deadline it is shown rather than counted down — the moment is the fact, and a
 * ticking clock in a browser that a reload resets is a second answer to
 * "when", free to disagree with the one the server is holding.
 */
function Deadline({ at }: { at?: string | null }) {
  if (!at) return null;
  return (
    <span className="text-muted-foreground">
      Answerable until {new Date(at).toLocaleString()}. After that the step decides for itself.
    </span>
  );
}

/**
 * The role a question named, as one this panel can check.
 *
 * The floor is the answer route's own: it requires an editor and reads nothing
 * weaker, so a request naming anything below that is still answered by an
 * editor and saying otherwise would put buttons in front of somebody who is
 * about to be refused. A stricter name is honoured as it stands.
 */
function roleOf(question: InputRequest): Role {
  return question.role === 'admin' ? 'admin' : 'editor';
}

/**
 * Whether the plan declared this question, from the binding it pinned.
 *
 * One reader, exported, because both views ask it and a second `=== 'human_input'`
 * in the other file is the drift this component exists to prevent. `undefined`
 * for a binding that is not there — see [`AnswerGate`]'s `authored`.
 */
export function askedBy(binding: RuntimeBinding | undefined): boolean | undefined {
  return binding === undefined ? undefined : binding.runtime === 'human_input';
}
