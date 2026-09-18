import { z } from 'zod';

/**
 * Nothing.
 *
 * This is the one page here with no selection to keep, and the empty schema is
 * the statement rather than an omission: an instance's configuration is one
 * answer with no window, no filter and no object to choose, so there is
 * nothing a link could carry that the address alone does not.
 *
 * It exists because `commands.test.ts` holds every page in `navigation.ts`
 * against a schema, and a page with none would be a page whose command nobody
 * checks the day it gains a parameter.
 */
export const searchSchema = z.object({});
