/** The control that ends every dialog that makes a resource: only me, everyone in the org, chosen people, or as the org decides.
 *
 * Making a resource asks first where it belongs; this is the one question that ends the dialog: who
 * may use it. "Only me" cuts the new page loose so that its owner holds it alone, "Everyone in <home>"
 * names the people of its home org, "Choose…" names the people, groups and roles picked here, and
 * "As <home> decides" is the only one that keeps following the home's defaults, with what that comes
 * to now written beside it. The control only says what was chosen: the dialog makes the change
 * (`whoMayUseChange`) once the resource exists.
 */
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import type { AccessTarget } from '@/api/access';
import type { PageChange } from '@/generated/api-types';
import { Combobox } from '@/ui/Combobox';
import { RadioGroup, type RadioOption } from '@/ui/RadioGroup';
import { ChoiceFieldHelp } from '@/ui/ChoiceField';

import { type Candidate, useCandidates } from './candidates';
import { HolderView } from './holders';

export type WhoMayUseMode = 'owner' | 'home' | 'chosen' | 'defaults';

export interface WhoMayUseValue {
  mode: WhoMayUseMode;
  /** The people, groups and roles picked, for "Choose…". */
  holders: readonly Candidate[];
}

/** The change that makes the new page say what was chosen, or `null` where it should simply follow its home. */
export function whoMayUseChange(value: WhoMayUseValue, home: { id: string }): PageChange | null {
  switch (value.mode) {
    case 'owner':
      return { whole: 'private' };
    case 'home':
      return { name: [{ choice: 'full_interact', holders: [{ kind: 'role', role: 'viewer', at: home.id }] }] };
    case 'chosen':
      return value.holders.length === 0 ? { whole: 'private' } : { name: [{ choice: 'full_interact', holders: value.holders.map((entry) => entry.input) }] };
    case 'defaults':
      return null;
  }
}

export interface WhoMayUseProps {
  /** The org the resource is made in: its home. */
  home: { id: string; name: string };
  value: WhoMayUseValue;
  onChange: (value: WhoMayUseValue) => void;
  /** What following the home comes to now, in words: "only its owner". */
  defaultsNow: string;
  /** Why the choice cannot be made, where it cannot. */
  disabled?: string | undefined;
}

export function WhoMayUse({ home, value, onChange, defaultsNow, disabled }: WhoMayUseProps) {
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const place: AccessTarget = { kind: 'org', id: home.id, name: home.name, orgId: home.id };
  const candidates = useCandidates(query, home.id, { kinds: ['user', 'group', 'role', 'members', 'node'], thing: home.name });
  const taken = new Set(value.holders.map((entry) => JSON.stringify(entry.input)));
  const options: RadioOption<WhoMayUseMode>[] = [
    { value: 'owner', ...choiceOption(t, 'resourceAudience.owner') },
    { value: 'home', ...choiceOption(t, 'resourceAudience.home', { home: home.name }) },
    { value: 'chosen', ...choiceOption(t, 'resourceAudience.chosen') },
    { value: 'defaults', ...choiceOption(t, 'resourceAudience.defaults', { home: home.name, now: defaultsNow }) },
  ];

  return (
    <fieldset className="flex flex-col gap-3" disabled={disabled !== undefined}>
      <legend className="text-sm font-medium text-text"><span className="inline-flex items-center gap-1">{t('access.who.title')}<ChoiceFieldHelp label={t('access.who.title')} options={options} /></span></legend>
      <RadioGroup<WhoMayUseMode>
        label={t('access.who.title')}
        variant="list"
        value={value.mode}
        onValueChange={(mode) => onChange({ ...value, mode })}
        options={options}
      />
      {value.mode === 'chosen' && (
        <div className="flex flex-col gap-2 pl-1">
          {candidates.placePicker}
          <Combobox
            label={t('access.who.pick')}
            placeholder={t('access.add.placeholder')}
            query={query}
            onQueryChange={setQuery}
            minChars={0}
            loading={candidates.loading}
            empty={t('access.candidates.none')}
            groups={candidates.groups.map((group) => ({ ...group, options: group.options.filter((option) => { const found = candidates.byValue.get(option.value); return found === undefined || !taken.has(JSON.stringify(found.input)); }) }))}
            onSelect={(option) => {
              const found = candidates.byValue.get(option.value);
              if (found === undefined) return;
              onChange({ ...value, holders: [...value.holders, found] });
              setQuery('');
            }}
          />
          <ul className="flex flex-wrap items-center gap-2" aria-label={t('access.who.picked')}>
            {value.holders.map((entry) => (
              <li key={JSON.stringify(entry.input)}>
                <HolderView
                  holder={entry.holder}
                  target={place}
                  remove={{ label: t('access.add.unstage', { name: entry.name }), onRemove: () => onChange({ ...value, holders: value.holders.filter((other) => other !== entry) }) }}
                />
              </li>
            ))}
          </ul>
        </div>
      )}
      {disabled !== undefined && <p className="text-sm text-text-muted">{disabled}</p>}
    </fieldset>
  );
}
