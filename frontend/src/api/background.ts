/** Live native command pages and exact reviewed End jobs, with no roster persisted in the browser. */
import { api, ok, repeatable } from '@/api/client';
import type { BackgroundEndRequest, Job, ThreadBackground } from '@/generated/api-types';

/** Reads a fresh roster, or the next page of the same caller-bound review. */
export function fetchBackground(id: string, cursor?: string, signal?: AbortSignal): Promise<ThreadBackground> {
  return ok(api.GET('/threads/{id}/background', { params: { path: { id }, query: { ...(cursor === undefined ? {} : { cursor }) } }, ...(signal === undefined ? {} : { signal }) }));
}

/** Accepts an exact captured set; replay returns this job's outcomes, never a live roster. */
export function endBackground(id: string, request: BackgroundEndRequest, key: string): Promise<Job> {
  return ok(api.POST('/threads/{id}/background/end', { params: { path: { id } }, body: request, ...repeatable(key) }));
}
