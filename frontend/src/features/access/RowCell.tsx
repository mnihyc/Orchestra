/** One cell of the full form: a row of the page with whom it names and whom it follows from a place, the lock that decides it, and the acts on it.
 *
 * A cell shows the holders its row names itself and those it follows from the place above (quietly,
 * with an arrow whose words name the place). A row that a lock above decides shows the lock and has no
 * menu. Whoever may change the row can name a holder in it, stop it following its place (after being
 * told who loses what), make it follow again, and on a defaults page lock it for everything inside.
 */
import { useQuery } from '@tanstack/react-query';
import { ArrowUp, Ellipsis, Plus } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { type AccessRowTarget, permissionsQuery } from '@/api/access';
import { choiceOption } from '@/i18n/choices';
import { meQuery } from '@/api/auth';
import type { Holder, NodeOrSystem, PermissionRow, RowProfile } from '@/generated/api-types';
import { Combobox } from '@/ui/Combobox';
import { DateField } from '@/ui/DateField';
import { IconButton } from '@/ui/IconButton';
import { LockBadge } from '@/ui/LockBadge';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { Popover } from '@/ui/Popover';
import { Select } from '@/ui/Select';
import { Tooltip } from '@/ui/Tooltip';

import { useCandidates, useTargetOrg } from './candidates';
import type { Asked } from './ChangePreview';
import type { Changes, PageSource } from './changes';
import { gaveText, HolderView, holderName, placeName } from './holders';
import { useAppearance } from '@/lib/appearance';
import { addToRowChange, type Cell, cellHolders, followChange, forkRowChange, lockChange, removeChange } from './model';
import { permissionNameOf } from './names';
import { EntryOptions, optionFields } from './EntryOptions';

export interface RowCellProps {
  cell: Cell;
  /** What the page is of. */
  target: AccessRowTarget;
  source: PageSource;
  changes: Changes;
  /** Asks the person to confirm an act that takes something away. */
  ask: (asked: Asked) => void;
  /** The place whose defaults this row follows, by name. */
  place: NodeOrSystem | null;
  profile: RowProfile;
  /** For whoever may change the lock above: goes to the place that set it. */
  onOpenLock?: ((place: NodeOrSystem) => void) | undefined;
  /** Who can give the holders that this cell takes when it is the caller's to add outsiders: disables them with these words. */
  outsiderOff?: string | undefined;
  /** Added after the holders: a note on what the cell reaches. */
  aside?: ReactNode;
}

/** The arrow that marks a holder the row follows from a place, whose words name the place. */
export function FollowMark({ place }: { place: NodeOrSystem | null }) {
  const { t } = useTranslation();
  const words = place === null ? t('access.cell.followsDefaults') : t('access.holder.asDecides', { place: placeName(t, place) });
  return (
    <Tooltip content={words}>
      <span role="img" aria-label={words} className="inline-flex text-text-subtle">
        <ArrowUp size={16} aria-hidden />
      </span>
    </Tooltip>
  );
}

