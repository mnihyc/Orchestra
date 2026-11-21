/** Each folder control holds only its current authorized native directory page. */
import { useQuery } from '@tanstack/react-query';
import { ChevronLeft, ChevronRight } from 'lucide-react';
import { useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { listMachineDirectories } from '@/api/directories';
import type { Door } from '@/api/workspace';
import { workspaceBinding } from '@/api/workspaceBinding';
import { DirectoryPager } from '@/features/files/DirectoryPager';
import { useEnvironment } from '@/features/files/door';
import { useDirectoryTree } from '@/features/files/useDirectoryTree';
import type { Machine } from '@/generated/api-types';
import { directoryKey } from '@/realtime/stores/environment';
import { IconButton } from '@/ui/IconButton';

export type DirectoryTarget = { kind: 'machine'; machine: Machine } | { kind: 'environment'; door: Door | null };
export interface DirectoryLevel {
  names: readonly string[];
  pending: boolean;
  ready: boolean;
  issue: unknown;
  pager: ReactNode;
  retry: () => void;
}
interface Props {
  target: DirectoryTarget; root: string; path: string; prefix: string; recent: readonly string[]; enabled: boolean;
  children: (level: DirectoryLevel) => ReactNode;
}
const EMPTY: DirectoryLevel = { names: [], pending: false, ready: false, issue: null, pager: null, retry: () => undefined };
export const directoryScope = (target: DirectoryTarget): string => target.kind === 'machine' ? `machine:${target.machine.id}` : `environment:${target.door?.environmentId ?? 'new'}`;
export const directoryReader = (): string => JSON.stringify([client.session.getState().userId, client.session.getState().readerVersion]);

export function DirectoryListing(props: Props) {
  if (props.target.kind === 'machine') return <MachineLevel {...props} machine={props.target.machine.id} />;
  if (props.target.door === null) return props.children(EMPTY);
  return <EnvironmentLevel {...props} door={props.target.door} />;
}

function MachineLevel({ machine, root, path, prefix, recent, enabled, children }: Props & { machine: string }) {
  const { t } = useTranslation();
  const reader = useStore(client.session, () => directoryReader());
  const signature = JSON.stringify([reader, machine, root, path, prefix, recent]);
  const [held, hold] = useState<{ signature: string; trail: (string | null)[]; at: number }>(() => ({ signature, trail: [null], at: 0 }));
  const page = held.signature === signature ? held : { signature, trail: [null], at: 0 };
  if (page !== held) hold(page);
  const cursor = page.trail[page.at] ?? null;
  const query = useQuery({
    queryKey: ['machine-directories', signature, cursor],
    queryFn: ({ signal }) => listMachineDirectories(machine, { read_kind: 'choices', root, path, prefix, recent: JSON.stringify(recent), cursor, limit: null }, signal),
    enabled, retry: false, staleTime: 0, refetchOnMount: 'always',
  });
  const ready = enabled && query.isSuccess && !query.isFetching && query.data.read_kind === 'choices';
  const next = ready ? query.data.next_cursor : null;
  const retry = () => { if (page.at === 0) void query.refetch(); else hold({ signature, trail: [null], at: 0 }); };
  const pager = next === null && page.at === 0 ? null : <span className="inline-flex items-center gap-1">
    <IconButton size="sm" label={t('files.pages.previous')} icon={<ChevronLeft aria-hidden />} disabled={query.isFetching || page.at === 0}
      onClick={() => hold({ ...page, at: page.at - 1 })} />
    <IconButton size="sm" label={t('files.pages.next')} icon={<ChevronRight aria-hidden />} disabled={query.isFetching || next === null}
      onClick={() => { if (next !== null) hold({ signature, trail: [...page.trail.slice(0, page.at + 1), next], at: page.at + 1 }); }} />
  </span>;
  return children({ names: ready ? query.data.entries : [], pending: enabled && query.isFetching, ready,
    issue: query.isFetching ? null : query.error, pager, retry });
}

function EnvironmentLevel({ door, path, prefix, recent, enabled, children }: Props & { door: Door }) {
  const { t } = useTranslation();
  const { environment, failed } = useEnvironment(enabled ? door.environmentId : null);
  const binding = environment === undefined || failed !== null ? null : workspaceBinding(environment);
  const { tree, directories, capacity } = useDirectoryTree(door, path, binding, enabled);
  const page = enabled ? directories.get(directoryKey(door.environmentId, path)) : undefined;
  const invalid = page !== undefined && (page.invalidated === true || page.restartRequired || page.status === 'failed' || page.watch === 'refused');
  const issue = failed ?? (capacity !== null || (page?.capacity ?? null) !== null ? t('files.pages.unavailable') : invalid ? page.error ?? t('directoryChoice.readFailed') : null);
  const ready = enabled && binding !== null && issue === null && page?.status === 'ready';
  const wanted = prefix.toLocaleLowerCase();
  const names = ready ? page.entries.filter((entry) => entry.type === 'dir' && entry.name.toLocaleLowerCase().startsWith(wanted)).map((entry) => entry.name) : [];
  const order = new Map(recent.map((name, at) => [name, at]));
  names.sort((left, right) => (order.get(left) ?? Infinity) - (order.get(right) ?? Infinity));
  return children({ names, ready, issue, pending: enabled && issue === null && (environment === undefined || page === undefined || page.status === 'loading'),
    pager: page === undefined ? null : <DirectoryPager window={page} path={path} navigate={(at, action) => tree.navigate(at, action)} />,
    retry: () => tree.navigate(path, page?.invalidated === true || page?.restartRequired === true ? 'restart' : 'retry') });
}
