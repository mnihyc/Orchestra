/** Home moves retain identity; another org must use the separate native copy-and-delete act. */
import type { TFunction } from 'i18next';
import { FolderInput } from 'lucide-react';
import type { DuplicateKind } from '@/api/duplicates';
import type { ChatKind, NodeRef, ResourceAccess } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import type { MenuEntry } from '@/ui/Menu';
import type { PickedPlace } from '@/features/settings/PlacePicker';
import { useDuplicateHost } from './target';

export type HomeMoveKind = DuplicateKind | 'host' | 'machine';
export interface HomeMoveTarget { kind: HomeMoveKind; id: string; name: string; rootOrgId: string; revision?: number; draft?: boolean; imported?: boolean }
interface MoveRow { id: string; name: string; revision?: number; org_id?: string | null; home_path?: readonly NodeRef[]; access?: ResourceAccess | null; imported?: unknown }
export function moveRoot(row: Pick<MoveRow, 'org_id' | 'home_path'>): string | undefined {
  if (typeof row.org_id === 'string') return row.org_id;
  const root = row.home_path?.[0];
  return root?.kind === 'org' ? root.id : undefined;
}
export function homeMoveReason(target: HomeMoveTarget, place: PickedPlace, t: TFunction): string | false {
  if (place.kind !== 'org' && place.kind !== 'project') return t('duplicate.homeRequired');
  if (place.rootOrgId === undefined) return t('duplicate.homeUnknown');
  return target.rootOrgId === place.rootOrgId ? false : t('duplicate.moveOtherOrgUnavailable');
}
function moveEntries(t: TFunction, kind: HomeMoveKind, row: Pick<MoveRow, 'id' | 'name' | 'revision'>, rootOrgId: string | undefined, manages: boolean, draft = false, imported = false): MenuEntry[] {
  const reason = !manages ? t('duplicate.manageRequired') : rootOrgId === undefined ? t('duplicate.homeUnknown') : false;
  return [{ id: 'move-home', label: t('duplicate.move'), icon: <FolderInput aria-hidden />, disabled: reason, onSelect: () => {
    if (reason !== false || rootOrgId === undefined || useDuplicateHost.getState().current !== null) return;
    useDuplicateHost.getState().move({ kind, id: row.id, name: row.name, rootOrgId, ...(draft ? { draft: true } : {}), ...(imported ? { imported: true } : {}), ...(row.revision === undefined ? {} : { revision: row.revision }) });
  } }];
}
export function homeMoveEntries(t: TFunction, kind: Exclude<HomeMoveKind, 'chat'>, row: MoveRow, options: { draft?: boolean } = {}): MenuEntry[] {
  return moveEntries(t, kind, row, moveRoot(row), holds(row.access, `${kind === 'mcp_server' ? 'mcp' : kind}.manage`), options.draft === true, kind === 'mcp_server' && row.imported !== undefined && row.imported !== null);
}
export function groupHomeMoveEntries(t: TFunction, chat: { id: string; name: string | null; kind?: ChatKind; org_id?: string; can?: { manage?: boolean } }): MenuEntry[] {
  if (chat.kind !== undefined && chat.kind !== 'group') return [];
  return moveEntries(t, 'chat', { id: chat.id, name: chat.name ?? t('chat.title.groupUnnamed') }, chat.org_id, chat.can?.manage === true);
}
