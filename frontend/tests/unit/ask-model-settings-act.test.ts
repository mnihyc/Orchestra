// @vitest-environment node
/** Saving the policy cannot itself describe a person's newly usable default; only shown native model reads are repaired. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyChanges, findAct } from '@/api/acts';
import { askModels } from './ask-fixtures';

for (const path of ['/nodes/root/settings/ask.models', '/admin/settings/ask.models']) {
  it.each(['PUT', 'DELETE'])(`retakes the shown Ask model projection after %s ${path}`, async (method) => {
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const key = ['ask-models', 'person', 1, 'root'], unseen = ['ask-models', 'person', 1, 'unopened'];
    const before = askModels('root', { models: [], default_model: null, blocked_reason: 'empty' }), after = askModels('root');
    cache.setQueryData(key, before); cache.setQueryData(unseen, before);
    const read = vi.fn(async () => after), stop = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity }).subscribe(() => undefined);
    try {
      const act = findAct(method, path, { answer: { key: 'ask.models', value: [after.default_model] } });
      expect(act).not.toBeNull(); await applyChanges(act?.changes ?? [], cache);
      expect(read).toHaveBeenCalledOnce(); expect(cache.getQueryData(key)).toEqual(after);
      expect(cache.getQueryData(unseen)).toEqual(before); expect(cache.getQueryState(unseen)?.isInvalidated).toBe(false);
    } finally { stop(); cache.clear(); }
  });
}
