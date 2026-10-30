/** A local address in a conversation or a shell (http://localhost:3000/…) leads a reader of its environment's Ports door to that port through Orch. */
import { useQuery } from '@tanstack/react-query';
import { type ReactNode, useCallback } from 'react';
import { portsQuery } from '@/api/ports';
import { LocalAddressRewrite } from '@/lib/markdown';
import { PORTS_POLL_MS, portAddressFor } from './model';

/** A local address as a shell prints one: a loopback or wildcard host with its port, and the path after it. */
const LOCAL_ADDRESS = /(?:https?:\/\/)?(?:localhost|127(?:\.\d{1,3}){3}|0\.0\.0\.0|\[(?:::1|::)\]):\d{1,5}(?:\/[^\s'"<>)\]]*)?/gi;

/** Where a local address leads for this reader: the port's stable address where it has one, else the environment's Ports view of that port; null where the reader has no Ports door here, or before the ports are read. */
export function useLocalAddresses(environmentId: string | null, canView: boolean): ((href: string) => string | null) | null {
  const enabled = environmentId !== null && canView;
  const query = useQuery({ ...portsQuery(environmentId ?? ''), enabled, staleTime: PORTS_POLL_MS });
  const ports = query.data?.ports;
  const rewrite = useCallback((href: string) => environmentId === null || ports === undefined ? null : portAddressFor(href, ports, environmentId), [environmentId, ports]);
  return enabled && ports !== undefined ? rewrite : null;
}

/** The local addresses in one line of text that lead somewhere: where each starts and ends (0-based, end exclusive) and where it leads. */
export function localLinks(text: string, rewrite: ((href: string) => string | null) | null): { start: number; end: number; text: string; target: string }[] {
  if (rewrite === null) return [];
  const found: { start: number; end: number; text: string; target: string }[] = [];
  for (const match of text.matchAll(LOCAL_ADDRESS)) {
    const target = rewrite(match[0]);
    if (target !== null) found.push({ start: match.index, end: match.index + match[0].length, text: match[0], target });
  }
  return found;
}

/** Supplies, below it, where a local address in a message leads. Without the Ports door nothing changes. */
export function LocalAddresses({ environmentId, canView, children }: { environmentId: string | null; canView: boolean; children: ReactNode }) {
  const rewrite = useLocalAddresses(environmentId, canView);
  return <LocalAddressRewrite.Provider value={rewrite}>{children}</LocalAddressRewrite.Provider>;
}
