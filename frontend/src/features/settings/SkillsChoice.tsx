/** Which skills a place gives its threads: each skill named here off, at its latest version or at a pinned one; a skill is added from those the person may switch on. */
import { useQuery } from '@tanstack/react-query';
import { X } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { skillVersionsQuery, skillsQuery } from '@/api/skills';
import { Hint } from '@/features/help/Hint';
import type { ValueControlProps } from '@/features/settings/ValueControl';
import type { Skill } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Select } from '@/ui/Select';
import { ResourceChip, resourceChoice } from '@/features/resource-facts/ResourceFacts';

export const SKILLS_SETTING = 'agents.skills';

/** A choice of one skill: off, latest, or a version's identifier. */
type Choice = string;
const OFF = 'off';
const LATEST = 'latest';

/** The members of a value of `agents.skills` that read as choices, in the order they were written. */
export function skillChoices(value: unknown): [string, Choice][] {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return [];
  return Object.entries(value).flatMap(([skill, choice]) => typeof choice === 'string' ? [[skill, choice] as [string, Choice]] : []);
}

function ChoiceRow({ skill, entry, choice, name, disabled, label, onChoose, onRemove }: {
  skill: string; entry: Skill | undefined; choice: Choice; name: string; disabled: boolean; label: string; onChoose: (choice: Choice) => void; onRemove: () => void;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const pinned = choice !== OFF && choice !== LATEST;
  const versions = useQuery({ ...skillVersionsQuery(skill), enabled: open || pinned });
  const options = [
    { value: OFF, label: t('settings.skills.off') },
    { value: LATEST, label: t('settings.skills.latest') },
    ...(versions.data?.items ?? []).map((version) => ({ value: version.id, label: t('resourceDetails.skill.version', { number: version.number }) })),
    ...(pinned && versions.data?.items.some((version) => version.id === choice) !== true ? [{ value: choice, label: t('settings.skills.pinned') }] : []),
  ];
  return <li className="flex min-w-0 flex-wrap items-center gap-2">
    <span className="min-w-0 flex-1">{entry === undefined ? name : <ResourceChip subject={{ kind: 'skill', skill: entry }} hint={false} />}</span>
    <Select size="sm" label={`${label}: ${name}`} hideLabel value={choice} disabled={disabled} open={open} onOpenChange={setOpen} options={options} onValueChange={onChoose} />
    <IconButton size="sm" label={t('settings.skills.remove', { name })} icon={<X aria-hidden />} disabled={disabled} onClick={onRemove} />
  </li>;
}

export function SkillsSettingChoice({ value, label, onChange, onEdited, disabled = false, error }: ValueControlProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const chosen = skillChoices(value);
  const skills = useQuery({ ...skillsQuery, enabled: open || chosen.length > 0 });
  const byId = new Map<string, Skill>((skills.data ?? []).map((skill) => [skill.id, skill]));
  const write = (next: [string, Choice][]) => { onEdited?.(); onChange(Object.fromEntries(next)); };
  const offered = (skills.data ?? []).filter((skill) => holds(skill.access, 'skill.use') && !chosen.some(([id]) => id === skill.id));
  return <div className="flex min-w-0 flex-col gap-2">
    <Hint id="skills" popover className="self-start" />
    {chosen.length > 0 && <ul aria-label={label} className="flex flex-col gap-2">
      {chosen.map(([skill, choice]) => <ChoiceRow key={skill} skill={skill} entry={byId.get(skill)} choice={choice} label={label} disabled={disabled}
        name={byId.get(skill)?.name ?? t(skills.isPending ? 'common.loading' : 'settings.resourceChoice.unavailable')}
        onChoose={(next) => write(chosen.map(([id, old]) => [id, id === skill ? next : old]))}
        onRemove={() => write(chosen.filter(([id]) => id !== skill))} />)}
    </ul>}
    <Select size="sm" label={t('settings.skills.add')} hideLabel value="" open={open} onOpenChange={setOpen} disabled={disabled} error={error}
      placeholder={t(open && skills.isPending ? 'common.loading' : 'settings.skills.add')}
      options={offered.map((skill) => ({ value: skill.id, label: skill.name, ...resourceChoice({ kind: 'skill', skill }) }))}
      onValueChange={(skill) => write([...chosen, [skill, LATEST]])} />
    {skills.isError && <ErrorState compact error={skills.error} retrying={skills.isRefetching} onRetry={() => void skills.refetch()} />}
  </div>;
}

/** How many skills a value switches on and off, for a compact row. */
export function SkillsSummary({ value }: { value: unknown }) {
  const { t } = useTranslation();
  const chosen = skillChoices(value);
  if (chosen.length === 0) return <span>{t('screenReview.emptyObject')}</span>;
  const off = chosen.filter(([, choice]) => choice === OFF).length;
  const on = chosen.length - off;
  return <span className="inline-flex flex-wrap gap-x-3 gap-y-1">
    {on > 0 && <span>{t('settings.compact.enabledCount', { count: on })}</span>}
    {off > 0 && <span>{t('settings.compact.disabledCount', { count: off })}</span>}
  </span>;
}
