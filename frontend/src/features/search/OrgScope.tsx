/** One address-backed root choice, shared by Search and the rail's content search. */
import { useTranslation } from 'react-i18next';
import type { Org } from '@/generated/api-types';
import { ChevronDown } from 'lucide-react';
import { Button } from '@/ui/Button';
import { Menu } from '@/ui/Menu';

export function OrgScope({ value, selected, orgs, onChange }: {
  value: string | undefined;
  selected: string | null;
  orgs: readonly Org[] | undefined;
  onChange: (org: string) => void;
}) {
  const { t } = useTranslation();
  const current = value === 'all' ? null : orgs?.find((org) => org.id === value);
  const root = orgs?.find((org) => org.id === selected) ?? orgs?.[0];
  const choices = [root, current].filter((org): org is Org => org !== undefined && org !== null)
    .filter((org, index, all) => all.findIndex((entry) => entry.id === org.id) === index);
  const options = choices.map((org) => ({ value: org.id, label: t('search.org.in', { org: org.name }) }));
  if ((orgs?.length ?? 0) > 1) options.push({ value: 'all', label: t('search.org.all') });
  const label = value === 'all' ? t('search.org.all') : current === undefined || current === null
    ? t(orgs === undefined ? 'common.loading' : 'search.org.unavailable') : t('search.org.in', { org: current.name });
  return <Menu label={t('search.org.label')} entries={[{ type: 'radio', id: 'search-org', label: t('search.org.label'), value: value ?? '', options, onValueChange: onChange }]}
    trigger={<Button variant="secondary" size="sm" className="max-w-full" aria-label={`${t('search.org.label')}: ${label}`}
      disabled={options.length === 0 || options.length === 1 && options[0]?.value === value} title={label}>
      <span className="min-w-0 truncate">{label}</span><ChevronDown aria-hidden className="size-4 shrink-0" />
    </Button>} />;
}
