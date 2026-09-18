import { Card, CardContent, CardHeader, CardTitle } from '@/shared/components/ui/primitives';

const SLOTS = 9;

/**
 * The nine lab slots, and the one thing about them that is real.
 *
 * A lab needs an instruction to read, a task to do it in, tests that say
 * whether it was done, an evaluation and a mark. Exactly one of those exists:
 * the project the work would happen in, which is the workshop itself and which
 * a participant's grant already reaches. Nothing in this instance answers the
 * other four — there is no route for a lesson's text, none for a lab's tests,
 * none for a participant's progress.
 *
 * So the slots are drawn and left empty. Filling them with a plausible brief
 * and a progress bar at 40% is the failure mode `AreaPlaceholder` exists to
 * prevent: it reads as working software, and the first bug report against a
 * number that was never real costs a day.
 */
export function Labs() {
  return (
    <Card>
      <CardHeader>
        <CardTitle>Labs</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <p className="text-xs text-muted-foreground">
          Nine slots, the shape a workshop&rsquo;s labs take. The work happens in this
          workshop&rsquo;s project, which every participant&rsquo;s grant already reaches. The rest
          — the brief, the tests, the evaluation and the mark — has no contract in this instance
          yet, and none of it is filled in below because a plausible fake would read as working
          software.
        </p>
        <ol className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
          {Array.from({ length: SLOTS }, (_, index) => (
            <li key={index} className="rounded-md border border-dashed border-border p-3">
              <p className="text-sm font-medium">Lab {index + 1}</p>
              <dl className="mt-2 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs text-muted-foreground">
                <dt>Brief</dt>
                <dd>no contract</dd>
                <dt>Tests</dt>
                <dd>no contract</dd>
                <dt>Evaluation</dt>
                <dd>no contract</dd>
                <dt>Result</dt>
                <dd>no contract</dd>
              </dl>
            </li>
          ))}
        </ol>
      </CardContent>
    </Card>
  );
}