function Add({ cell, target, changes, outsiderOff, profile }: Pick<RowCellProps, 'cell' | 'target' | 'changes' | 'outsiderOff' | 'profile'>) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const orgId = useTargetOrg(target, open);
  const [query, setQuery] = useState('');
  const [until, setUntil] = useState('');
  const kinds = (['user', 'group', 'role', 'members', 'node'] as const).filter((kind) => cell.def.holders.includes(kind));
  const localRoles = target.kind === 'system' && kinds.includes('role');
  const catalogue = useQuery({ ...permissionsQuery, enabled: open && localRoles });
  const candidates = useCandidates(query, orgId, { enabled: open, kinds, thing: target.name, outsiderOff, placeId: ['org', 'project', 'topic', 'thread'].includes(target.kind) ? target.id : orgId ?? undefined });
  const row = cell.row;
  const label = permissionNameOf(cell.def);
  const chooseRole = (key: string) => {
    const role = catalogue.data?.roles.find((entry) => entry.key === key);
    if (role === undefined) return;
    const expires = until === '' ? null : new Date(`${until}T23:59:59`).toISOString();
    setOpen(false);
    void changes.run({ label: t('access.cell.adding', { name: choiceOption(t, `accessRole.${role.key}`).name, permission: label }),
      make: (page) => addToRowChange(page, row.permission, profile, { kind: 'role', role: role.key }, { expires_at: expires }) });
  };
  return (
    <Popover
      label={t('access.cell.addTo', { permission: label })}
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) {
          setQuery('');
          setUntil('');
        }
      }}
      align="end"
      className="w-(--menu-max-width)"
      trigger={<IconButton size="sm" label={t('access.cell.addTo', { permission: label })} icon={<Plus aria-hidden />} disabled={changes.busy} />}
    >
      <div className="flex flex-col gap-3">
        {localRoles && <Select label={t('adminDefaults.role')} value="" placeholder={t('adminDefaults.chooseRole')}
          options={(catalogue.data?.roles ?? []).map((entry) => ({ value: entry.key, ...choiceOption(t, `accessRole.${entry.key}`) }))}
          onValueChange={chooseRole} />}
        {candidates.placePicker}
        <Combobox
          label={t('access.cell.addTo', { permission: label })}
          placeholder={t('access.add.placeholder')}
          query={query}
          onQueryChange={setQuery}
          minChars={0}
          loading={candidates.loading}
          empty={t('access.candidates.none')}
          groups={candidates.groups}
          onSelect={(option) => {
            const found = candidates.byValue.get(option.value);
            if (found === undefined) return;
            const expires = until === '' ? null : new Date(`${until}T23:59:59`).toISOString();
            setOpen(false);
            void changes.run({
              label: t('access.cell.adding', { name: found.name, permission: label }),
              make: (page) => addToRowChange(page, row.permission, profile, found.input, { expires_at: expires }),
            });
          }}
        />
        <DateField label={t('access.cell.until')} optional value={until} onValueChange={setUntil} />
      </div>
    </Popover>
  );
}

