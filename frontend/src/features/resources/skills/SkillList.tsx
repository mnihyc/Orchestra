/** Skills listed with readable records and private permission-page names: each with its latest version and its home. */
import { Link, useNavigate } from '@tanstack/react-router';
import { ArrowRight, GraduationCap, Plus } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { resourceReaders } from '@/features/resources/inventory';
import { Hint } from '@/features/help/Hint';
import type { Skill } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { entriesOf, shareAct, type Act, type RowActs } from '@/features/resources/acts';
import { ResourceList } from '@/features/resources/ResourceList';
import type { Column } from '@/ui/Table';
import { Tooltip } from '@/ui/Tooltip';
import { UserChip } from '@/ui/UserChip';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { AddSkill } from './SkillDialog';

export function SkillList() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [text, setText] = useState('');
  const [adding, setAdding] = useState(false);
  const title = t('screens.skills.title');
  const actsFor = (row: Skill): RowActs => {
    const open: Act = { id: 'open', label: t('resources.act.open'), icon: <ArrowRight aria-hidden />, run: () => void navigate({ to: '/resources/skills/$id', params: { id: row.id } }) };
    return { busy: false, primary: open, keys: {}, entries: entriesOf([shareAct(t, { kind: 'skill', id: row.id, name: row.name }, row.access)]) };
  };
  const columns: Column<Skill>[] = [
    { id: 'name', header: t('resources.listName'), cell: (row) => <div className="flex min-w-0 flex-col gap-1">
      <ResourceChip subject={{ kind: 'skill', skill: row }} asChild hint={false}><Link to="/resources/skills/$id" params={{ id: row.id }} className="focus-inset min-w-0 truncate font-medium text-text hover:underline">{row.name}</Link></ResourceChip>
      {row.description !== '' && <Tooltip content={row.description}><span tabIndex={0} className="block min-w-0 truncate rounded-sm text-sm text-text-muted">{row.description}</span></Tooltip>}
    </div> },
    { id: 'version', header: t('resourceDetails.skill.latest'), from: 'sm', cell: (row) => row.latest === null ? t('resourceDetails.skill.noVersion') : t('resourceDetails.skill.version', { number: row.latest.number }) },
    { id: 'home', header: t('resources.home.label'), from: 'lg', cell: (row) => row.home.name ?? '' },
    { id: 'owner', header: t('resources.listOwner'), from: 'lg', cell: (row) => row.owner ? <UserChip user={row.owner} thing={{ kind: 'skill', id: row.id }} /> : null },
  ];
  const add = <Button variant="primary" icon={<Plus aria-hidden />} onClick={() => setAdding(true)}>{t('resourceDetails.skill.add')}</Button>;
  return <>
    <ResourceList<Skill>
      kind="skills"
      search={{}}
      title={title}
      icon={GraduationCap}
      lead={t('resourceDetails.skill.list.lead')}
      label={title}
      readRow={resourceReaders.skills}
      columns={columns}
      rowKey={(row) => row.id}
      text={text}
      onText={setText}
      searchLabel={t('resourceDetails.skill.list.search')}
      searchPlaceholder={t('resourceDetails.skill.list.search')}
      action={<><Hint id="skills" popover />{add}</>}
      empty={{ icon: <GraduationCap aria-hidden />, title: t('resourceDetails.skill.list.none'), description: t('resourceDetails.skill.list.noneLead'), action: add }}
      noMatch={t('resourceDetails.skill.list.noMatch')}
      acts={{ name: (row) => row.name, of: actsFor }}
    />
    {adding && <AddSkill onClose={() => setAdding(false)} onCreated={(row) => void navigate({ to: '/resources/skills/$id', params: { id: row.id } })} />}
  </>;
}
