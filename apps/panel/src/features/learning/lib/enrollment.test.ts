import { describe, expect, it } from 'vitest';

import type { IamGrant, IamGrantWindow } from '@/api/generated';
import { participantsOf, phaseOf, windowSentence } from './enrollment';

const DAY = 24 * 60 * 60;
const MONDAY = 1_800_000_000;

const window = (parts: Partial<IamGrantWindow> = {}): IamGrantWindow => ({
  valid_from: MONDAY,
  edit_until: null,
  read_until: null,
  ...parts,
});

describe('where one grant sits in the window it was issued with', () => {
  it('opens on its first second, because the start is inclusive', () => {
    // Half-open, exactly as `GrantWindow::role_at` has it: a lesson starting at
    // nine is open at nine, and a panel that waited for 09:00:01 would be
    // telling somebody they are late while the server lets them in.
    expect(phaseOf(window(), 'editor', MONDAY - 1)).toBe('before');
    expect(phaseOf(window(), 'editor', MONDAY)).toBe('open');
  });

  it('closes on the second its reading ends, because the end is exclusive', () => {
    const friday = window({ read_until: MONDAY + 5 * DAY });
    expect(phaseOf(friday, 'editor', MONDAY + 5 * DAY - 1)).toBe('open');
    expect(phaseOf(friday, 'editor', MONDAY + 5 * DAY)).toBe('ended');
  });

  it('drops an editor to reading when editing ends, and leaves reading open with no end', () => {
    const lesson = window({ edit_until: MONDAY + DAY });
    expect(phaseOf(lesson, 'editor', MONDAY + DAY)).toBe('reading');
    expect(windowSentence(lesson, 'reading')).toContain('reading does not end');
  });

  it('does not announce a change a viewer never made', () => {
    // A viewer's grant grants reading and nothing else, so `edit_until` passing
    // changes nothing for them — `role_at` hands them Viewer before and after.
    // Calling that moment "read-only" would report a transition that did not
    // happen, on the row of the person least likely to know it did not.
    const lesson = window({ edit_until: MONDAY + DAY, read_until: MONDAY + 5 * DAY });
    expect(phaseOf(lesson, 'viewer', MONDAY + DAY)).toBe('open');
    expect(phaseOf(lesson, 'viewer', MONDAY + 5 * DAY)).toBe('ended');
  });

  it('says what happens next rather than only when it happened', () => {
    expect(windowSentence(window(), 'before')).toContain('opens');
    expect(windowSentence(window(), 'open')).toBe('no end declared');
    expect(windowSentence(window({ edit_until: MONDAY + DAY }), 'open')).toContain('editing until');
    expect(windowSentence(window({ read_until: MONDAY + DAY }), 'ended')).toContain('closed');
  });
});

describe('participants, when one person holds more than one grant', () => {
  const grant = (id: string, subject: string): IamGrant => ({
    id,
    role: 'viewer',
    scope: { organization: 'org', project: 'workshop' },
    grantee: { kind: 'user', value: { provider: 'authentik', subject } },
    window: window(),
  });

  it('keeps a permanent grant and a workshop grant on one row', () => {
    // The case the whole design turns on: a workshop grant expiring must not
    // read as access being gone. Two rows with the same subject read as a
    // duplicate; one row with two sources reads as what it is.
    const rows = participantsOf([
      grant('a', 'student'),
      grant('b', 'student'),
      grant('c', 'other'),
    ]);
    expect(rows).toHaveLength(2);
    expect(rows[0]?.grants.map((entry) => entry.id)).toEqual(['a', 'b']);
  });

  it('keeps two people apart when their subjects differ only by provider', () => {
    const elsewhere: IamGrant = {
      ...grant('d', 'student'),
      grantee: { kind: 'user', value: { provider: 'other-idp', subject: 'student' } },
    };
    expect(participantsOf([grant('a', 'student'), elsewhere])).toHaveLength(2);
  });

  it('keeps a team grant separate from a grant to a person of the same name', () => {
    const team: IamGrant = { ...grant('e', 'x'), grantee: { kind: 'team', value: 'student' } };
    expect(participantsOf([grant('a', 'student'), team])).toHaveLength(2);
  });
});
