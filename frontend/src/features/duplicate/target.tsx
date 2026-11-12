/** Every menu opens the same native copy flow, carrying only the facts already visible there. */
import type { TFunction } from 'i18next';
import { Copy, CopyPlus } from 'lucide-react';
import { create } from 'zustand';
import type { DuplicateKind } from '@/api/duplicates';
import type { CopySetupNote } from '@/api/copySetup';
import { libraryReader } from '@/api/resourceLibrary';
import type { DocumentDirectory, ResourceAccess } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import type { HomeMoveTarget } from './move';
import type { MenuEntry } from '@/ui/Menu';
import { copySetupDescription } from './CopySetup';

export interface DuplicateTarget {
  kind: DuplicateKind | 'document' | 'thread';
  directory?: DocumentDirectory;
  id: string;
  name: string;
  revision?: number;
  /** Only the unpublished editor can name the draft-reading operation. */
  draft?: boolean;
  imported?: boolean;
  /** Only the setup facts supplied by this readable native source. Never sent as authority. */
  setup?: readonly CopySetupNote[];
  /** A permission-page name is not readable source configuration. */
  refusal?: string;
}
type Asked = ({ mode: 'duplicate'; target: DuplicateTarget; quick: boolean } | { mode: 'move'; target: HomeMoveTarget }) & { serial: number; reader: ReturnType<typeof libraryReader> };
interface Host { current: Asked | null; show: (target: DuplicateTarget, quick: boolean) => void; move: (target: HomeMoveTarget) => void; close: () => void }
let serial = 0;
export const useDuplicateHost = create<Host>((set) => ({ current: null,
  show: (target, quick) => { serial += 1; set({ current: { mode: 'duplicate', target, quick, serial, reader: libraryReader() } }); },
  move: (target) => { serial += 1; set({ current: { mode: 'move', target, serial, reader: libraryReader() } }); },
  close: () => set({ current: null }),
}));
export function openDuplicate(target: DuplicateTarget, quick = false): void {
  if (target.refusal === undefined && useDuplicateHost.getState().current === null) useDuplicateHost.getState().show(target, quick);
}
export function duplicateEntries(t: TFunction, target: DuplicateTarget): MenuEntry[] {
  const description = target.refusal === undefined ? copySetupDescription(t, target.kind, target.setup) : undefined;
  const detail = description === undefined ? {} : { description };
  return [
    { id: 'duplicate', label: t(target.draft === true ? 'duplicate.draft' : 'duplicate.now'), ...detail, icon: <CopyPlus aria-hidden />, disabled: target.refusal ?? false, onSelect: () => openDuplicate(target, true) },
    { id: 'duplicate-to', label: t('duplicate.to'), ...detail, icon: <Copy aria-hidden />, disabled: target.refusal ?? false, onSelect: () => openDuplicate(target) },
  ];
}

/** Permission-page names stay visible, without treating them as readable configuration. */
export function resourceDuplicateEntries(t: TFunction, kind: Exclude<DuplicateKind, 'chat'>, row: { id: string; name: string; revision?: number; access: ResourceAccess }, options: Pick<DuplicateTarget, 'draft' | 'imported' | 'setup'> & { sourceView?: string } = {}): MenuEntry[] {
  const view = options.sourceView ?? `${kind === 'mcp_server' ? 'mcp' : kind}.view`;
  const readable = (options.draft === true ? holds(row.access, 'skill.manage') : holds(row.access, view)) && (kind !== 'environment' || holds(row.access, 'files.view'));
  return duplicateEntries(t, { kind, id: row.id, name: row.name, ...(options.draft === undefined ? {} : { draft: options.draft }), ...(options.imported === undefined ? {} : { imported: options.imported }),
    ...(row.revision === undefined ? {} : { revision: row.revision }),
    ...(readable && options.setup !== undefined ? { setup: options.setup } : {}),
    ...(readable ? {} : { refusal: t(kind === 'environment' ? 'duplicate.filesRequired' : 'duplicate.readRequired') }),
  });
}
