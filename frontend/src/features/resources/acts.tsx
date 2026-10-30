/** What a row of a list of resources can do, said once: its button, its menu, its right-click menu and the acts several ticked rows share.
 *
 * Every row has at its end the act most used as a visible button, and a menu with
 * the rest; the same entries open under the right mouse button, with the menu key and with `Shift+F10`.
 * An act the reader may not do is not offered in a row. Rows that are ticked have the acts they all take
 * above the table, and an act that destroys names what it is about to destroy before it does it.
 */
import type { TFunction } from 'i18next';
import { Ellipsis, LoaderCircle, Pencil, Share2 } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { AccessTarget } from '@/api/access';
import { openAccess } from '@/features/access/AccessDialog';
import { copyText } from '@/ui/clipboard';
import type { ResourceAccess } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { useErrorText } from '@/ui/errorText';
import { IconButton } from '@/ui/IconButton';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { toast } from '@/ui/Toast';
import { Tooltip } from '@/ui/Tooltip';
import { AdminActNotice, adminSpec, isAdminAct, type AdminActSpec } from '@/ui/AdminAct';

/** One thing a person can do with a resource. */
export interface Act {
  admin?: AdminActSpec;
  id: string;
  label: string;
  icon?: ReactNode;
  /** One quiet line under the label in the menu, where the label alone would not say what it does. */
  description?: string;
  /** The key that does the same, shown in the menu. */
  shortcut?: string;
  /** Marks an act that destroys. */
  danger?: boolean;
  /** Why it cannot be done now: the act stays in the menu, greyed, with these words. */
  disabled?: string | false;
  /** The act is under way: where it is a button it shows that, and takes no second press. */
  pending?: boolean;
  run: () => void;
}

/** These existing resource acts explicitly require their kind's Manage row; navigation and future acts get no guessed permission. */
const MANAGEMENT_ACTS: ReadonlySet<string> = new Set(['cleanup', 'next', 'key', 'enable', 'disable', 'edit', 'rename', 'delete', 'start', 'stop', 'restart', 'update', 'update-now', 'recreate', 'move', 'keep', 'resume', 'pause', 'recycle', 'again', 'refresh', 'revoke']);
export function adminEntries(entries: readonly MenuEntry[], admin: AdminActSpec | undefined): MenuEntry[] {
  if (!isAdminAct(admin)) return [...entries];
  return entries.map((entry) => entry.type === 'sub' ? { ...entry, entries: adminEntries(entry.entries, admin) }
    : entry.type !== undefined || !MANAGEMENT_ACTS.has(entry.id) ? entry
      : { ...entry, description: <>{entry.description}<AdminActNotice spec={admin} /></> });
}
export function adminRow(acts: RowActs, admin: AdminActSpec): RowActs {
  return { ...acts, primary: acts.primary === null || !MANAGEMENT_ACTS.has(acts.primary.id) ? acts.primary : { ...acts.primary, admin }, entries: adminEntries(acts.entries, admin) };
}

/** What a row can do. */
export interface RowActs {
  /** The act most used, always visible at the end of the row. */
  primary: Act | null;
  /** In the order of the menu: where it leads and what changes it first, what destroys last and apart. */
  entries: readonly MenuEntry[];
  /** The acts that have a key of their own (`F2` renames), by the key, for whatever of the row has the keyboard. */
  keys: Readonly<Record<string, () => void>>;
  /** An act of this row is under way. */
  busy: boolean;
}

/** An act as an entry of a menu. */
export function entryOf(act: Act): MenuEntry {
  const admin = isAdminAct(act.admin) ? act.admin : undefined;
  const description = admin === undefined ? act.description : <>{act.description}<AdminActNotice spec={admin} /></>;
  return {
    id: act.id,
    label: act.label,
    ...(act.icon === undefined ? {} : { icon: act.icon }),
    ...(description === undefined ? {} : { description }),
    ...(act.shortcut === undefined ? {} : { shortcut: act.shortcut }),
    ...(act.danger === true ? { danger: true } : {}),
    ...(act.disabled === undefined ? {} : { disabled: act.disabled }),
    onSelect: act.run,
  };
}

