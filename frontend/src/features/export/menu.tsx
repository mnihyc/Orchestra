/** Export… stays in the menu of a visible object, with its current decision and the way to ask.
 *
 * Every menu that has Export… builds its entries here, so that a thread's header, the "⋯" of a place, a chat's
 * menu and a document's say the same: "Export…" where it can be done, and where somebody reads a thing and may not
 * take it away as a file, the entry switched off with "Taking this away as a file is for people <owner> names",
 * and under it Request access.
 */
import type { TFunction } from 'i18next';
import { Download, KeyRound, RefreshCw } from 'lucide-react';

import type { UserRef } from '@/generated/api-types';
import type { MenuEntry } from '@/ui/Menu';

export interface ExportCheckFailure {
  /** The shared error reader's safe, localized explanation. */
  reason: string;
  /** Only a failed read is repeated, never an export or access request. */
  retry?: () => void;
}

export interface ExportEntryOptions {
  /** Whether the caller may take it away: `null` while that is not known yet. */
  allowed: boolean | null;
  /** The first owner of the thing, whom to ask; `null` when the thing does not say. */
  owner: Pick<UserRef, 'display_name'> | null | undefined;
  /** Rows and screens share the same Export decision and explanation. */
  where: 'row' | 'screen';
  /** Other words for why it is switched off, where nobody could give the right: then there is no way to ask. */
  reason?: string;
  checkFailure?: ExportCheckFailure;
  onExport: () => void;
  /** Opens the form that asks for the Export row of the thing. */
  onRequest: () => void;
  t: TFunction;
}

/** Checking never becomes a denial; only a known, requestable refusal offers Request access. */
export function exportEntries({ allowed, owner, reason, checkFailure, onExport, onRequest, t }: ExportEntryOptions): MenuEntry[] {
  if (allowed === null) {
    const checking: MenuEntry = { id: 'export', label: t('export.menu.export'), icon: <Download aria-hidden />,
      disabled: checkFailure?.reason ?? t('export.menu.checking'), onSelect: onExport };
    const retry = checkFailure?.retry;
    return retry === undefined ? [checking] : [checking, { id: 'export-retry', label: t('common.tryAgain'), icon: <RefreshCw aria-hidden />, onSelect: retry }];
  }
  if (allowed) return [{ id: 'export', label: t('export.menu.export'), icon: <Download aria-hidden />, onSelect: onExport }];
  const off: MenuEntry = {
    id: 'export',
    label: t('export.menu.export'),
    icon: <Download aria-hidden />,
    disabled: reason ?? (owner === null || owner === undefined ? t('export.menu.notAllowedNoOwner') : t('export.menu.notAllowed', { name: owner.display_name })),
    onSelect: onExport,
  };
  return reason === undefined ? [off, { id: 'export-request', label: t('export.menu.request'), icon: <KeyRound aria-hidden />, onSelect: onRequest }] : [off];
}
