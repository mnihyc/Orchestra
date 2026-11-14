/** Read-only native judgements never create, delete or grant access to their candidate objects. */
import { queryOptions } from '@tanstack/react-query';
import { traceJson } from '@/api/trace';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import type { DuplicateDestination } from '@/api/duplicates';
import type { Problem } from '@/generated/api-types';

// Published native shapes; generated aliases arrive with the contract carrier.
export type CopySubjectKind = 'provider' | 'agent' | 'template' | 'mcp_server' | 'mount' | 'skill' | 'environment' | 'thread' | 'document' | 'document_container' | 'chat' | 'host' | 'machine';
export interface CopySubject { kind: CopySubjectKind; id: string }
export type CopyIntent = 'duplicate' | 'move';
export type CopyDestinationCause = 'source_policy' | 'export_permission' | 'destination_creation' | 'owners_only' | 'not_available' | 'delete_refused' | 'registration_only';
export type CopyDestinationChoice = 'in_org_move' | 'environment_placement' | 'mcp_machine' | 'fork_plan' | 'document_tree_preview' | 'delete_preview' | 'native_operation_scope';
export type CopyDestinationOutcome = { state: 'allowed' } | { state: 'blocked'; cause: CopyDestinationCause; problem: Problem } | { state: 'needs_choice'; choice: CopyDestinationChoice };
export interface CopyDestinationJudgement { destination: DuplicateDestination; cross_root: boolean; outcome: CopyDestinationOutcome }
export interface CopyDestinationsRequest { subject: CopySubject; intent: CopyIntent; destinations: DuplicateDestination[] }
export interface CopyDestinationsResponse { subject: CopySubject; intent: CopyIntent; source_owned: boolean; destinations: CopyDestinationJudgement[] }
const address = (destination: DuplicateDestination) => destination.kind === 'node' ? `node:${destination.id}` : 'system';

export async function judgeCopyDestinations(request: CopyDestinationsRequest, signal?: AbortSignal): Promise<CopyDestinationsResponse> {
  const before = libraryReader();
  // The native reader accepts one hundred observations per request; the chooser has no total limit.
  const batchSize = 100;
  if (request.destinations.length > batchSize) {
    let first: CopyDestinationsResponse | undefined;
    const destinations: CopyDestinationJudgement[] = [];
    for (let at = 0; at < request.destinations.length; at += batchSize) {
      signal?.throwIfAborted();
      const batch = await judgeCopyDestinations({ ...request, destinations: request.destinations.slice(at, at + batchSize) }, signal);
      if (!sameLibraryReader(before)) throw new DOMException('The requesting reader changed.', 'AbortError');
      if (first !== undefined && first.source_owned !== batch.source_owned) throw new Error('The source changed during destination checks.');
      first ??= batch;
      destinations.push(...batch.destinations);
    }
    if (first === undefined) throw new Error('No destination batch was read.');
    return { ...first, destinations };
  }
  const result = await traceJson<CopyDestinationsResponse>('/copy-destinations/judge', { method: 'POST', body: request, ...(signal === undefined ? {} : { signal }) });
  signal?.throwIfAborted();
  if (!sameLibraryReader(before)) throw new DOMException('The requesting reader changed.', 'AbortError');
  const requested = new Set(request.destinations.map(address));
  const returned = new Set(result.destinations.map((row) => address(row.destination)));
  if (result.subject.kind !== request.subject.kind || result.subject.id !== request.subject.id || result.intent !== request.intent ||
      result.destinations.length !== request.destinations.length || returned.size !== requested.size || [...returned].some((key) => !requested.has(key))) throw new Error('The destination judgement did not match its requested subjects.');
  return result;
}
export const copyDestinationsQuery = (request: CopyDestinationsRequest, reader: ReturnType<typeof libraryReader>) => queryOptions({
  queryKey: ['copy-destinations', reader, request], queryFn: ({ signal }) => judgeCopyDestinations(request, signal),
  staleTime: 0, gcTime: 0, retry: false,
});
