/** The groups of a provider's models: chips on a model's row editor, added by typing with the provider's other groups offered, and the strip above the list with how many models each group has and "Limit this group…".
 *
 * A group is a name of the managers' own for several of a provider's models ("frontier", "small"): a limit,
 * an allowlist or somebody's access can name the group instead of each model. A model is in at most eight,
 * each named by letters a to z, digits, `-` and `_`.
 */
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { groupPattern, GROUPS_MOST } from '@/features/models/patterns';
import { groupFault } from '@/features/resources/providers/entry';
import { AddLimit } from '@/features/resources/providers/Caps';
import type { ProviderModel, ProviderRef } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ActionIcon } from '@/ui/actions';
import { Input } from '@/ui/Input';

/** The groups a provider's models are in, with how many models each has, the largest first and then by name. */
export function groupsOf(models: readonly Pick<ProviderModel, 'groups'>[]): { name: string; models: number }[] {
  const counts = new Map<string, number>();
  for (const model of models) for (const name of model.groups) counts.set(name, (counts.get(name) ?? 0) + 1);
  return [...counts]
    .map(([name, count]) => ({ name, models: count }))
    .sort((one, other) => other.models - one.models || one.name.localeCompare(other.name));
}

/** A group's name as a chip, on the row of a model or in a list. */
export function GroupChip({ name, onRemove }: { name: string; onRemove?: () => void }) {
  const { t } = useTranslation();
  return (
    <span className="inline-flex h-5 max-w-full shrink-0 items-center gap-1 rounded-full bg-bg-muted px-2 text-xs font-medium text-text">
      <span className="truncate">{name}</span>
      {onRemove !== undefined && (
        <button
          type="button"
          aria-label={t('models.groups.remove', { name })}
          title={t('models.groups.remove', { name })}
          className="focus-inset -mr-1 inline-flex size-4 items-center justify-center rounded-full transition-colors hover:bg-border"
          onClick={onRemove}
        >
          <ActionIcon action="delete" className="size-4" />
        </button>
      )}
    </span>
  );
}

/** The groups of one model: its chips, a field to put it in another by typing, and the provider's other groups to take from. */
export function GroupsField({ groups, offered, onChange, error, disabled }: { groups: string[]; offered: readonly string[]; onChange: (groups: string[]) => void; error?: string | undefined; disabled: boolean }) {
  const { t } = useTranslation();
  const [typed, setTyped] = useState('');
  const [fault, setFault] = useState<string | null>(null);
  const others = offered.filter((name) => !groups.includes(name));
  const full = groups.length >= GROUPS_MOST;

  const put = (name: string) => {
    const clean = name.trim();
    if (clean === '') return;
    const wrong = groupFault(clean, t);
    if (wrong !== null) {
      setFault(wrong);
      return;
    }
    setFault(null);
    setTyped('');
    if (!groups.includes(clean)) onChange([...groups, clean]);
  };

  return (
    <div className="flex flex-col gap-3">
      <ul aria-label={t('models.groups.of')} className="flex min-h-5 flex-wrap items-center gap-2">
        {groups.length === 0 && <li className="text-sm text-text-muted">{t('models.groups.none')}</li>}
        {groups.map((name) => (
          <li key={name}>
            <GroupChip name={name} {...(disabled ? {} : { onRemove: () => onChange(groups.filter((other) => other !== name)) })} />
          </li>
        ))}
      </ul>
      {!disabled && (
        <div className="flex flex-wrap items-start gap-2">
          <Input
            label={t('models.groups.add')}
            hint={full ? t('models.groups.full', { most: GROUPS_MOST }) : t('models.groups.addHint')}
            error={fault ?? error}
            value={typed}
            disabled={full}
            fieldClassName="min-w-0 flex-1 basis-[12rem]"
            onChange={(event) => {
              setTyped(event.target.value);
              setFault(null);
            }}
            // Enter puts the model in the group; it does not send the form of the dialog this stands in.
            onKeyDown={(event) => {
              if (event.key !== 'Enter') return;
              event.preventDefault();
              put(typed);
            }}
            autoComplete="off"
          />
          <Button size="sm" action="create" disabled={full || typed.trim() === ''} className="mt-6" onClick={() => put(typed)}>
            {t('models.groups.put')}
          </Button>
        </div>
      )}
      {disabled && error !== undefined && <p className="text-sm text-danger-text">{error}</p>}
      {!disabled && others.length > 0 && !full && (
        <div className="flex flex-wrap items-center gap-2" role="group" aria-label={t('models.groups.others')}>
          <span className="text-sm text-text-muted">{t('models.groups.othersLead')}</span>
          {others.map((name) => (
            <Button key={name} size="sm" variant="ghost" action="create" onClick={() => put(name)}>
              {name}
            </Button>
          ))}
        </div>
      )}
      <p className="text-sm text-text-muted">{t('models.groups.about')}</p>
    </div>
  );
}

/** The provider's groups above its list of models, each with how many models are in it and the means to limit the group. */
export function GroupStrip({ provider, models, manage }: { provider: ProviderRef; models: readonly ProviderModel[]; manage: boolean }) {
  const { t } = useTranslation();
  const groups = groupsOf(models);
  // A provider with no model has nothing to put in a group; one the reader may not manage says nothing of groups that are not there.
  if (models.length === 0 || (groups.length === 0 && !manage)) return null;
  return (
    <section aria-label={t('models.groups.title')} className="flex flex-col gap-2 rounded-lg border border-border bg-surface p-3">
      <div className="flex flex-col gap-1">
        <h3 className="text-sm font-medium text-text">{t('models.groups.title')}</h3>
        <p className="text-sm text-text-muted">{groups.length === 0 ? t('models.groups.noneYet') : t('models.groups.about')}</p>
      </div>
      {groups.length > 0 && (
        <ul className="flex flex-col divide-y divide-border">
          {groups.map((group) => (
            <li key={group.name} className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1 py-2 first:pt-0 last:pb-0">
              <span className="flex items-center gap-2">
                <GroupChip name={group.name} />
                <span className="text-sm text-text-muted">{t('models.groups.count', { count: group.models })}</span>
              </span>
              {manage && <AddLimit provider={provider} models={[groupPattern(group.name)]} label={t('models.groups.limit')} />}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
