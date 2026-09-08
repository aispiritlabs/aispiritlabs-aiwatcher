import * as React from 'react';
import { Link } from '@tanstack/react-router';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { CalendarClock, Play, Save, Trash2 } from 'lucide-react';

import { clearSchedule, getSchedule, setSchedule } from '@/api/generated/sdk.gen';
import type { Cadence, LastFiring, OverlapPolicy } from '@/api/generated/types.gen';
import { rejectionDetails } from '@/lib/annotations';

import { Button, Card, Spinner } from './ui/primitives';

/**
 * When a saved pipeline runs unattended.
 *
 * **Nothing here works out when that is.** `next_run` comes from the server,
 * computed by the same `slots_between` the tick uses — a second implementation
 * in TypeScript would have its own idea of when the clocks change, and the
 * first hour it disagreed on would be one somebody planned a morning around.
 * Which hours are legal is the server's too: the form sends what was typed and
 * renders the refusal.
 *
 * It is beside **Run on the server** rather than replacing it. That button
 * starts whatever is on the canvas, once, now; this is about mornings nobody
 * is here for.
 */

/** Days as the API numbers them: Monday is 0. */
const WEEKDAYS = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'];

type Draft = {
  every: Cadence['every'];
  hour: number;
  minute: number;
  weekday: number;
  timezone: string;
  enabled: boolean;
  overlap: OverlapPolicy;
};

/**
 * The zone the person reading this is in.
 *
 * A better default than UTC and better than a list of six hundred names: an
 * hour typed here almost always means an hour where the typist is sitting. The
 * server refuses a name it does not know, so a hand-typed one is checked rather
 * than trusted.
 */
function localZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
  } catch {
    return 'UTC';
  }
}

const EMPTY: Draft = {
  every: 'daily',
  hour: 9,
  minute: 0,
  weekday: 0,
  timezone: localZone(),
  enabled: true,
  overlap: 'skip',
};

function cadenceOf(draft: Draft): Cadence {
  if (draft.every === 'hourly') return { every: 'hourly', minute: draft.minute };
  if (draft.every === 'weekly') {
    return { every: 'weekly', weekday: draft.weekday, hour: draft.hour, minute: draft.minute };
  }
  return { every: 'daily', hour: draft.hour, minute: draft.minute };
}

function draftOf(cadence: Cadence): Pick<Draft, 'every' | 'hour' | 'minute' | 'weekday'> {
  return {
    every: cadence.every,
    hour: 'hour' in cadence ? cadence.hour : 9,
    minute: cadence.minute,
    weekday: 'weekday' in cadence ? cadence.weekday : 0,
  };
}

