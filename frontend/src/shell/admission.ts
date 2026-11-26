/** AuthFrame and route admission join the same native Meta supplier. */
import { queryOptions } from '@tanstack/react-query';
import { fetchMeta, metaKey, metaQuery } from '@/api/auth';
import { pageHtmlReads } from '@/api/htmlReads';
import { queryClient } from '@/api/query';
import type { Meta } from '@/generated/api-types';
import { client } from '@/api/client';

/** Claim the public receipt before anonymous cookie discovery retires the document's private reads. */
export function primeLoginDocument(): Promise<void> | undefined {
  const reads = pageHtmlReads();
  if (reads?.hasLoginMeta !== true) return;
  const receipt = reads.take(new Request(new URL('/api/v1/meta', location.origin)));
  return receipt?.then(async (reply) => {
    if (reply === null) return;
    const policy = await reply.response.json() as Meta;
    if (client.session.getState().userId === null && !policy.may_create_org) {
      queryClient.setQueryData(metaKey, policy);
    }
  });
}

export const admissionMetaQuery = queryOptions({
  ...metaQuery,
  retryOnMount: false,
  queryFn: async ({ signal, client: cache }) => {
    const previousWrites = cache.getQueryState(metaKey)?.dataUpdateCount ?? 0;
    const signedIn = await client.restore();
    const userId = client.session.getState().userId;
    if (signedIn && userId !== null) {
      const reader = client.session.getState().readerVersion;
      // Anonymous screens keep the workbench chunk lazy. A cold signed-in
      // admission reuses the Meta already offered by the admitted bootstrap.
      const { ensureShellBootstrap, shellBootstrapKey } = await import('@/shell/bootstrap');
      // The shell can publish Meta while admission awaits the session or this import. Use the
      // answer delivered to this flight. A later explicit refresh still goes to the native supplier.
      const meta = cache.getQueryState<Meta>(metaKey);
      const current = client.session.getState();
      if (meta?.data !== undefined && !meta.isInvalidated && meta.dataUpdateCount > previousWrites
        && current.userId === userId && current.readerVersion === reader) return meta.data;
      if (cache.getQueryData(shellBootstrapKey(userId)) === undefined) {
        return (await ensureShellBootstrap(cache, userId)).meta;
      }
    }
    // Explicit later metadata repair and an anonymous entrance retain the
    // native supplier, its errors and its caller-version fence.
    return fetchMeta(signal);
  },
});
