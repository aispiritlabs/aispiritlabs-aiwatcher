import * as React from 'react';
import { useMutation } from '@tanstack/react-query';

import { provideInput } from '@/api/generated/sdk.gen';
import type { InputRequest, Role } from '@/api/generated/types.gen';
import { needsRole, useRoleDecision } from '@/lib/auth';
import { answerOf } from '@/lib/result';

import { Button, Refusal } from './ui/primitives';

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
 */
export function AnswerGate({
  executionId,
  stepId,
  attempt,
  question,
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
        <span className="text-muted-foreground">{needsRole(roleOf(question))}</span>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-2 text-xs">
      <p>{question.prompt}</p>
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
