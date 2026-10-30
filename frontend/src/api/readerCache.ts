/** Reader-owned query data is retired when the signed-in person changes. */
import type { QueryClient } from '@tanstack/react-query';
import { metaKey } from '@/api/auth';
import type { ApiClient } from '@/api/client';
import type { Meta } from '@/generated/api-types';

/** A direct sign-in can replace a person without emitting sign-out; renewals for the same person keep their cache. */
export function installReaderCacheBoundary(client: ApiClient, cache: QueryClient): () => void {
  let previous = client.session.getState().userId;
  return client.onSession((event) => {
    const next = event.type === 'signed_out' ? null : event.result.user.id;
    if (event.type === 'signed_out' || event.type === 'signed_in' && previous !== null && previous !== next) {
      const meta = cache.getQueryData<Meta>(metaKey);
      cache.clear();
      // The system's public facts remain; the old person's ability to make an org does not.
      if (meta !== undefined) cache.setQueryData(metaKey, { ...meta, may_create_org: false });
    }
    previous = next;
  });
}
