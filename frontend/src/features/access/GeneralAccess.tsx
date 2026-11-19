/** One general audience choice, using the same catalogue and rows as the three-line form. */
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';
import type { AccessChoice } from '@/generated/api-types';

import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Select, type SelectOption } from '@/ui/Select';

import { type Catalogue, type GeneralAccess, type GeneralAccessPreset, middleOf } from './model';

function accessVerb(catalogue: Catalogue, kind: string, preset: 'view' | 'interact' | 'manage') {
  const middle = catalogue.choice('full_interact', kind).map((name) => catalogue.defs.get(name)?.level);
  const level = middle.includes('use') ? 'use' : middle.includes('interact') ? 'interact' : middle.includes('edit') ? 'edit' : middleOf(kind);
  return kind === 'chat' && preset !== 'manage' ? preset === 'view' ? 'read' : 'write' : preset === 'interact' ? level : preset;
}

function accessTerm(catalogue: Catalogue, kind: string, preset: GeneralAccess) {
  return preset === 'defaults' || preset === 'private' || preset === 'custom' ? preset : accessVerb(catalogue, kind, preset);
}

export function generalAccessWords(t: TFunction, catalogue: Catalogue, kind: string, place: string, preset: GeneralAccess): string {
  if (preset === 'defaults') return t('access.general.defaults', { place });
  if (preset === 'private') return t('access.general.private');
  if (preset === 'custom') return t('access.general.custom');
  const verb = accessVerb(catalogue, kind, preset);
  return t(kind === 'org' ? 'access.general.members' : 'access.general.everyone', { place, verb: t(`access.general.verb.${verb}`) });
}

interface GeneralAccessProps {
  catalogue: Catalogue;
  kind: string;
  place: string;
  /** An org names its own members, while its defaults still come from its parent or the system. */
  rulesPlace?: string;
  value: GeneralAccess;
  onChange: (preset: GeneralAccessPreset) => void;
  disabled?: boolean;
  /** A page with no visible place still offers Private and its existing defaults, but cannot invent an audience. */
  hasAudience?: boolean;
  canGive?: readonly AccessChoice[];
  summary?: string;
  onReset?: () => void;
  environment?: { checked: boolean; onChange: (checked: boolean) => void; disabled?: boolean };
}

export function GeneralAccessControl({ catalogue, kind, place, rulesPlace = place, value, onChange, disabled = false, hasAudience = true, canGive, summary, onReset, environment }: GeneralAccessProps) {
  const { t } = useTranslation();
  const options: SelectOption<GeneralAccess>[] = (['defaults', 'view', 'interact', 'manage', 'private'] as const).map((preset) => ({
    value: preset,
    ...choiceOption(t, `generalAccess.${accessTerm(catalogue, kind, preset)}`, { place }),
    description: generalAccessWords(t, catalogue, kind, preset === 'defaults' ? rulesPlace : place, preset),
    ...(preset === 'interact' && catalogue.choice('full_interact', kind).length === 0 ? { description: t('access.general.noMiddle') } : {}),
    disabled: (preset === 'view' || preset === 'interact' || preset === 'manage') && (!hasAudience || catalogue.choice(preset === 'view' ? 'full_view' : preset === 'manage' ? 'full_manage' : 'full_interact', kind).length === 0 || canGive !== undefined && !canGive.includes(preset === 'view' ? 'full_view' : preset === 'manage' ? 'full_manage' : 'full_interact')),
  }));
  if (value === 'custom') options.push({ value: 'custom', ...choiceOption(t, 'generalAccess.custom'), disabled: true });
  return (
    <div className="flex flex-col gap-2">
      <Select<GeneralAccess>
        label={t('access.general.label')}
        value={value}
        options={options}
        disabled={disabled}
        onValueChange={(next) => { if (next !== 'custom') onChange(next); }}
        aside={value === 'custom' && onReset !== undefined ? <Button size="sm" variant="ghost" disabled={disabled} onClick={onReset}>{t('access.general.back', { place: rulesPlace })}</Button> : undefined}
      />
      {value === 'defaults' && summary !== undefined && <p className="text-sm text-text-muted">{summary}</p>}
      {environment !== undefined && <Checkbox label={t('access.general.letIn')} checked={environment.checked} onCheckedChange={environment.onChange} disabled={environment.disabled ?? disabled} />}
    </div>
  );
}
