import {
  useMutation,
  useQuery,
  useQueryClient,
  type UseMutationResult,
  type UseQueryResult,
} from '@tanstack/react-query';

import {
  projectGetLab,
  projectGetLabMeasurement,
  projectListLabs,
  projectListResults,
  projectPublishLab,
} from '@/api/generated';
import type {
  DurableEvaluation,
  LabDetail,
  LabMeasurementView,
  LabPublished,
  LabPublishRequest,
  LabSummary,
} from '@/api/generated';
import { answerOf } from '@/shared/lib/result';

/**
 * A workshop's labs, over the project routes and nothing else.
 *
 * **A lab holds a brief and pins a measurement; it holds no marks.** The
 * scorecard, the cohort, the run that scores a submission and the result it
 * publishes all belonged to `evaluation` before Learning existed, and the join
 * between them is a `context_id` — the content address of the cases, the card
 * and what each metric means together. Every result measured on one lab's
 * pins shares it, so "everybody's marks for lab 3" is a filter on a list that
 * was already there ([`useLabResults`]).
 *
 * Which is why nothing here computes that id. `GET .../labs/{name}/measurement`
 * answers it, the same way `POST /evaluation-approvals/address` answers an
 * approval's: a digest over a canonicalised document is exactly the thing a
 * second implementation in TypeScript would get subtly wrong, and the day the
 * two disagreed a lab would quietly show somebody else's marks.
 *
 * Every read is scoped — `/orgs/{organization}/projects/{project}/…` — because
 * who may see a lab is the grant on the workshop's project. There is no second
 * notion of access here and none to invent: `ProjectAccess` already answers it,
 * per request, about the caller.
 */

const labsKey = (organization: string, project: string) =>
  ['learning', 'labs', organization, project] as const;
const labKey = (organization: string, project: string, name: string) =>
  ['learning', 'lab', organization, project, name] as const;
const measurementKey = (organization: string, project: string, name: string) =>
  ['learning', 'lab-measurement', organization, project, name] as const;
const resultsKey = (organization: string, project: string, contextId: string) =>
  ['learning', 'lab-results', organization, project, contextId] as const;

/**
 * A grant revoked in another tab must not leave a workshop's material on
 * screen, so none of these is served from a stale cache. It is the same rule
 * `shared/lib/iam.ts` states for an access answer, one layer out: the server
 * takes the decision per request and these reads are that request.
 */
const FRESH = { retry: false, staleTime: 0, gcTime: 0 } as const;

function ready(...values: (string | undefined)[]): boolean {
  return values.every((value) => Boolean(value));
}

/** Every lab in this workshop, by position and then by name. */
export function useLabs(
  organization: string | undefined,
  project: string | undefined,
): UseQueryResult<LabSummary[]> {
  return useQuery({
    ...FRESH,
    queryKey: labsKey(organization ?? '', project ?? ''),
    enabled: ready(organization, project),
    queryFn: async () =>
      answerOf(
        await projectListLabs({
          path: { organization: organization as string, project: project as string },
        }),
        'this workshop did not answer its labs',
      ).labs,
  });
}

/** One lab: its versions, and the brief the `published` label points at. */
export function useLab(
  organization: string | undefined,
  project: string | undefined,
  name: string | undefined,
): UseQueryResult<LabDetail> {
  return useQuery({
    ...FRESH,
    queryKey: labKey(organization ?? '', project ?? '', name ?? ''),
    enabled: ready(organization, project, name),
    queryFn: async () =>
      answerOf(
        await projectGetLab({
          path: {
            organization: organization as string,
            project: project as string,
            name: name as string,
          },
        }),
        'this lab could not be read',
      ),
  });
}

/**
 * What this lab measures, or the server's sentence saying why it cannot say.
 *
 * `unavailable` is an answer rather than a failure — a lab being written pins
 * no tests yet — so it is rendered as prose and never as an error.
 */
export function useLabMeasurement(
  organization: string | undefined,
  project: string | undefined,
  name: string | undefined,
): UseQueryResult<LabMeasurementView> {
  return useQuery({
    ...FRESH,
    queryKey: measurementKey(organization ?? '', project ?? '', name ?? ''),
    enabled: ready(organization, project, name),
    queryFn: async () =>
      answerOf(
        await projectGetLabMeasurement({
          path: {
            organization: organization as string,
            project: project as string,
            name: name as string,
          },
        }),
        'this lab did not answer what it measures',
      ),
  });
}

/**
 * The marks: every result published on this lab's cases by this lab's card.
 *
 * One page, newest first, and deliberately not an infinite list: a workshop's
 * labs have as many results as it has participants times their attempts, and
 * the page that would need a virtual list is the evaluation area's own — which
 * is where a reader goes for one result in full.
 */
export function useLabResults(
  organization: string | undefined,
  project: string | undefined,
  contextId: string | undefined,
): UseQueryResult<DurableEvaluation[]> {
  return useQuery({
    ...FRESH,
    queryKey: resultsKey(organization ?? '', project ?? '', contextId ?? ''),
    enabled: ready(organization, project, contextId),
    queryFn: async () =>
      answerOf(
        await projectListResults({
          path: { organization: organization as string, project: project as string },
          query: { context_id: contextId as string },
        }),
        'this workshop did not answer the results measured on this lab',
      ).evaluations,
  });
}

/**
 * `X-AIWatcher-IAM: 1` is not a simple header, so a cross-origin form carrying
 * only a session cookie cannot set it — which is the whole reason a scoped
 * write requires it. It rides per call rather than on the client, so nothing
 * else this area does silently becomes a mutation.
 */
const MUTATION = { headers: { 'X-AIWatcher-IAM': '1' } } as const;

/**
 * Publish a version of a lab, and optionally move a label onto it.
 *
 * Publishing and *setting* are separate decisions here for the same reason
 * they are in the prompt registry: an instructor writing next week's lab while
 * the class is on this one publishes a version with no label, and nothing they
 * are reading moves. The form asks that question in those words.
 *
 * The server takes the grant decision twice — once when the request arrives and
 * again after the body does — so a long brief cannot be uploaded through a
 * window that closed while it was in flight. Nothing here needs to know that
 * beyond not caching the answer.
 */
export function usePublishLab(
  organization: string | undefined,
  project: string | undefined,
): UseMutationResult<LabPublished, Error, LabPublishRequest> {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (body: LabPublishRequest) =>
      answerOf(
        await projectPublishLab({
          path: { organization: organization as string, project: project as string },
          body,
          ...MUTATION,
        }),
        'this lab was not published',
      ),
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: ['learning'] }),
  });
}
