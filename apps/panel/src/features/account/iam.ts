import { useMutation, useQuery, useQueryClient, type UseQueryResult } from '@tanstack/react-query';

import {
  access as readAccess,
  apply as applyCommand,
  audit as readAudit,
  createOrganization as postOrganization,
  organizations as readOrganizations,
  projects as readProjects,
} from '@/api/generated';
import type {
  AuditEntry,
  IamChange,
  IamCommand,
  IamOrganization,
  IamProjectAccess,
} from '@/api/generated';
import { answerOf } from '@/shared/lib/result';

/**
 * The control plane, as the six calls it actually has.
 *
 * Two things about this API shape decide how the Access view is built, and
 * both are worth knowing before reading it.
 *
 * **Nothing here lists an organization's members, its teams, or a project's
 * grants.** `projects` and `access` answer about *the caller*, which is the
 * right shape for a permission check and the wrong one for an administrator's
 * roster. So the history below is the audit — what the server recorded itself
 * doing — and it is labelled as history rather than drawn as state. Folding
 * those entries into a roster in the browser would be a second implementation
 * of the policy that already exists in Rust, and it would be wrong the first
 * time a grant expired.
 *
 * **An access answer is a decision, not a capability.** `evaluated_at` says
 * when it was taken; the README says every operation needs a fresh one. So
 * these two reads are never cached: a grant revoked in another tab must not go
 * on being true here because react-query still had it.
 */

const ORGANIZATIONS = ['iam', 'organizations'] as const;
const projectsKey = (organization: string) => ['iam', 'projects', organization] as const;
const accessKey = (organization: string, project: string) =>
  ['iam', 'access', organization, project] as const;
const auditKey = (organization: string) => ['iam', 'audit', organization] as const;

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
      void client.invalidateQueries({ queryKey: ['iam', 'access', organization] });
    },
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
