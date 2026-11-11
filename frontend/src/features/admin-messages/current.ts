/** Known authority/lifecycle refusals retire old content; temporary read failures retain it dimmed. */
import { ApiError } from '@/api/client';

export function messageReadRefused(error: unknown): boolean {
  return error instanceof ApiError && [401, 403, 404, 410].includes(error.status);
}
