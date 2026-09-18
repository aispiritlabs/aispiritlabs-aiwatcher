import { useMutation, useQuery, useQueryClient, type UseQueryResult } from '@tanstack/react-query';

import {
  access as readAccess,
  apply as applyCommand,
  audit as readAudit,
  createOrganization as postOrganization,
  invitations as readInvitations,
  invite as postInvitation,
  organizations as readOrganizations,
  projectGrants as readProjectGrants,
  projects as readProjects,
  redeem as postRedemption,
  revokeInvitation as deleteInvitation,
  roster as readRoster,
} from '@/api/generated';
import type {
  AuditEntry,
  IamChange,
  IamCommand,
  IamGrant,
  IamGrantWindow,
  IamInvitation,
  IamIssuedInvitation,
  IamOrganization,
  IamProjectAccess,
  IamProjectRole,
  IamRedeemed,
  IamRoster,
} from '@/api/generated';
import { confirmDone } from '@/shared/lib/result';
import { answerOf } from '@/shared/lib/result';

/**
 * The control plane, as the eight calls it has.
 *
 * Two of them answer about the **caller** — `projects` and `access`, which is
 * the right shape for a permission check — and two about the **organization**:
 * `roster`, for an administrator who may grant on a project no grant of theirs
 * reaches, and `project_grants`, for whoever may issue one there. Keeping them
 * apart is the point: a role in a roster is what somebody was given, and only
 * an access answer says what anybody may do now.
 *
 * **An access answer is a decision, not a capability.** `evaluated_at` says
 * when it was taken; the README says every operation needs a fresh one. So
 * none of these reads is cached: a grant revoked in another tab must not go on
 * being true here because react-query still had it.
 *
 * The audit stays what it is — a history of mutations. Now that the roster and
 * a project's grants are readable, nothing here has to reconstruct state from
 * it, which was the one thing a browser must never do with a policy.
 */

const ORGANIZATIONS = ['iam', 'organizations'] as const;
const projectsKey = (organization: string) => ['iam', 'projects', organization] as const;
const accessKey = (organization: string, project: string) =>
  ['iam', 'access', organization, project] as const;
const auditKey = (organization: string) => ['iam', 'audit', organization] as const;
const rosterKey = (organization: string) => ['iam', 'roster', organization] as const;
const grantsKey = (organization: string, project: string) =>
  ['iam', 'grants', organization, project] as const;
const invitationsKey = (organization: string) => ['iam', 'invitations', organization] as const;

/**
 * Required on every mutation, and harmless on a read.
 *
 * `X-AIWatcher-IAM` is not a simple header, so a cross-origin form carrying a
 * session cookie cannot set it — which is the whole reason it is required. It
 * rides per call rather than on the client, so nothing else the panel does
 * silently becomes an IAM mutation.
 */
const MUTATION = { headers: { 'X-AIWatcher-IAM': '1' } } as const;

export function useOrganizations(enabled: boolean): UseQueryResult<IamOrganization[]> {
  return useQuery({
    queryKey: ORGANIZATIONS,
    enabled,
    queryFn: async () =>
      answerOf(await readOrganizations(), 'the instance did not answer /iam/organizations'),
    retry: false,
  });
}

/** Only the projects this caller holds a live grant on — membership grants none. */
export function useProjects(organization: string | undefined): UseQueryResult<IamProjectAccess[]> {
  return useQuery({
    queryKey: projectsKey(organization ?? ''),
    enabled: Boolean(organization),
    queryFn: async () =>
      answerOf(
        await readProjects({ path: { organization: organization as string } }),
        'the instance did not answer this organization',
      ),
    retry: false,
    staleTime: 0,
    gcTime: 0,
  });
}

/** One fresh decision, with every source that contributed to it. */
export function useProjectAccess(
  organization: string | undefined,
  project: string | undefined,
): UseQueryResult<IamProjectAccess> {
  return useQuery({
    queryKey: accessKey(organization ?? '', project ?? ''),
    enabled: Boolean(organization && project),
    queryFn: async () =>
      answerOf(
        await readAccess({
          path: { organization: organization as string, project: project as string },
        }),
        'the instance did not answer this project',
      ),
    retry: false,
    staleTime: 0,
    gcTime: 0,
  });
}

/**
 * Who is in the organization: members, teams and every project in it.
 *
 * Owner and admin only — a 403 is the server's answer about the reader, and
 * the page reads it as "you do not administer this" rather than as a failure.
 */
export function useRoster(organization: string | undefined): UseQueryResult<IamRoster> {
  return useQuery({
    queryKey: rosterKey(organization ?? ''),
    enabled: Boolean(organization),
    queryFn: async () =>
      answerOf(
        await readRoster({ path: { organization: organization as string } }),
        'the instance did not answer the roster',
      ),
    retry: false,
    staleTime: 0,
    gcTime: 0,
  });
}

/**
 * Every grant on one project, as issued.
 *
 * Nothing is filtered by the clock here and nothing should be: a grant that has
 * lapsed is exactly what somebody opens this list to find. What each one does
 * *now* is an access answer, taken per person.
 */
export function useProjectGrants(
  organization: string | undefined,
  project: string | undefined,
): UseQueryResult<IamGrant[]> {
  return useQuery({
    queryKey: grantsKey(organization ?? '', project ?? ''),
    enabled: Boolean(organization && project),
    queryFn: async () =>
      answerOf(
        await readProjectGrants({
          path: { organization: organization as string, project: project as string },
        }),
        'the instance did not answer this project\u2019s grants',
      ),
    retry: false,
    staleTime: 0,
    gcTime: 0,
  });
}

