/** The templates a person may use or manage, of both kinds: where each comes from, its driver, where its image comes from, how it confines, how many units were made from it and whether it is on, with the means to add one.
 */
import { Link } from '@tanstack/react-router';
import { TEMPLATE_KINDS, type TemplatesSearch } from '@/features/resources/search';
import { Cpu, Plus } from 'lucide-react';
import { useMemo, useState } from 'react';
import { useTranslation, Trans } from 'react-i18next';

import { useQuery } from '@tanstack/react-query';
import { hostsQuery } from '@/api/resources';
import { resourceReaders } from '@/features/resources/inventory';
import { classNamed, templateRuntime, useIsolationWords } from '@/features/resources/isolation/runtimes';
import { ResourceList, useSearchText } from '@/features/resources/ResourceList';
import { oneOf } from '@/features/resources/search';
import { useTemplateActs } from '@/features/resources/templates/acts';
import { NewTemplateDialog } from '@/features/resources/templates/NewTemplate';
import { confinementOf, imageSource } from '@/features/resources/templates/spec';
import type { RuntimeClass, Template, UnitKind } from '@/generated/api-types';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { SegmentedControl } from '@/ui/SegmentedControl';
import type { Column } from '@/ui/Table';
import { UserChip } from '@/ui/UserChip';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';

export { TEMPLATE_KINDS, type TemplatesSearch };
const FILTER_KINDS:readonly ('all'|UnitKind)[]=['all',...TEMPLATE_KINDS];

export function TemplateLink({ template }: { template: Pick<Template, 'id' | 'name'> & Partial<Template> }) {
  return (
    <ResourceChip subject={{ kind: 'template', template }} asChild hint={false}><Link to="/resources/templates/$id" params={{ id: template.id }} className="truncate font-medium text-text hover:underline">
      {template.name}
    </Link></ResourceChip>
  );
}

/** Where a template's image comes from, in words. */
export function useImageWords(): (spec: unknown) => string {
  const { t } = useTranslation();
  return (spec) => {
    const source = imageSource(spec);
    switch (source.kind) {
      case 'build':
        return t('templates.source.build');
      case 'archive':
        return t('templates.source.archive');
      case 'image':
        return t('templates.source.image', { reference: source.reference });
      case 'none':
        return t('templates.source.none');
    }
  };
}

/** Keep a registry reference visibly technical when its source column is collapsed. */
function ImageWords({ spec }: { spec: unknown }) {
  const source = imageSource(spec);
  return source.kind === 'image' ? <Trans i18nKey="templates.source.imageReference" values={{ reference: source.reference }} components={{ reference: <code /> }} /> : null;
}

/** How a template confines what runs in it, in a few words. */
export function useConfinementWords(): (spec: unknown) => string {
  const { t } = useTranslation();
  return (spec) => {
    const confinement = confinementOf(spec);
    const pieces: string[] = [];
    if (confinement.runtime !== null) pieces.push(t('templates.confinement.runtime', { runtime: confinement.runtime }));
    if (confinement.network !== null) pieces.push(t('templates.confinement.network', { mode: confinement.network }));
    if (confinement.readOnlyRoot) pieces.push(t('templates.confinement.readOnly'));
    return pieces.length === 0 ? '–' : pieces.join(t('startFlow.separator'));
  };
}

