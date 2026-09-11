import { z } from 'zod';

/**
 * The gate: nothing reaches a corpus until somebody has read it.
 *
 * Three things about this screen are deliberate and easy to get wrong.
 *
 * **The list decrypts nothing.** Everything on the left and everything in a
 * turn's header — role, ordering, policy, findings, expiry — comes from the
 * plaintext head. Content is fetched one turn at a time, by an explicit click,
 * and only for a caller with the `admin` role. A queue that pre-loaded every
 * message would make the encryption ornamental.
 *
 * **A rejection needs a reason.** The server refuses one without a note or a
 * finding, and this renders that refusal rather than reimplementing it — the
 * same rule the annotation canvas keeps about the registry's 422.
 *
 * **The preference is not the review state.** Approving says the content may be
 * used; the preference says which of two answers was better. They are separate
 * because a turn rejected for holding somebody's address must never become the
 * rejected half of a preference pair.
 */

export const searchSchema = z.object({
  conversation: z.string().optional(),
  review: z.enum(['pending', 'approved', 'rejected']).optional(),
  finding: z.enum(['pii', 'secret', 'unsafe', 'duplicate', 'policy']).optional(),
});
