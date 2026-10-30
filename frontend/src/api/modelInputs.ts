/** What Orch supplied to a model for a node, exactly as it was sent: the newest first, a page at a time. */
import { infiniteQueryOptions } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import type { ModelInput, ModelInputPage, ModelTextBlock } from '@/generated/api-types';

/** Read the full immutable block through the same node's current view permission. */
export async function modelInputText(record: ModelInput, block: ModelTextBlock, signal?: AbortSignal): Promise<string> {
  if (block.text.blob === null) return block.text.inline;
  const text = await ok(api.GET('/nodes/{id}/model-inputs/{model_input_id}/blobs/{hash}', { params: { path: { id: record.node_id, model_input_id: record.id, hash: block.text.blob } }, parseAs: 'text', ...(signal === undefined ? {} : { signal }) }));
  if (typeof text !== 'string') throw new Error('The model-input text could not be read');
  return text;
}

/** A page holds what a reader takes in at once: a session's record carries the standing instructions whole. */
const GIVEN_PAGE = 20;

/** The records of a node, narrowed to one turn where `turn` names it. */
export const modelInputsQuery = (node: string, turn: string | null, record: string | null = null) => infiniteQueryOptions({
  queryKey: ['model-inputs', node, turn, record] as const,
  initialPageParam: null as number | null,
  queryFn: ({ signal, pageParam }): Promise<ModelInputPage> => ok(api.GET('/nodes/{id}/model-inputs', {
    params: {
      path: { id: node },
      query: { limit: GIVEN_PAGE, ...(turn === null ? {} : { turn_id: turn }), ...(record === null ? {} : { id: record }), ...(pageParam === null ? {} : { before: pageParam }) },
    },
    signal,
  })),
  // The cursor is the sequence the next page lies before.
  getNextPageParam: (last) => (last.next_cursor === null ? undefined : Number(last.next_cursor)),
  retry: false,
});