export function ScheduleCard({ name, saved }: { name?: string; saved: boolean }) {
  const queryClient = useQueryClient();
  const [draft, setDraft] = React.useState<Draft>(EMPTY);
  const [problems, setProblems] = React.useState<string[]>([]);
  // One-shot, like the pipeline draft next door: what the server has is loaded
  // once and never over the top of somebody mid-edit.
  const loaded = React.useRef<string | undefined>(undefined);

  const current = useQuery({
    queryKey: ['schedule', name],
    enabled: Boolean(name) && saved,
    retry: false,
    queryFn: async () => {
      const response = await getSchedule({ path: { name: name ?? '' } });
      // A pipeline with no schedule is a 404 and is not a failure — it is the
      // ordinary state of most of them.
      if (!response.data) return null;
      return response.data;
    },
  });

  React.useEffect(() => {
    if (!name || loaded.current === name) return;
    if (current.isPending) return;
    loaded.current = name;
    const found = current.data?.schedule.schedule;
    setDraft(
      found
        ? {
            ...draftOf(found.cadence),
            timezone: found.timezone,
            enabled: found.enabled ?? true,
            overlap: found.overlap ?? 'skip',
          }
        : EMPTY,
    );
  }, [name, current.isPending, current.data]);

  const save = useMutation({
    mutationFn: async (runNow: boolean) => {
      const response = await setSchedule({
        path: { name: name ?? '' },
        body: {
          cadence: cadenceOf(draft),
          timezone: draft.timezone,
          enabled: draft.enabled,
          overlap: draft.overlap,
          run_now: runNow,
        },
      });
      if (!response.data) throw response.error ?? new Error('That schedule was refused.');
      return response.data;
    },
    onSuccess: () => {
      setProblems([]);
      void queryClient.invalidateQueries({ queryKey: ['schedule', name] });
    },
    onError: (error) => setProblems(rejectionDetails(error)),
  });

  const forget = useMutation({
    mutationFn: async () => clearSchedule({ path: { name: name ?? '' } }),
    onSuccess: () => {
      loaded.current = undefined as string | undefined;
      setDraft(EMPTY);
      void queryClient.invalidateQueries({ queryKey: ['schedule', name] });
    },
  });

  if (!saved || !name) {
    return (
      <Card className="p-3 text-xs text-muted-foreground">
        <span className="flex items-center gap-2">
          <CalendarClock className="h-3.5 w-3.5" /> Save this pipeline to give it a schedule. A
          schedule names the definition, so there has to be one to name.
        </span>
      </Card>
    );
  }

  const busy = save.isPending || forget.isPending;
  const existing = current.data;
  const field = 'h-8 rounded-md border border-border bg-background px-2 text-xs';

  return (
    <Card className="flex flex-col gap-3 p-3 text-xs">
      <div className="flex items-center gap-2">
        <CalendarClock className="h-3.5 w-3.5" />
        <span className="text-sm font-medium">Schedule</span>
        {existing ? (
          <Button
            variant="ghost"
            size="sm"
            className="ml-auto"
            disabled={busy}
            onClick={() => forget.mutate()}
            title="Forget this schedule. Turning it off keeps the settings instead."
          >
            <Trash2 className="mr-1 h-3 w-3" /> Forget
          </Button>
        ) : null}
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <select
          className={field}
          value={draft.every}
          onChange={(event) =>
            setDraft((previous) => ({ ...previous, every: event.target.value as Cadence['every'] }))
          }
        >
          <option value="hourly">Every hour</option>
          <option value="daily">Every day</option>
          <option value="weekly">Every week</option>
        </select>

        {draft.every === 'weekly' ? (
          <select
            className={field}
            value={draft.weekday}
            onChange={(event) =>
              setDraft((previous) => ({ ...previous, weekday: Number(event.target.value) }))
            }
          >
            {WEEKDAYS.map((day, index) => (
              <option key={day} value={index}>
                {day}
              </option>
            ))}
          </select>
        ) : null}

        <span className="text-muted-foreground">at</span>
        {draft.every === 'hourly' ? null : (
          <>
            <input
              type="number"
              min={0}
              max={23}
              className={`${field} w-16`}
              value={draft.hour}
              onChange={(event) =>
                setDraft((previous) => ({ ...previous, hour: Number(event.target.value) }))
              }
            />
            <span className="text-muted-foreground">:</span>
          </>
        )}
        <input
          type="number"
          min={0}
          max={59}
          className={`${field} w-16`}
          value={draft.minute}
          onChange={(event) =>
            setDraft((previous) => ({ ...previous, minute: Number(event.target.value) }))
          }
        />
        {draft.every === 'hourly' ? (
          <span className="text-muted-foreground">past the hour</span>
        ) : (
          <input
            className={`${field} min-w-40 flex-1`}
            value={draft.timezone}
            placeholder="Europe/Warsaw"
            onChange={(event) =>
              setDraft((previous) => ({ ...previous, timezone: event.target.value }))
            }
            // Named rather than offset, so nine stays nine after the clocks
            // change. The server refuses a zone it does not know.
            title="An IANA time zone, such as Europe/Warsaw"
          />
        )}
      </div>

      <div className="flex flex-wrap items-center gap-4">
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={draft.enabled}
            onChange={(event) =>
              setDraft((previous) => ({ ...previous, enabled: event.target.checked }))
            }
          />
          On
        </label>
        <label className="flex items-center gap-2" title="What to do if the last run is still going">
          <input
            type="checkbox"
            checked={draft.overlap === 'allow'}
            onChange={(event) =>
              setDraft((previous) => ({
                ...previous,
                overlap: event.target.checked ? 'allow' : 'skip',
              }))
            }
          />
          Start even if the last run is still going
        </label>
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" disabled={busy} onClick={() => save.mutate(false)}>
          {save.isPending ? <Spinner /> : <Save className="mr-1 h-3 w-3" />} Save
        </Button>
        <Button
          variant="outline"
          size="sm"
          disabled={busy}
          onClick={() => save.mutate(true)}
          title="Save it, and start one run now as well."
        >
          <Play className="mr-1 h-3 w-3" /> Save and run now
        </Button>
        {existing?.next_run ? (
          <span className="text-muted-foreground">
            next {new Date(existing.next_run).toLocaleString()}
          </span>
        ) : existing ? (
          <span className="text-muted-foreground">off — it starts nothing</span>
        ) : null}
      </div>

      {save.data?.started ? (
        <p className="text-muted-foreground">
          Started one run now. It is on the server; this page does not have to stay open.
        </p>
      ) : null}

      {existing?.schedule.last ? <LastFiringLine last={existing.schedule.last} /> : null}

      {problems.length > 0 ? (
        <ul className="flex flex-col gap-1 text-destructive">
          {problems.map((problem) => (
            <li key={problem}>{problem}</li>
          ))}
        </ul>
      ) : null}
    </Card>
  );
}

/**
 * What the tick last did, and never whether the run then worked.
 *
 * The scheduler's own decision is the thing nothing else records; whether the
 * run succeeded is on the event log, one click away in the waterfall. Keeping
 * the two apart is what stops this being a second answer free to disagree with
 * the fold (ADR_0026).
 *
 * It exists because the alternative was a log line: a schedule refused every
 * morning for a week looked from here exactly like one that had been working.
 */
function LastFiringLine({ last }: { last: LastFiring }) {
  const when = new Date(last.slot).toLocaleString();
  if (last.outcome === 'refused') {
    return (
      <p className="text-destructive">
        {when}: could not start — {last.detail ?? 'no reason recorded'}
      </p>
    );
  }
  if (last.outcome === 'skipped') {
    return (
      <p className="text-warning">
        {when}: skipped, the previous run had not finished. Untick the box above to start anyway.
      </p>
    );
  }
  return (
    <p className="text-muted-foreground">
      {when}: started{' '}
      {last.execution_id ? (
        <Link
          to="/workflows"
          search={{ execution: last.execution_id }}
          className="text-primary hover:underline"
        >
          that run →
        </Link>
      ) : (
        'a run'
      )}
    </p>
  );
}
