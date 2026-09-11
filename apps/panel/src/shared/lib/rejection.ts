/**
 * The problems a refused save carries.
 *
 * The API answers 422 with one line per problem rather than 400 with the
 * first, because a drawing can be wrong in nine ways at once and fixing them
 * one round trip at a time is how somebody stops using the tool.
 */
export function rejectionDetails(error: unknown): string[] {
  const body = error as { code?: string; message?: string; details?: string[] } | null | undefined;
  if (!body) return [];
  if (body.details?.length) return body.details;
  return body.message ? [body.message] : [];
}
