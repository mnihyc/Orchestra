/** Org home and settings resolve the same identifier alias before drawing their canonical address. */
import { nodeQuery, orgBySlugQuery } from '@/api/nodes';
import { queryClient } from '@/api/query';

/** This is the identifier shape already accepted by the org-home route, rather than an org's short name. */
export const isOrgIdAddress = (value: string): boolean => /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value);

export async function canonicalOrgAddress(value: string): Promise<string | undefined> {
  if (!isOrgIdAddress(value)) return undefined;
  const node = await queryClient.query(nodeQuery(value));
  if (node.kind !== 'org') return undefined;
  // The native node answer is already this reader's complete Org; the canonical page shares it.
  queryClient.setQueryData(orgBySlugQuery(node.address).queryKey, node);
  return node.address;
}