/** Groups of acts as the entries of a menu, a line between the groups; an act that is absent (`null`) is left out, and so is a group with nothing in it. */
export function entriesOf(...groups: readonly (readonly (Act | null | false)[])[]): MenuEntry[] {
  const entries: MenuEntry[] = [];
  for (const group of groups) {
    const acts = group.filter((act): act is Act => act !== null && act !== false);
    if (acts.length === 0) continue;
    if (entries.length > 0) entries.push({ type: 'separator', id: `apart-${entries.length}` });
    entries.push(...acts.map(entryOf));
  }
  return entries;
}

/** The keys of the acts that have one: only a key pressed alone, by name (`F2`); an act that cannot be done now answers to none. */
export function keysOf(...acts: readonly (Act | null | false)[]): Record<string, () => void> {
  const keys: Record<string, () => void> = {};
  for (const act of acts) {
    if (act === null || act === false || act.shortcut === undefined || typeof act.disabled === 'string') continue;
    keys[act.shortcut] = act.run;
  }
  return keys;
}

/**
 * The act that opens a resource's permission page with the field that adds people in focus. Whoever holds something on the
 * resource may share it (the page itself says how far, and with whom), so it is offered to everyone the row is shown to.
 */
export function shareAct(t: TFunction, target: AccessTarget, access: Pick<ResourceAccess, 'permissions' | 'administers'>): Act | null {
  if (access.permissions.length === 0 && !access.administers) return null;
  return { id: 'share', label: t('access.menu.share'), icon: <Share2 aria-hidden />, run: () => openAccess(target) };
}

/** Copies the address of a screen, and says that it did, or that the browser would not let it. */
export function useCopyLink(): (path: string) => void {
  const { t } = useTranslation();
  return (path) => void copyText(`${location.origin}${path}`, t('common.linkCopied'));
}

/** Which rows have an act under way, and the means to carry an act out so that its row says so until the answer comes. */
export function useBusy(): { busy: (id: string) => boolean; track: (id: string, work: () => Promise<unknown>) => void } {
  const { t } = useTranslation();
  const fail = useErrorText();
  const [busy, setBusy] = useState<ReadonlySet<string>>(new Set());
  const track = (id: string, work: () => Promise<unknown>) => {
    setBusy((before) => new Set(before).add(id));
    work()
      .catch((error: unknown) => toast({ tone: 'danger', title: t('resources.act.failed'), description: fail(error).message }))
      .finally(() =>
        setBusy((before) => {
          const after = new Set(before);
          after.delete(id);
          return after;
        }),
      );
  };
  return { busy: (id) => busy.has(id), track };
}

/** The sign that an act of a row is under way, said to a screen reader by the row's name. */
function Working({ name }: { name: string }) {
  const { t } = useTranslation();
  return (
    <span role="status" className="inline-flex size-(--control-sm) items-center justify-center text-text-muted">
      <LoaderCircle className="animate-spin" aria-hidden />
      <span className="sr-only">{t('resources.act.working', { name })}</span>
    </span>
  );
}

/**
 * The main act is visible beside the menu at every width, without hover or focus.
 */
export function RowPrimaryCell({ name, acts }: { name: string; acts: RowActs }) {
  const { t } = useTranslation();
  const { primary } = acts;
  if (acts.busy) return <Working name={name} />;
  if (primary === null) return null;
  return (
    <Tooltip content={typeof primary.disabled === 'string' ? primary.disabled : null}>
      <Button
        admin={primary.admin}
        size="sm"
        variant="secondary"
        icon={primary.icon}
        blocked={typeof primary.disabled === 'string'}
        pending={primary.pending === true}
        aria-label={t('resources.act.of', { act: primary.label, name })}
        onClick={primary.run}
      >
        {primary.label}
      </Button>
    </Tooltip>
  );
}

/** The primary button is always present; the row's menus contain the remaining acts once. */
export function secondaryEntries(acts: RowActs): readonly MenuEntry[] {
  const remaining: MenuEntry[] = [];
  for (const entry of acts.entries) {
    if ((entry.type === undefined || entry.type === 'item') && entry.id === acts.primary?.id) continue;
    if (entry.type === 'separator' && (remaining.length === 0 || remaining.at(-1)?.type === 'separator')) continue;
    remaining.push(entry);
  }
  if (remaining.at(-1)?.type === 'separator') remaining.pop();
  return remaining;
}

