/** AuthFrame and route admission join the same native Meta supplier. */
import { queryOptions } from '@tanstack/react-query';
import { fetchMeta, metaQuery } from '@/api/auth';
import { client } from '@/api/client';

export const admissionMetaQuery = queryOptions({
  ...metaQuery,
  retryOnMount: false,
  queryFn: async ({ signal, client: cache }) => {
    const signedIn = await client.restore();
    const userId = client.session.getState().userId;
    if (signedIn && userId !== null) {
      // Anonymous screens keep the workbench chunk lazy. A cold signed-in
      // admission reuses the Meta already offered by the admitted bootstrap.
      const { ensureShellBootstrap, shellBootstrapKey } = await import('@/shell/bootstrap');
      if (cache.getQueryData(shellBootstrapKey(userId)) === undefined) {
        return (await ensureShellBootstrap(cache, userId)).meta;
      }
    }
    // Explicit later metadata repair and an anonymous entrance retain the
    // native supplier, its errors and its caller-version fence.
    return fetchMeta(signal);
  },
});
