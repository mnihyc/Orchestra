/** What sharing a place reaches: the control that says "<Place> and everything in it" or "Only <place> itself", and the line under it that counts what is reached and what stays closed.
 *
 * Sharing a place is sharing a folder: it gives what is in it, what is made there later included, unless the
 * person means the place alone. The same two words ask the question for people and for agents. The line
 * reads the page's `covers`: how many places, threads and documents inside follow the place, and how
 * many were made private or cut loose and so stay closed to whoever is named.
 */
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import type { PageCounts, PermissionPage } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { cx } from '@/ui/cx';
import { Select } from '@/ui/Select';

import { Hint } from '@/features/help/Hint';

/** How much of a place is shared: with what is in it, or the place alone. */
export type Reach = 'inside' | 'alone';

/** The kinds of place that have something inside them to share. */
export type PlaceKind = 'org' | 'project' | 'topic';

export function ReachSelect({
  place,
  value,
  onValueChange,
  disabled = false,
  className,
}: {
  /** The name of the place. */
  place: string;
  value: Reach;
  onValueChange: (value: Reach) => void;
  disabled?: boolean;
  className?: string;
}) {
  const { t } = useTranslation();
  return (
    <div className={cx('flex items-center gap-1', className)}>
      <Select<Reach>
        label={t('access.reach.label')}
        hideLabel
        value={value}
        onValueChange={onValueChange}
        disabled={disabled}
        fieldClassName="min-w-0 flex-1"
        options={[
          { value: 'inside', ...choiceOption(t, 'reach.inside', { place }), description: t('access.reach.insideAbout') },
          { value: 'alone', ...choiceOption(t, 'reach.alone', { place }), description: t('access.reach.aloneAbout') },
        ]}
      />
      <Hint id="share-reach" />
    </div>
  );
}

/** One phrase for each thing there is some of, in the words of the kind of place it lies in: "12 topics and folders", "31 threads". */
function phrases(t: TFunction, kind: PlaceKind, counts: PageCounts, side: 'in' | 'apart'): string[] {
  const found: string[] = [];
  if (counts.places > 0 && kind !== 'topic') found.push(side === 'in' ? t(`access.covers.in.places.${kind}`, { count: counts.places }) : t(`access.covers.apart.places.${kind}`, { count: counts.places }));
  if (counts.threads > 0) found.push(side === 'in' ? t('access.covers.in.threads', { count: counts.threads }) : t('access.covers.apart.threads', { count: counts.threads }));
  if (counts.documents > 0) found.push(side === 'in' ? t('access.covers.in.documents', { count: counts.documents }) : t('access.covers.apart.documents', { count: counts.documents }));
  return found;
}

const total = (one: PageCounts, other: PageCounts): PageCounts => ({
  places: one.places + other.places,
  threads: one.threads + other.threads,
  documents: one.documents + other.documents,
});

/**
 * The line under the field: what the choice reaches as things stand. `whom` says whether it is people or
 * agents that are named, since the two are told in the same words with one difference: agents follow a
 * place's defaults, and a place shared with them alone has its own chat and its documents.
 */
export function CoversLine({ page, kind, name, reach, whom = 'people' }: { page: PermissionPage; kind: PlaceKind; name: string; reach: Reach; whom?: 'people' | 'agents' }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const covers = page.covers;
  if (covers === null) return null;
  const list = new Intl.ListFormat(locale, { type: 'conjunction' });
  const inside = phrases(t, kind, covers, 'in');
  const apart = phrases(t, kind, covers.apart, 'apart');

  let text: string;
  if (reach === 'inside') {
    const first = inside.length === 0 ? t('access.covers.later', { name }) : t(whom === 'agents' ? 'access.covers.insideAgents' : 'access.covers.inside', { what: list.format(inside), name });
    const closed = apart.length === 0 ? '' : ` ${t(whom === 'agents' ? 'access.covers.notCoveredAgents' : 'access.covers.closed', { what: list.format(apart) })}`;
    text = `${first}${closed}`;
  } else if (whom === 'agents') {
    text = t('access.covers.aloneAgents', { name });
  } else {
    const all = total(covers, covers.apart);
    if (kind !== 'topic' && all.places > 0) text = t('access.covers.alonePlaces', { name, places: t(`access.covers.in.places.${kind}`, { count: all.places }) });
    else if (all.threads > 0) text = t('access.covers.aloneThreads', { name, threads: t('access.covers.in.threads', { count: all.threads }) });
    else text = t('access.covers.aloneEmpty', { name });
  }
  return (
    <p className="text-sm text-text-muted" aria-live="polite">
      {text}
    </p>
  );
}