export function RowCell({ cell, target, source, changes, ask, place, profile, onOpenLock, outsiderOff, aside }: RowCellProps) {
  const { t } = useTranslation();
  const { def, row } = cell;
  const label = permissionNameOf(def);
  const decidedAbove = row.locked !== null && !row.locked.here;
  const holders = cellHolders(row);
  const placeText = placeName(t, place);
  const canChange = row.writable && !decidedAbove;
  // Whoever shares without managing the row names holders in it up to what they hold, and takes back their own.
  const canAdd = row.can_give && !decidedAbove;
  const me = useQuery(meQuery).data;
  const locale = useAppearance((state) => state.locale);
  const mayTakeBack = (entry: { created_by: { id: string } | null }) => canChange || (canAdd && me !== undefined && entry.created_by?.id === me.id);
  const [editing, setEditing] = useState<{ id: string; holder: Holder; followed: boolean } | null>(null);
  const hasOptions = optionFields(def.options_schema).length > 0;

  const entries: MenuEntry[] = [];
  if (canChange && row.inherit && place !== null) {
    entries.push({
      id: 'cut',
      label: t('access.cell.stopFollowing', { place: placeText }),
      onSelect: () =>
        ask({
          title: t('access.cell.stopTitle', { permission: label }),
          body: t('access.cell.stopBody', { permission: label, place: placeText }),
          confirmLabel: t('access.cell.stopConfirm', { place: placeText }),
          danger: true,
          label: t('access.cell.stopping', { permission: label }),
          make: (page) => followChange(page, row.permission, profile, false),
        }),
    });
  }
  if (canChange && !row.inherit && place !== null) {
    entries.push({
      id: 'follow',
      label: t('access.cell.followAgain', { place: placeText }),
      onSelect: () => void changes.run({ label: t('access.cell.following', { permission: label }), make: (page) => followChange(page, row.permission, profile, true) }),
    });
  }
  if (canChange && source.type !== 'page') {
    entries.push(
      row.locked?.here === true
        ? {
            id: 'unlock',
            label: t('access.cell.unlock'),
            onSelect: () => void changes.run({ label: t('access.cell.unlocking', { permission: label }), make: (page) => lockChange(page, row.permission, profile, false) }),
          }
        : {
            id: 'lock',
            label: t('access.cell.lock'),
            description: t('access.cell.lockAbout'),
            onSelect: () =>
              ask({
                title: t('access.cell.lockTitle', { permission: label }),
                body: t('access.cell.lockBody'),
                confirmLabel: t('access.cell.lockConfirm'),
                danger: true,
                label: t('access.cell.locking', { permission: label }),
                make: (page) => lockChange(page, row.permission, profile, true),
              }),
          },
    );
  }

  // What a named holder carries besides the row it is in: an end, and on a resource options that narrow what they get.
  if (canChange) {
    for (const holder of holders) {
      const entry = holder.entry;
      const name = holderName(t, holder.holder, target.name);
      const id = entry?.id ?? `followed-${holder.key}`;
      entries.push({ id: `options-${id}`, label: t(hasOptions ? 'access.cell.optionsOf' : 'access.cell.endOf', { name }), onSelect: () => setEditing({ id, holder: holder.holder, followed: entry === null }) });
    }
  }

  return (
    <div data-access-permission={row.permission} tabIndex={-1} className="flex min-w-0 flex-wrap items-center gap-1" role="group" aria-label={label}>
      {holders.map((holder) => (
        <HolderView
          key={`${holder.key}:${holder.entry?.id ?? 'follows'}`}
          holder={holder.holder}
          target={target}
          inherited={holder.entry === null}
          outside={holder.entry?.outside ?? false}
          compact
          note={
            holder.entry === null ? (
              <FollowMark place={holder.from} />
            ) : (
              <span className="text-text-muted">
                {[gaveText(t, locale, holder.entry), holder.entry.expires_at === null ? null : t('access.holder.until', { date: new Date(holder.entry.expires_at).toLocaleDateString(locale) })]
                  .filter((text) => text !== null)
                  .join(t('startFlow.separator'))}
              </span>
            )
          }
          remove={
            canChange || holder.entry !== null && mayTakeBack(holder.entry)
              ? {
                  label: t('access.cell.remove', { name: holderName(t, holder.holder, target.name), permission: label }),
                  disabled: changes.busy,
                  onRemove: () => {
                    const entry = holder.entry;
                    ask({
                      title: t('access.cell.removeTitle', { name: holderName(t, holder.holder, target.name) }),
                      body: t('access.cell.removeBody', { name: holderName(t, holder.holder, target.name), permission: label }),
                      confirmLabel: t('access.cell.removeConfirm', { name: holderName(t, holder.holder, target.name) }),
                      danger: true,
                      label: t('access.cell.removing', { name: holderName(t, holder.holder, target.name), permission: label }),
                      make: (page) => entry === null ? {
                        expected_revision: page.revision,
                        rows: [forkRowChange(page.rows.find((candidate) => candidate.permission === row.permission && candidate.profile === profile) ?? row, new Set([holder.key]))],
                      } : removeChange(page, [{ permission: row.permission, entry, profile }]),
                    });
                  },
                }
              : undefined
          }
        />
      ))}
      {holders.length === 0 && (
        <span className="text-sm text-text-muted">{row.inherit ? <span aria-label={t('access.cell.nobody')}>—</span> : t('access.cell.ownersOnly')}</span>
      )}
      {decidedAbove && row.locked !== null && (
        <LockBadge onOpen={onOpenLock === undefined ? undefined : () => onOpenLock(row.locked?.by ?? 'system')}>{t('access.cell.locked', { place: placeName(t, row.locked.by) })}</LockBadge>
      )}
      {row.locked?.here === true && <LockBadge>{t('access.cell.lockedHere')}</LockBadge>}
      {aside}
      {editing !== null && (
        <EntryOptions
          key={editing.id}
          open
          onClose={() => setEditing(null)}
          def={def}
          row={row}
          entryId={editing.id}
          holder={holderName(t, editing.holder, target.name)}
          {...(editing.followed ? { followedHolder: editing.holder } : {})}
          changes={changes}
        />
      )}
      {(canChange || canAdd) && (
        <span className="flex shrink-0 items-center">
          {canAdd && <Add cell={cell} target={target} changes={changes} outsiderOff={outsiderOff} profile={profile} />}
          {entries.length > 0 && (
            <Menu
              label={t('access.cell.menuOf', { permission: label })}
              align="end"
              entries={entries}
              trigger={<IconButton size="sm" label={t('access.cell.menuOf', { permission: label })} icon={<Ellipsis aria-hidden />} disabled={changes.busy} />}
            />
          )}
        </span>
      )}
    </div>
  );
}

/** Whether a row follows a place that names anybody, for the words that say what following amounts to. */
export function followsSomebody(row: PermissionRow): boolean {
  return row.inherit && row.inherited !== null && row.inherited.holders.length > 0;
}
