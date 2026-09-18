import { z } from 'zod';

/**
 * Which organization's workshops, and which workshop.
 *
 * In the URL like every other selection here, so "come and look at this
 * lesson" is a link. It is deliberately the same shape as `/account/access`
 * and deliberately *not* a path prefix: `/orgs/{organization}/projects/…` is
 * what a scoped panel would look like, and this is not one — the data plane is
 * still instance-wide, and a URL that claimed otherwise would be the
 * announcement the migration plan says not to make.
 */
export const searchSchema = z.object({
  organization: z.string().optional(),
  project: z.string().optional(),
});