/** The end of a row: the menu of every act of the row, and the sign that one is under way where there is no place for it beside the menu. */
export function RowMenuCell({ name, acts, working }: { name: string; acts: RowActs; working: boolean }) {
  const { t } = useTranslation();
  const entries = secondaryEntries(acts);
  if (entries.length === 0) return working && acts.busy ? <Working name={name} /> : null;
  return (
    <div className="flex items-center justify-end gap-1">
      {working && acts.busy && <Working name={name} />}
      <Menu
        label={t('resources.act.menuOf', { name })}
        align="end"
        entries={entries}
        trigger={<IconButton size="sm" variant="secondary" label={t('resources.act.menuOf', { name })} icon={<Ellipsis aria-hidden />} />}
      />
    </div>
  );
}

/** The acts in the heading of a resource's page, at the end of the line with its name: the act most used as a button, Edit, and the menu of the rest. */
export function HeadingActs({
  name,
  primary,
  edit,
  menu,
  before,
  admin,
}: {
  name: string;
  primary: Act | null;
  edit: Act | null;
  menu: readonly MenuEntry[];
  /** What else stands first: an act that belongs to the kind alone. */
  before?: ReactNode;
  admin?: AdminActSpec;
}) {
  const { t } = useTranslation();
  return (
    <>
      {before}
      {primary !== null && (
        <Tooltip content={typeof primary.disabled === 'string' ? primary.disabled : null}>
          <Button size="sm" admin={primary.admin ?? (MANAGEMENT_ACTS.has(primary.id) ? admin : undefined)} icon={primary.icon} pending={primary.pending === true} blocked={typeof primary.disabled === 'string'} onClick={primary.run}>
            {primary.label}
          </Button>
        </Tooltip>
      )}
      {edit !== null && (
        <Button size="sm" admin={admin} icon={<Pencil aria-hidden />} onClick={edit.run}>
          {t('resources.act.editButton')}
        </Button>
      )}
      {menu.length > 0 && (
        <Menu
          label={t('resources.act.menuOf', { name })}
          align="end"
          entries={adminEntries(menu, admin)}
          trigger={<IconButton size="sm" variant="secondary" label={t('resources.act.menuOf', { name })} icon={<Ellipsis aria-hidden />} />}
        />
      )}
    </>
  );
}

// ---------------------------------------------------------------- several rows at once

/** An act several ticked rows may share. */
interface BulkActBase<Row> {
  id: string;
  label: string;
  icon?: ReactNode;
  danger?: boolean;
  /** Whether this row takes the act; one row that does not takes it from the whole selection. */
  can: (row: Row) => boolean;
  /** Asked once, naming what is about to be done, before the first row is touched. */
  confirm?: (rows: readonly Row[]) => { title: string; body: ReactNode; label: string };
  /** What is said when every row is done: "Switched on 3 providers". */
  done: (count: number) => string;
}

/** An immediate row act or one reviewed dialog for the whole selection. */
export type BulkAct<Row> = BulkActBase<Row> & ({ run: (row: Row) => Promise<unknown>; open?: undefined } | { open: (rows: readonly Row[]) => void; run?: undefined });

/** What a list gives its rows to do. */
export interface ListActs<Row> {
  /** What a row is called, for the names of its controls. */
  name: (row: Row) => string;
  /** The address or owner qualifies competing equal-name rows in their action menu. */
  menuName?: (row: Row) => string;
  of: (row: Row) => RowActs;
  /** The acts rows share; a selection without any permitted bulk act can still be cleared. */
  bulk?: readonly BulkAct<Row>[];
  /** What the acts open (their dialogs), drawn once for the list. */
  dialogs?: ReactNode;
}

/** The rows that are ticked, by the key of each; a row that is no longer in the list is no longer ticked. */
export function useSelection<Row>(rows: readonly Row[], keyOf: (row: Row) => string) {
  const [ticked, setTicked] = useState<ReadonlySet<string>>(new Set());
  const selected = rows.filter((row) => ticked.has(keyOf(row)));
  return {
    selected,
    isTicked: (row: Row) => ticked.has(keyOf(row)),
    toggle: (row: Row, on: boolean) =>
      setTicked((before) => {
        const after = new Set(before);
        if (on) after.add(keyOf(row));
        else after.delete(keyOf(row));
        return after;
      }),
    all: selected.length === rows.length && rows.length > 0 ? (true as const) : selected.length === 0 ? (false as const) : ('indeterminate' as const),
    toggleAll: (on: boolean) => setTicked(on ? new Set(rows.map(keyOf)) : new Set()),
    clear: () => setTicked(new Set()),
  };
}