export function TemplateList({ search, onSearch }: { search: TemplatesSearch; onSearch: (patch: Partial<TemplatesSearch>) => void }) {
  const { t } = useTranslation();
  const [adding, setAdding] = useState(false);
  const acts = useTemplateActs();
  const imageWords = useImageWords();
  const isolation = useIsolationWords();
  const hosts = useQuery(hostsQuery(null));
  // What a template runs on and under, from what the hosts report: the driver's title and the runtime's.
  // Read once for the hosts, not again for every row: each driver's title and the classes its hosts report.
  const drivers = useMemo(() => {
    const found = new Map<string, { title: string; classes: RuntimeClass[] }>();
    for (const host of hosts.data ?? []) {
      for (const entry of host.drivers) {
        const driver = found.get(entry.name) ?? { title: entry.title, classes: [] };
        for (const runtime of entry.runtimes ?? []) if (!driver.classes.some((known) => known.name === runtime.name)) driver.classes.push(runtime);
        found.set(entry.name, driver);
      }
    }
    return found;
  }, [hosts.data]);
  const runsOn = (template: Template): string => isolation.driver(drivers.get(template.driver)?.title ?? template.driver);
  const runtimeOf = (template: Template): { title: string; hint: string } | null => {
    const name = templateRuntime(template.spec);
    const found = classNamed(drivers.get(template.driver)?.classes ?? [], name);
    if (name === null) return null;
    return { title: found?.title ?? name, hint: found === undefined ? name : isolation.hint(found.title, found.boundary) };
  };
  const [text, setText] = useSearchText(search.q ?? '', (q) => onSearch({ q: q === '' ? undefined : q }));

  const columns: Column<Template>[] = [
    {
      id: 'name',
      header: t('templates.list.name'),
      className: 'min-w-48!',
      cell: (template, shown) => {
        const withoutSource = !shown.has('source');
        const withoutKind = !shown.has('kind');
        const withoutPair = !shown.has('runson') && !shown.has('runtime');
        return (
          <div className="flex min-w-0 flex-col gap-1">
            <span className="flex min-w-0 items-center gap-2">
              <TemplateLink template={template} />
              {!template.enabled && <Badge>{t('resources.disabled')}</Badge>}
            </span>
            {withoutKind && <span className="text-sm text-text-muted">{t(`templates.kind.${template.kind}`)}</span>}
            {withoutPair && template.kind === 'environment' && <span className="text-sm text-text-muted">{isolation.pair(runsOn(template), runtimeOf(template)?.title)}</span>}
            {withoutSource && <span className="truncate text-sm text-text-muted" title={imageWords(template.spec)}>{imageSource(template.spec).kind === 'image' ? <ImageWords spec={template.spec} /> : imageWords(template.spec)}</span>}
          </div>
        );
      },
    },
    { id: 'runson', header: t('isolation.runsOn'), from: 'md', className: 'whitespace-nowrap', cell: (template) => runsOn(template) },
    { id: 'runtime', header: t('isolation.runtime'), from: 'md', className: 'whitespace-nowrap', cell: (template) => {
      const runtime = runtimeOf(template);
      return runtime === null ? <span className="text-text-muted">–</span> : <span title={runtime.hint}>{runtime.title}</span>;
    } },
    { id: 'kind', header: t('templates.list.kind'), from: 'sm', cell: (template) => <Badge className="whitespace-nowrap">{t(`templates.kind.${template.kind}`)}</Badge> },
    {
      id: 'owner',
      header: t('templates.list.owner'),
      from: 'lg',
      cell: (template) => (template.owner === null ? <span className="text-text-muted">{t('templates.list.system')}</span> : <UserChip user={template.owner} size={24} thing={{ kind: 'template', id: template.id }} />),
    },
    {
      id: 'source',
      header: t('templates.list.source'),
      from: 'lg',
      cell: (template) => {
        const source = imageSource(template.spec);
        // The content-sized column keeps the whole reference; genuinely wide data uses the table's scroller.
        return source.kind === 'image' ? (
          <code className="block whitespace-nowrap text-sm">
            {source.reference}
          </code>
        ) : (
          <span>{imageWords(template.spec)}</span>
        );
      },
    },
    { id: 'units', header: t('templates.list.units'), from: 'md', align: 'end', className: 'text-right whitespace-nowrap tabular-nums', cell: (template) => template.units },
  ];

  const add = (
    <Button variant="primary" icon={<Plus aria-hidden />} onClick={() => setAdding(true)}>
      {t('templates.list.add')}
    </Button>
  );

  return (
    <>
      <ResourceList
        kind="templates"
        search={{ ...search }}
        acts={acts.list}
        title={t('screens.templates.title')}
        icon={Cpu}
        lead={t('templates.list.lead')}
        label={t('templates.list.label')}
        readRow={resourceReaders.templates}
        filter={{ kind: search.kind }}
        columns={columns}
        rowKey={(template) => template.id}
        rowClassName={(template) => (template.enabled ? undefined : 'bg-bg-subtle')}
        text={text}
        onText={setText}
        searchLabel={t('templates.list.search')}
        searchPlaceholder={t('templates.list.searchPlaceholder')}
        filters={
          <SegmentedControl
            label={t('templates.list.kindFilter')}
            value={search.kind ?? 'all'}
            onValueChange={(next) => onSearch({ kind: oneOf(TEMPLATE_KINDS, next) })}
            segments={FILTER_KINDS.map((value)=>({value,name:t(`templates.purposeChoices.${value}.name`),description:t(`templates.purposeChoices.${value}.description`)}))}
          />
        }
        action={add}
        empty={{ icon: <Cpu aria-hidden />, title: t('templates.list.none'), description: t('templates.list.noneLead'), action: add }}
        noMatch={t('templates.list.noMatch')}
      />
      {adding && <NewTemplateDialog onClose={() => setAdding(false)} />}
    </>
  );
}
