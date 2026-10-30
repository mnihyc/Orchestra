/** Already-served org addresses are enough for most setting links; reading a row starts no request. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { nodeQuery, orgsQuery } from '@/api/nodes';
import type { Org } from '@/generated/api-types';
import { useTreeNode } from '@/realtime/stores/tree';

export function useSourceOrgAddress(id: string | null): string | undefined {
  const client = useQueryClient();
  const node = useTreeNode(id);
  const roots = useQuery({ ...orgsQuery, enabled: false });
  const detail = useQuery({ ...nodeQuery(id ?? ''), enabled: false });
  if (id === null) return undefined;
  return (node?.kind === 'org' ? node.address ?? undefined : undefined)
    ?? roots.data?.find((root) => root.id === id)?.address
    ?? client.getQueriesData<Org>({ queryKey: ['org'] }).find(([, value]) => value?.id === id)?.[1]?.address
    ?? (!detail.isError && detail.data?.kind === 'org' ? detail.data.address : undefined);
}