/** The box that ticks one row, named by the row. */
export function RowBox({ name, checked, onChange }: { name: string; checked: boolean; onChange: (on: boolean) => void }) {
  const { t } = useTranslation();
  return <Checkbox label={<span className="sr-only">{t('resources.bulk.select', { name })}</span>} checked={checked} onCheckedChange={onChange} />;
}

/** The box at the head of the column, which ticks every row there is. */
export function AllBox({ checked, onChange }: { checked: boolean | 'indeterminate'; onChange: (on: boolean) => void }) {
  const { t } = useTranslation();
  return <Checkbox label={<span className="sr-only">{t('resources.bulk.selectAll')}</span>} checked={checked} onCheckedChange={onChange} />;
}

/** The acts the ticked rows share, above the table. */
export function BulkBar<Row>({
  rows,
  acts,
  onClear,
  onDone,
  permission,
}: {
  rows: readonly Row[];
  acts: readonly BulkAct<Row>[];
  onClear: () => void;
  /** An optional completion callback; the registry of acts refreshes the list. */
  onDone?: () => void;
  permission?: string | readonly string[];
}) {
  const { t } = useTranslation();
  const fail = useErrorText();
  const [asking, setAsking] = useState<BulkAct<Row> | null>(null);
  const [working, setWorking] = useState<string | null>(null);
  const [error, setError] = useState<unknown>(undefined);
  const shared = acts.filter((act) => rows.every(act.can));
  const admin = permission === undefined ? undefined : rows.map((row) => adminSpec(row, permission)).find(isAdminAct);

  const run = async (act: BulkAct<Row>) => {
    if (act.open !== undefined) { act.open(rows); return; }
    setWorking(act.id);
    setError(undefined);
    const failed: string[] = [];
    let last: unknown;
    for (const row of rows) {
      try {
        await act.run(row);
      } catch (cause) {
        failed.push(fail(cause).message);
        last = cause;
      }
    }
    setWorking(null);
    setAsking(null);
    onDone?.();
    if (failed.length === 0) {
      toast({ tone: 'success', title: act.done(rows.length) });
      onClear();
    } else {
      setError(last);
      toast({ tone: 'danger', title: t('resources.bulk.failed', { failed: failed.length, count: rows.length }), description: failed[0] ?? '' });
    }
  };

  const ask = asking === null ? null : asking.confirm?.(rows);
  return (
    <div role="toolbar" aria-label={t('resources.bulk.label')} className="flex flex-wrap items-center gap-2 rounded-lg border border-border bg-bg-subtle px-3 py-2">
      <span className="text-sm font-medium text-text">{t('resources.bulk.count', { count: rows.length })}</span>
      {shared.map((act) => (
        <Button
          admin={admin}
          key={act.id}
          size="sm"
          variant={act.danger === true ? 'danger-ghost' : 'secondary'}
          icon={act.icon}
          pending={working === act.id}
          blocked={working !== null && working !== act.id}
          onClick={() => act.confirm === undefined ? void run(act) : setAsking(act)}
        >
          {act.label}
        </Button>
      ))}
      <Button size="sm" variant="ghost" disabled={working !== null} onClick={onClear}>
        {t('resources.bulk.clear')}
      </Button>
      {asking !== null && ask !== null && ask !== undefined && (
        <ConfirmDialog
          open
          onOpenChange={(next) => !next && working === null && setAsking(null)}
          title={ask.title}
          confirmLabel={ask.label}
          danger={asking.danger === true}
          pending={working !== null}
          error={error}
          onConfirm={() => void run(asking)}
        >
          {ask.body}
          {permission !== undefined && rows.map((row, index) => <AdminActNotice key={index} spec={adminSpec(row, permission)} />)}
        </ConfirmDialog>
      )}
    </div>
  );
}

/** The names of what an act is about to be done to, under one sentence that says what it does. */
export function BulkNames({ names, lead }: { names: readonly string[]; lead: string }) {
  return (
    <span className="flex flex-col gap-2">
      <span>{lead}</span>
      <ul className="list-disc pl-6 text-text">
        {names.map((name, index) => (
          // What is acted on is named in the order the list shows it, and never reordered while the question is open.
          <li key={index} className="wrap-anywhere">
            {name}
          </li>
        ))}
      </ul>
    </span>
  );
}
