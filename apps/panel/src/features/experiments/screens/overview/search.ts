import { windowSearchSchema } from '@/shared/components/time-range';
import { z } from 'zod';

export const searchSchema = z.object({
  ...windowSearchSchema,
  dataset: z.string().optional(),
  variant: z.string().optional(),
  stage: z.enum(['training', 'evaluation', 'inference']).optional(),
  engine: z.string().optional(),
  engineFind: z.string().optional(),
});
