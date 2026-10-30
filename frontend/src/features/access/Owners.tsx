/** The Owners line of a page: who owns the thing, Add owner, Make first, taking oneself out, and the last owner who cannot.
 *
 * It is the first line of a page and a row like the others, with one difference: it follows no default,
 * so it has no "as … decides" and is touched neither by Private nor by Reset. The owners are people and
 * groups of the org; each holds everything on the page, and the one marked first is who the thing's
 * address names. Whoever may not change the row reads it. The last person among the owners is never
 * taken out: somebody else is named first.
 */
import { useQuery } from '@tanstack/react-query';
import { ChevronDown, Crown, Plus, UserMinus } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { AccessTarget } from '@/api/access';
import { meQuery } from '@/api/auth';
import type { Entry, PermissionPage } from '@/generated/api-types';
import { Avatar } from '@/ui/Avatar';
import { Button } from '@/ui/Button';
import { Combobox } from '@/ui/Combobox';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { cx } from '@/ui/cx';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { Popover } from '@/ui/Popover';

import { useCandidates, useTargetOrg } from './candidates';
import type { Changes } from './changes';
import { HolderView, holderName } from './holders';
import { isFirstOwner, mayTakeOutOwner, ownersChange } from './model';

/** The field that names an owner: a person or a group of the org. */
function AddOwner({ page, target, changes }: { page: PermissionPage; target: AccessTarget; changes: Changes }) {
  const { t } = useTranslation();
  const orgId = useTargetOrg(target);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const candidates = useCandidates(query, orgId, { kinds: ['user', 'group'], thing: target.name, enabled: open });
  const taken = new Set(page.owners.map((entry) => (entry.holder.kind === 'user' ? `user:${entry.holder.user.id}` : entry.holder.kind === 'group' ? `group:${entry.holder.group.id}` : '')));

  return (
    <Popover
      label={t('access.owners.addLabel')}
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) setQuery('');
      }}
      align="end"
      className="w-(--menu-max-width)"
      trigger={
        <Button size="sm" icon={<Plus aria-hidden />} blocked={changes.busy}>
          {t('access.owners.add')}
        </Button>
      }
    >
      {candidates.placePicker}
      <Combobox
        label={t('access.owners.addLabel')}
        hint={t('access.owners.addHint')}
        placeholder={t('access.owners.addPlaceholder')}
        query={query}
        onQueryChange={setQuery}
        minChars={2}
        loading={candidates.loading}
        empty={t('access.candidates.none')}
        groups={candidates.groups.map((group) => ({ ...group, options: group.options.filter((option) => !taken.has(option.value)) }))}
        onSelect={(option) => {
          const found = candidates.byValue.get(option.value);
          if (found === undefined) return;
          setOpen(false);
          setQuery('');
          void changes.run({
            label: t('access.owners.adding', { name: found.name }),
            make: (current) => ownersChange(current, { add: [found.input] }),
          });
        }}
      />
    </Popover>
  );
}

/** One owner: a chip that opens its menu when the caller may change the row. */
function Owner({ entry, page, target, changes, onLeave }: { entry: Entry; page: PermissionPage; target: AccessTarget; changes: Changes; onLeave: (entry: Entry) => void }) {
  const { t } = useTranslation();
  const first = isFirstOwner(page, entry);
  const name = holderName(t, entry.holder, target.name);
  const firstNote = first ? <span className="text-text-muted">{t('access.roster.primaryOwner')}</span> : null;

  if (!page.owners_writable) {
    return <HolderView holder={entry.holder} target={target} note={firstNote} outside={entry.outside} />;
  }

  const onlyPerson = !mayTakeOutOwner(page, entry);
  const entries: MenuEntry[] = [];
  if (entry.holder.kind === 'user' && !first) {
    const userId = entry.holder.user.id;
    entries.push({
      id: 'first',
      label: t('access.owners.makeFirst'),
      icon: <Crown aria-hidden />,
      onSelect: () => void changes.run({ label: t('access.owners.makingFirst', { name }), make: (current) => ownersChange(current, { first: userId }) }),
    });
  }
  entries.push({
    id: 'remove',
    label: t('access.owners.remove'),
    icon: <UserMinus aria-hidden />,
    danger: true,
    disabled: onlyPerson ? t('access.owners.onlyOne') : false,
    onSelect: () => onLeave(entry),
  });

  const pending = changes.busy;
  return (
    <Menu
      label={t('access.owners.menuOf', { name })}
      entries={entries}
      trigger={
        <button
          type="button"
          aria-label={t('access.owners.menuOf', { name })}
          className={cx(
            'inline-flex min-h-(--control-sm) max-w-full min-w-0 items-center gap-1 rounded-full border border-border-strong bg-surface py-0.5 pr-2 text-sm text-text transition-colors hover:border-text-subtle',
            entry.holder.kind === 'user' ? 'pl-1' : 'pl-2',
            pending && 'opacity-60',
          )}
        >
          {entry.holder.kind === 'user' && (
            <Avatar text={entry.holder.user.avatar.text} color={entry.holder.user.avatar.color} kind={entry.holder.user.kind === 'service' ? 'service' : 'human'} size={24} />
          )}
          <span className="min-w-0 font-medium wrap-anywhere">{name}</span>
          {firstNote}
          <ChevronDown className="shrink-0 text-text-subtle" size={16} aria-hidden />
        </button>
      }
    />
  );
}

export function Owners({ page, target, changes }: { page: PermissionPage; target: AccessTarget; changes: Changes }) {
  const { t } = useTranslation();
  const me = useQuery(meQuery).data;
  const [leaving, setLeaving] = useState<Entry | null>(null);
  const yourself = leaving?.holder.kind === 'user' && leaving.holder.user.id === me?.id;

  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-2" role="group" aria-label={t('access.owners.label')}>
      <span className="w-[5rem] shrink-0 text-sm font-medium text-text">{t('access.owners.label')}</span>
      <div className="flex min-w-0 flex-1 basis-[min(100%,12rem)] flex-wrap items-center gap-2">
        {page.owners.map((entry) => (
          <Owner key={entry.id} entry={entry} page={page} target={target} changes={changes} onLeave={setLeaving} />
        ))}
      </div>
      {page.owners_writable && <AddOwner page={page} target={target} changes={changes} />}
      <ConfirmDialog
        open={leaving !== null}
        onOpenChange={(next) => !next && setLeaving(null)}
        title={yourself ? t('access.owners.leaveTitle', { name: target.name }) : t('access.owners.removeTitle', { name: leaving === null ? '' : holderName(t, leaving.holder, target.name) })}
        confirmLabel={yourself ? t('access.owners.leaveConfirm') : t('access.owners.removeConfirm')}
        danger
        pending={changes.busy}
        error={changes.error}
        onConfirm={() => {
          if (leaving === null) return;
          const gone = leaving;
          void changes
            .run({
              label: t('access.owners.removing', { name: holderName(t, gone.holder, target.name) }),
              make: (current) => ownersChange(current, { remove: [gone.id] }),
            })
            .then((made) => made && setLeaving(null));
        }}
      >
        {yourself ? t('access.owners.leaveBody') : t('access.owners.removeBody')}
      </ConfirmDialog>
    </div>
  );
}
