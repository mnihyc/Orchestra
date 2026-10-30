/** Editing a text file where it is shown: saved on the version it was read at, unsaved text kept on the device for its person alone, the file that changed underneath (Compare, Keep mine, Take theirs), and why a file cannot be edited.
 *
 * Nothing is saved by itself. What was typed and not saved is kept in the browser's storage under
 * the id of the person signed in, for seven days at most, and is put back when the file is opened
 * again; it is removed when it is saved or discarded. A file that changed while its text was being
 * edited is never overwritten: the person is shown what happened and decides.
 */
import { GitCompare, Lock } from 'lucide-react';
import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';

import { useWorkbench } from '@/features/workbench/store';
import { formatDateTime } from '@/lib/time';
import { useFormatContext } from '@/lib/appearance';
import { Button } from '@/ui/Button';
import { InlineAlert } from '@/ui/InlineAlert';

import { workspacePermissionLabel } from './door';
import type { ReadOnlyReason } from './viewers';

// ---------------------------------------------------------------- what is kept on the device

/** Unsaved text is not kept beyond a week: longer than a weekend away, short enough that forgotten text does not sit on a machine. */
export const UNSAVED_KEPT_MS = 7 * 24 * 60 * 60 * 1000;

const PREFIX = (userId: string) => `orch.${userId}.unsaved.`;

/** What was typed into a file and not saved. */
export interface KeptText {
  /** The version it was typed on. */
  base: string | null;
  text: string;
  /** When it was kept, in milliseconds. */
  at: number;
}

const keyOf = (userId: string, environmentId: string, path: string) => `${PREFIX(userId)}${environmentId}.${path}`;

export function keepUnsaved(userId: string | null, environmentId: string, path: string, kept: Omit<KeptText, 'at'>, now: number = Date.now()): void {
  if (userId === null) return;
  try {
    localStorage.setItem(keyOf(userId, environmentId, path), JSON.stringify({ ...kept, at: now }));
  } catch {
    // A device that keeps nothing (full, or private) loses the text with the tab, which is all it can do.
  }
}

/** The text kept for a file, unless it is older than a week, in which case it is removed. */
export function readUnsaved(userId: string | null, environmentId: string, path: string, now: number = Date.now()): KeptText | null {
  if (userId === null) return null;
  try {
    const raw = localStorage.getItem(keyOf(userId, environmentId, path));
    if (raw === null) return null;
    const kept = JSON.parse(raw) as Partial<KeptText>;
    if (typeof kept.text !== 'string' || typeof kept.at !== 'number' || now - kept.at > UNSAVED_KEPT_MS) {
      localStorage.removeItem(keyOf(userId, environmentId, path));
      return null;
    }
    return { base: typeof kept.base === 'string' ? kept.base : null, text: kept.text, at: kept.at };
  } catch {
    return null;
  }
}

export function forgetUnsaved(userId: string | null, environmentId: string, path: string): void {
  if (userId === null) return;
  try {
    localStorage.removeItem(keyOf(userId, environmentId, path));
  } catch {
    // Nothing was kept that could be removed.
  }
}

/** How many files hold text that was not saved: what signing out asks about. */
export function countUnsaved(userId: string | null, now: number = Date.now()): number {
  if (userId === null) return 0;
  let count = 0;
  try {
    for (let index = 0; index < localStorage.length; index += 1) {
      const key = localStorage.key(index);
      if (!key?.startsWith(PREFIX(userId))) continue;
      const kept = JSON.parse(localStorage.getItem(key) ?? 'null') as Partial<KeptText> | null;
      if (kept !== null && typeof kept.at === 'number' && now - kept.at <= UNSAVED_KEPT_MS) count += 1;
    }
  } catch {
    return count;
  }
  return count;
}

// ---------------------------------------------------------------- what the tab says

/** The layout owns the mark until a file is saved or its tab closes, including while its body is paused. */
export function useUnsavedMark(tabId: string, dirty: boolean): void {
  useEffect(() => {
    useWorkbench.getState().setUnsaved(tabId, dirty);
  }, [tabId, dirty]);
}

/** Why a file cannot be edited, said in the tab. */
export function ReadOnlyNote({ reason }: { reason: ReadOnlyReason }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  let text: string;
  switch (reason.kind) {
    case 'small_screen':
      text = t('craftFiles.readOnly');
      break;
    case 'permission':
      text = t('files.edit.why.permission', { permission: workspacePermissionLabel(t, 'files.manage') });
      break;
    case 'offline':
      text =
        reason.asOf === null
          ? t('files.edit.why.offlineNoTime', { name: reason.environment })
          : t('files.edit.why.offline', { name: reason.environment, time: formatDateTime(reason.asOf, format) });
      break;
    case 'no_writes':
      text = reason.machine === null ? t('files.edit.why.noWritesHere') : t('files.edit.why.noWrites', { machine: reason.machine });
      break;
    case 'viewer':
      text = t('files.edit.why.viewer');
      break;
    case 'too_large':
      text = t('files.edit.why.tooLarge');
      break;
  }
  return (
    <p className="flex items-center gap-2 border-b border-border bg-bg-subtle px-3 py-1 text-sm text-text-muted" data-testid="read-only-note">
      <Lock className="shrink-0" aria-hidden />
      <span className="min-w-0 flex-1 wrap-anywhere">{text}</span>
    </p>
  );
}

export interface ChangedBarProps {
  /** Who changed it, where the changes of a turn say so: "Agent · turn 14". */
  by: string | null;
  comparing: boolean;
  onCompare: () => void;
  onKeepMine: () => void;
  onTakeTheirs: () => void;
  pending: boolean;
}

/** The file changed while its text was being edited: nothing was written, and the person decides. */
export function ChangedBar({ by, comparing, onCompare, onKeepMine, onTakeTheirs, pending }: ChangedBarProps) {
  const { t } = useTranslation();
  return (
    <InlineAlert
      tone="warning"
      className="m-2"
      title={t('files.edit.changed.title')}
      action={
        <>
          <Button size="sm" icon={<GitCompare aria-hidden />} onClick={onCompare} disabled={comparing || pending}>
            {t('files.edit.changed.compare')}
          </Button>
          <Button size="sm" onClick={onKeepMine} disabled={pending}>
            {t('files.edit.changed.keepMine')}
          </Button>
          <Button size="sm" onClick={onTakeTheirs} pending={pending}>
            {t('files.edit.changed.takeTheirs')}
          </Button>
        </>
      }
    >
      {by === null ? t('files.edit.changed.what') : t('files.edit.changed.whatBy', { by })}
    </InlineAlert>
  );
}
