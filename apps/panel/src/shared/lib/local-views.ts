import { z } from 'zod';
import { useAuthConfig, useSession } from '@/shared/lib/auth';
import { client } from '@/api/generated/client.gen';

export const viewSchema = z.object({
  schema_version: z.literal(1),
  name: z.string().trim().min(1).max(80),
  screen: z.enum(['training', 'evaluation']),
  filters: z.object({
    model: z.string().optional(),
    dataset: z.string().optional(),
    status: z.string().optional(),
    suite: z.string().optional(),
    q: z.string().optional(),
    window: z.number().int().nonnegative().optional(),
  }),
  run_ids: z.array(z.string()).max(5),
  report_id: z.string().optional(),
  baseline_id: z.string().optional(),
  metrics: z.array(z.string()),
  normalise: z.boolean().optional(),
});
export type LocalView = z.infer<typeof viewSchema>;
export type ViewScreen = LocalView['screen'];

export function storageScope(instance: string, identity: string) {
  return `aiwatcher.local.v1:${JSON.stringify([instance, identity])}`;
}

/** No identity fallback while authentication is loading, failed, or signed out. */
export function useLocalScope() {
  const config = useAuthConfig();
  const session = useSession(config.data?.enabled === true);
  if (!config.data || config.isError || (config.data.enabled && (!session.data || session.isError)))
    return undefined;
  const identity = config.data.enabled
    ? JSON.stringify([
        config.data.mode,
        config.data.issuer ?? config.data.provider,
        session.data!.subject,
      ])
    : 'auth-disabled';
  const instance = new URL(client.getConfig().baseUrl || '/', window.location.origin).href;
  return storageScope(instance, identity);
}

export function viewFromSearch(
  name: string,
  screen: ViewScreen,
  search: Record<string, unknown>,
): LocalView {
  return viewSchema.parse({
    schema_version: 1,
    name,
    screen,
    filters: Object.fromEntries(
      (screen === 'training'
        ? ['model', 'dataset', 'status']
        : ['suite', 'dataset', 'status', 'q', 'window']
      ).map((key) => [key, search[key]]),
    ),
    run_ids: screen === 'training' ? (search.runs ?? (search.run ? [search.run] : [])) : [],
    report_id: screen === 'evaluation' ? search.report : undefined,
    baseline_id: screen === 'evaluation' ? search.baseline : undefined,
    metrics: typeof search.metrics === 'string' ? search.metrics.split(',').filter(Boolean) : [],
    normalise: screen === 'training' ? search.normalise : undefined,
  });
}

export function searchFromView(view: LocalView): Record<string, unknown> {
  return {
    ...view.filters,
    ...(view.screen === 'training'
      ? { runs: view.run_ids, normalise: view.normalise }
      : { report: view.report_id, baseline: view.baseline_id }),
    metrics: view.metrics.join(',') || undefined,
  };
}

export function readViews(raw: string | null): LocalView[] {
  if (!raw) return [];
  const parsed: unknown = JSON.parse(raw);
  return z.array(viewSchema).max(30).parse(parsed);
}
