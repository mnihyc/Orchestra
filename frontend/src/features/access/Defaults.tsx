/** What a place gives to what is made inside it: a grid for each kind, two columns where it depends on where work runs, locks, and how many pages follow.
 *
 * A place has a defaults page apart from its own: the rows that every page inside it starts from and
 * follows until somebody changes one. This tab draws the same grid as the full form, once for each kind
 * of thing made inside the place; threads and environments have two grids, one for work in a managed
 * environment and one for work on a person's machine. A row can be locked: everything inside then
 * follows it exactly. Before a change is made the server is asked whom it reaches, and a change that
 * takes something away is confirmed once.
 */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { type AccessRowTarget, permissionsQuery } from '@/api/access';
import type { PermissionDef, PermissionPage, RowProfile } from '@/generated/api-types';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { Button } from '@/ui/Button';

import { GridView, Disclosure } from './ByArea';
import { type Asked, ChangePreview } from './ChangePreview';
import { type PageSource, sourceQuery, useChanges } from './changes';
import { type Catalogue, gridOf, indexCatalogue, rowOnDefaults, wholeChange } from './model';
import { ChangeFailed, StaleNotice } from './ShortForm';

/** The kinds of thing a place makes inside it that have a grid of their own, in the order a person goes down the tree. */
const INSIDE = ['org', 'project', 'topic', 'thread', 'environment'] as const;
/** The kinds of resource that people bring, whose defaults an org keeps for those whose home it is. */
const BROUGHT = ['provider', 'agent', 'host', 'template', 'mcp_server', 'mount', 'skill'] as const;

type Inside = (typeof INSIDE)[number];

/** The permissions that exist on exactly one kind: what is particular to it, which is what its grid is of. */
const particular = (catalogue: Catalogue, kind: string): PermissionDef[] => catalogue.of(kind).filter((def) => def.on.length === 1);

/** The kinds that this place's defaults page has rows for. */
function kindsOf(page: PermissionPage, catalogue: Catalogue): Inside[] {
  const present = new Set(page.rows.map((row) => row.permission));
  return INSIDE.filter((kind) => particular(catalogue, kind).some((def) => present.has(def.name)));
}

function Grids({
  kind,
  page,
  catalogue,
  place,
  source,
  changes,
  ask,
}: {
  kind: (typeof INSIDE)[number] | (typeof BROUGHT)[number];
  page: PermissionPage;
  catalogue: Catalogue;
  place: AccessRowTarget;
  source: PageSource;
  changes: ReturnType<typeof useChanges>;
  ask: (asked: Asked) => void;
}) {
  const { t } = useTranslation();
  const defs = particular(catalogue, kind);
  const split = defs.some((def) => def.profiles);
  const profiles: readonly RowProfile[] = split ? ['managed', 'connected'] : ['any'];
  return (
    <div className="flex flex-col gap-4">
      {profiles.map((profile) => (
        <section key={profile} aria-label={profile === 'any' ? t(`access.kinds.${kind}`) : t(`access.defaults.${profile}`)} className="flex flex-col gap-2">
          {split && <h4 className="text-sm font-medium text-text">{t(`access.defaults.${profile}`)}</h4>}
          <GridView
            grid={gridOf(defs, rowOnDefaults(page, profile))}
            target={place}
            source={source}
            changes={changes}
            ask={ask}
            profile={profile}
            page={page}
            kind={kind}
            hideAdvanced={false}
          />
        </section>
      ))}
    </div>
  );
}

export function Defaults({ place }: { place: AccessRowTarget }) {
  const { t } = useTranslation();
  const source: PageSource = place.kind === 'system' ? { type: 'system-defaults', place } : { type: 'defaults', place };
  const catalogueQuery = useQuery(permissionsQuery);
  const pageQuery = useQuery(sourceQuery(source));
  const changes = useChanges(source);
  const [asked, setAsked] = useState<Asked | null>(null);
  const [chosen, setChosen] = useState<Inside | null>(null);

  if (catalogueQuery.isPending || pageQuery.isPending) return <SkeletonRows rows={6} />;
  if (pageQuery.isError) return <ErrorState compact error={pageQuery.error} retrying={pageQuery.isRefetching} onRetry={() => void pageQuery.refetch()} />;
  if (catalogueQuery.isError) return <ErrorState compact error={catalogueQuery.error} retrying={catalogueQuery.isRefetching} onRetry={() => void catalogueQuery.refetch()} />;

  const catalogue = indexCatalogue(catalogueQuery.data);
  const page = pageQuery.data;
  const kinds = kindsOf(page, catalogue);
  const kind = chosen !== null && kinds.includes(chosen) ? chosen : (kinds[0] ?? null);
  const brought = BROUGHT.filter((entry) => particular(catalogue, entry).some((def) => page.rows.some((row) => row.permission === def.name)));
  const shared = catalogue.reply.permissions.filter((def) => def.on.length > 1 && page.rows.some((row) => row.permission === def.name));

  return (
    <div className="flex flex-col gap-4">
      <StaleNotice changes={changes} />
      <ChangeFailed changes={changes} />
      <div className="flex flex-col gap-1">
        <p className="text-sm text-text-muted">{t('access.defaults.lead', { name: place.name })}</p>
        {page.followers !== null && <p className="text-sm font-medium text-text">{t('access.defaults.followers', { count: page.followers })}</p>}
      </div>

      {place.kind === 'system' && page.rows.some((row) => row.writable) && <div><Button size="sm" disabled={changes.busy} onClick={() => setAsked({
        title: t('adminDefaults.resetTitle'), body: t('adminDefaults.resetBody'), confirmLabel: t('adminDefaults.reset'),
        label: t('adminDefaults.resetting'), make: (current) => wholeChange(current, 'reset'),
      })}>{t('adminDefaults.reset')}</Button></div>}

      {kind !== null && (
        <>
          <SegmentedControl<Inside>
            label={t('access.defaults.kinds')}
            value={kind}
            onValueChange={setChosen}
            segments={kinds.map((entry) => ({ value: entry, ...choiceOption(t, `placeCategory.${entry}`) }))}
            className="self-start"
          />
          <Grids kind={kind} page={page} catalogue={catalogue} place={place} source={source} changes={changes} ask={setAsked} />
        </>
      )}

      {shared.length > 0 && (
        <Disclosure title={t('access.defaults.everything')} lead={t('access.defaults.everythingAbout')}>
          <div className="py-3">
            <GridView
              grid={gridOf(shared, rowOnDefaults(page, 'any'))}
              target={place}
              source={source}
              changes={changes}
              ask={setAsked}
              profile="any"
              page={page}
              kind={place.kind}
              hideAdvanced={false}
            />
          </div>
        </Disclosure>
      )}

      {(place.kind === 'org' || place.kind === 'system') && brought.length > 0 && (
        <Disclosure title={t('access.defaults.brought')} lead={t('access.defaults.broughtAbout')}>
          <div className="flex flex-col gap-4 py-3">
            {brought.map((entry) => (
              <section key={entry} aria-label={t(`access.kinds.${entry}`)} className="flex flex-col gap-2">
                <h4 className="text-sm font-medium text-text">{t(`access.kindsPlural.${entry}`)}</h4>
                <p className="text-sm text-text-muted">{t('access.defaults.whoMayUse')}</p>
                <Grids kind={entry} page={page} catalogue={catalogue} place={place} source={source} changes={changes} ask={setAsked} />
              </section>
            ))}
          </div>
        </Disclosure>
      )}

      <ChangePreview source={source} asked={asked} changes={changes} onClose={() => setAsked(null)} />
    </div>
  );
}
