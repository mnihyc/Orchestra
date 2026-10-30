/** The Export… of a menu: offered to whoever may take the thing away, left out of a row's menu otherwise, and on the thing's own screen switched off with one line and the way to ask.
 *
 * Every menu that has Export… builds its entries here, so that a thread's header, the "⋯" of a place, a chat's
 * menu and a document's say the same: "Export…" where it can be done, and where somebody reads a thing and may not
 * take it away as a file, the entry switched off with "Taking this away as a file is for people <owner> names",
 * and under it Request access.
 */
import type { TFunction } from 'i18next';
import { Download, KeyRound } from 'lucide-react';

import type { UserRef } from '@/generated/api-types';
import type { MenuEntry } from '@/ui/Menu';

export interface ExportEntryOptions {
  /** Whether the caller may take it away: `null` while that is not known yet. */
  allowed: boolean | null;
  /** The first owner of the thing, whom to ask; `null` when the thing does not say. */
  owner: Pick<UserRef, 'display_name'> | null | undefined;
  /** In a row of a list what the caller may not do is left out; on the thing's own screen it stays, switched off, with the reason. */
  where: 'row' | 'screen';
  /** Other words for why it is switched off, where nobody could give the right: then there is no way to ask. */
  reason?: string;
  onExport: () => void;
  /** Opens the form that asks for the Export row of the thing. */
  onRequest: () => void;
  t: TFunction;
}

/** The entries of Export… for one thing: none, the entry, or the entry switched off with Request access under it. */
export function exportEntries({ allowed, owner, where, reason, onExport, onRequest, t }: ExportEntryOptions): MenuEntry[] {
  if (allowed === null) return [];
  if (allowed) return [{ id: 'export', label: t('export.menu.export'), icon: <Download aria-hidden />, onSelect: onExport }];
  if (where === 'row') return [];
  const off: MenuEntry = {
    id: 'export',
    label: t('export.menu.export'),
    icon: <Download aria-hidden />,
    disabled: reason ?? (owner === null || owner === undefined ? t('export.menu.notAllowedNoOwner') : t('export.menu.notAllowed', { name: owner.display_name })),
    onSelect: onExport,
  };
  return reason === undefined ? [off, { id: 'export-request', label: t('export.menu.request'), icon: <KeyRound aria-hidden />, onSelect: onRequest }] : [off];
}
