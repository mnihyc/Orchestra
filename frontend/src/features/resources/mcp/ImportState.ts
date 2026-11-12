/** An accepted act is separate from permission to read a current imported-server association. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import type { McpFound } from '@/generated/api-types';
import { discoveryKey } from './importValues';

export function useImportState(environment: string, machine: string | null = null) {
  const cache = useQueryClient();
  const user = useStore(client.session, (state) => state.userId);
  const version = useStore(client.session, (state) => state.readerVersion);
  const queryKey = ['mcp-import-accepted', user, version, environment, machine] as const;
  const accepted = useQuery<Record<string, string>>({ queryKey, initialData: {}, queryFn: () => Promise.resolve({}), enabled: false, staleTime: Infinity });
  return { accepted: accepted.data,
    acknowledge: (found: McpFound, id: string) => cache.setQueryData<Record<string, string>>(queryKey, (before) => ({ ...before, [discoveryKey(found)]: id })),
  };
}