/**
 * Successful mutations, oldest first as the server pages them.
 *
 * Owner and admin only — a 403 here is an answer about the reader, not a
 * failure of the page.
 */
export function useAudit(organization: string | undefined): UseQueryResult<AuditEntry[]> {
  return useQuery({
    queryKey: auditKey(organization ?? ''),
    enabled: Boolean(organization),
    queryFn: async () =>
      answerOf(
        await readAudit({ path: { organization: organization as string }, query: { limit: 100 } }),
        'the instance did not answer the audit',
      ),
    retry: false,
    staleTime: 0,
    gcTime: 0,
  });
}

export function useCreateOrganization() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: async (name: string) =>
      answerOf(
        await postOrganization({ body: { name }, ...MUTATION }),
        'the organization was not created',
      ),
    onSuccess: () => void client.invalidateQueries({ queryKey: ORGANIZATIONS }),
  });
}

/**
 * One command, and then everything this caller can see about that organization
 * read again.
 *
 * Every command changes who may see what, and the answer to that is the
 * server's — so a grant issued here does not update a cache, it invalidates
 * one.
 */
export function useCommand(organization: string | undefined) {
  const client = useQueryClient();
  return useMutation<IamChange, unknown, IamCommand>({
    mutationFn: async (command: IamCommand) =>
      answerOf(
        await applyCommand({
          path: { organization: organization as string },
          body: command,
          ...MUTATION,
        }),
        'the command was not applied',
      ),
    onSuccess: () => {
      if (!organization) return;
      void client.invalidateQueries({ queryKey: projectsKey(organization) });
      void client.invalidateQueries({ queryKey: auditKey(organization) });
      void client.invalidateQueries({ queryKey: rosterKey(organization) });
      void client.invalidateQueries({ queryKey: ['iam', 'access', organization] });
      void client.invalidateQueries({ queryKey: ['iam', 'grants', organization] });
      void client.invalidateQueries({ queryKey: invitationsKey(organization) });
    },
  });
}

/**
 * The offers for projects this caller may administer, newest first.
 *
 * None of them carries its token: the plaintext existed once, in the response
 * that created it, and nothing can show it again. What is left is a record of
 * what was offered and whether somebody took it.
 */
export function useInvitations(organization: string | undefined): UseQueryResult<IamInvitation[]> {
  return useQuery({
    queryKey: invitationsKey(organization ?? ''),
    enabled: Boolean(organization),
    queryFn: async () =>
      answerOf(
        await readInvitations({ path: { organization: organization as string } }),
        'the instance did not answer the invitations',
      ),
    retry: false,
    staleTime: 0,
    gcTime: 0,
  });
}

/** Offer a grant to somebody who may never have signed in here. */
export function useInvite(organization: string | undefined, project: string | undefined) {
  const client = useQueryClient();
  return useMutation<
    IamIssuedInvitation,
    unknown,
    { role: IamProjectRole; window: IamGrantWindow; expires_at: number; label?: string }
  >({
    mutationFn: async (body) =>
      answerOf(
        await postInvitation({
          path: { organization: organization as string, project: project as string },
          body,
          ...MUTATION,
        }),
        'the invitation was not created',
      ),
    onSuccess: () => {
      if (organization) void client.invalidateQueries({ queryKey: invitationsKey(organization) });
    },
  });
}

export function useRevokeInvitation(organization: string | undefined) {
  const client = useQueryClient();
  return useMutation<void, unknown, string>({
    mutationFn: async (invitation) =>
      confirmDone(
        await deleteInvitation({
          path: { organization: organization as string, invitation },
          ...MUTATION,
        }),
        'the invitation was not withdrawn',
      ),
    onSuccess: () => {
      if (organization) void client.invalidateQueries({ queryKey: invitationsKey(organization) });
    },
  });
}

/**
 * Turn a token into membership and a grant.
 *
 * The one call here that names no organization: whoever holds a token does not
 * know which one it belongs to, and does not have to. Everything this caller
 * can see changes afterwards, so the whole IAM cache goes.
 */
export function useRedeem() {
  const client = useQueryClient();
  return useMutation<IamRedeemed, unknown, string>({
    mutationFn: async (token) =>
      answerOf(
        await postRedemption({ body: { token }, ...MUTATION }),
        'the token was not accepted',
      ),
    onSuccess: () => void client.invalidateQueries({ queryKey: ['iam'] }),
  });
}

/** Unix seconds from a `datetime-local` value, and `null` for an empty field. */
export function unixFrom(value: string): number | null {
  if (!value) return null;
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? null : Math.floor(parsed / 1000);
}

/** What a `datetime-local` input wants, in the reader's own zone. */
export function localInputValue(when: Date): string {
  const shifted = new Date(when.getTime() - when.getTimezoneOffset() * 60_000);
  return shifted.toISOString().slice(0, 16);
}

/** A window's edge, or the sentence that says there is not one. */
export function edgeOf(seconds: number | null | undefined, absent: string): string {
  if (seconds === null || seconds === undefined) return absent;
  return new Date(seconds * 1000).toLocaleString();
}

/**
 * Enough of an opaque id to recognise, with the whole of it a click away.
 *
 * A provider's subject is a 64-character hash and a grant id is a UUID; drawn
 * in full they are the widest thing on a phone, and the page scrolls sideways
 * for a string nobody reads character by character.
 */
export function short(value: string, keep = 12): string {
  return value.length > keep ? `${value.slice(0, keep)}…` : value;
}
