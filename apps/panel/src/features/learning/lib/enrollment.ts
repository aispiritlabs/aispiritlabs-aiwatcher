import type { IamGrant, IamGrantWindow, IamProjectRole } from '@/api/generated';

/**
 * A workshop is a project, a participant is a grant, and enrolling is
 * redeeming an invitation. Nothing in the backend knows the word "workshop".
 *
 * That mapping is what makes this area buildable at all, and it is also what
 * decides the one hard question here: **what may the browser say about
 * somebody else's access?**
 *
 * The answer the rest of this panel gives is "nothing" — a grants list shows
 * rows exactly as issued, because filtering by the clock would hide the row
 * somebody opened it to find and would be a second copy of the policy. That
 * still holds, and this keeps it: no row is hidden, no control is enabled or
 * disabled by what is computed here.
 *
 * What a workshop page has to answer, though, is "has this lesson started for
 * them, and has it closed" — and the server answers that only about the
 * caller, through `GET .../access`. There is no route that asks it about
 * anybody else, and composing one from a roster would be exactly the second
 * implementation this repository refuses.
 *
 * So the line is drawn at **one grant at a time**. [`phaseOf`] reads the dates
 * on a single row against a clock; it is never summed across rows into a
 * person's access, because the effective role is the maximum over every live
 * grant and a lapsed workshop grant must not read as access being gone when a
 * permanent one is still there. The page says that in words beside the rows.
 *
 * The caller's own access is not read from here at all: `IamProjectAccess`
 * carries the server's verdict for the whole project and one per source, so
 * where a verdict exists it is the verdict that is drawn.
 */

/**
 * Where one grant's window sits, relative to a moment.
 *
 * The four cases are the server's, in `GrantWindow::role_at`: half-open
 * intervals, `valid_from` inclusive and both ends exclusive, and after
 * `edit_until` a still-readable grant falls back to viewer. `reading` is that
 * fallback and is reported **only** for a grant that was issued as editor or
 * admin — for a viewer nothing changed at `edit_until`, and calling their
 * ordinary state "read-only" from that instant would announce a transition
 * they never made.
 */
export type Phase = 'before' | 'open' | 'reading' | 'ended';

export function phaseOf(window: IamGrantWindow, role: IamProjectRole, now: number): Phase {
  if (now < window.valid_from) return 'before';
  if (window.read_until != null && now >= window.read_until) return 'ended';
  if (role !== 'viewer' && window.edit_until != null && now >= window.edit_until) return 'reading';
  return 'open';
}

/** What each phase is called where a reader sees it, and the tone it carries. */
export const PHASES: Record<Phase, { label: string; tone: 'neutral' | 'success' | 'warning' }> = {
  before: { label: 'not open yet', tone: 'neutral' },
  open: { label: 'open', tone: 'success' },
  reading: { label: 'read-only', tone: 'warning' },
  ended: { label: 'closed', tone: 'neutral' },
};

/**
 * The one sentence that says what the dates on this row mean next.
 *
 * Deliberately about the window rather than about the person: "opens", not
 * "they cannot get in yet". The distinction is the whole reason this file has
 * a preamble.
 */
export function windowSentence(window: IamGrantWindow, phase: Phase): string {
  const when = (seconds: number) => new Date(seconds * 1000).toLocaleString();
  switch (phase) {
    case 'before':
      return `opens ${when(window.valid_from)}`;
    case 'open':
      return window.edit_until != null
        ? `editing until ${when(window.edit_until)}`
        : window.read_until != null
          ? `until ${when(window.read_until)}`
          : 'no end declared';
    case 'reading':
      return window.read_until != null
        ? `editing ended ${when(window.edit_until as number)}, reading until ${when(window.read_until)}`
        : `editing ended ${when(window.edit_until as number)}, reading does not end`;
    case 'ended':
      return `closed ${when(window.read_until as number)}`;
  }
}

/**
 * Participants, grouped by the person or team a grant names.
 *
 * Grouped rather than listed flat because two grants to one person is the
 * ordinary case this whole design turns on — a permanent one and a workshop
 * one — and two rows with the same subject read as a mistake unless they are
 * visibly one person's two sources.
 */
export interface Participant {
  /** Stable across renders and unique per grantee: a provider, a subject, a team id. */
  key: string;
  grantee: IamGrant['grantee'];
  grants: IamGrant[];
}

export function participantsOf(grants: IamGrant[]): Participant[] {
  const byGrantee = new Map<string, Participant>();
  for (const grant of grants) {
    const key =
      grant.grantee.kind === 'user'
        ? `user\u0000${grant.grantee.value.provider}\u0000${grant.grantee.value.subject}`
        : `team\u0000${grant.grantee.value}`;
    const found = byGrantee.get(key);
    if (found) found.grants.push(grant);
    else byGrantee.set(key, { key, grantee: grant.grantee, grants: [grant] });
  }
  return [...byGrantee.values()];
}
