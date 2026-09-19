import { z } from 'zod';

/**
 * Which organization's workshops, which workshop, and which lab.
 *
 * In the URL like every other selection here, so "come and look at this
 * lesson" is a link. It is deliberately the same shape as `/account/access`
 * and deliberately *not* a path prefix: `/orgs/{organization}/projects/…` is
 * what a scoped panel would look like, and this is not one — the data plane is
 * still instance-wide, and a URL that claimed otherwise would be the
 * announcement the migration plan says not to make.
 *
 * `lab` was component state while a lab was a section of this page and nothing
 * else. It stopped being one the day a lab handed out a notebook: opening it
 * runs code, working in it makes a file of your own, and both are things
 * somebody comes back to and sends to a classmate. `notebook` is that copy —
 * the participant's own file in the runtime, which is theirs and not the
 * lab's.
 */
export const searchSchema = z.object({
  organization: z.string().optional(),
  project: z.string().optional(),
  lab: z.string().optional(),
  notebook: z.string().optional(),
});
