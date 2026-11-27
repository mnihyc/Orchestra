/** The committed public-address projection and its prepared intrinsic write receipt. */
import { queryOptions } from '@tanstack/react-query';
import { accepted, api, ok } from '@/api/client';
import type { PublicAddressSnapshot, PublicAddressUpdateBody, PublicAddressUpdated } from '@/generated/api-types';
export const publicAddressKey = ['admin', 'public-address'] as const;
export const publicAddressQuery = queryOptions({ queryKey: publicAddressKey,
  queryFn: ({ signal }): Promise<PublicAddressSnapshot> => ok(api.GET('/admin/public-address', { signal })),
});
export function updatePublicAddress(body: PublicAddressUpdateBody): Promise<PublicAddressUpdated> {
  return accepted(api.PUT('/admin/public-address', { body }));
}
