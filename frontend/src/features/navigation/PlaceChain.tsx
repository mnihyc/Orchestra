/** The project and the folders a topic lies in, as links on one line: the place, with the folders on the way where there is room.
 *
 * "The project" of a topic is the one it lies in directly, and that may be a folder. Where a topic lies
 * in folders its place is the whole way through them (`Web › Checkout › Payments`), each a link to its
 * screen. A long way keeps its first place and its last and folds what is between into "…", which a
 * click or Enter expands into its real links; private steps stay unnamed and unlinked.
 */
import { ChevronRight } from 'lucide-react';
import { Fragment, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { PlaceLink, usePlaceLookup } from '@/features/navigation/places';
import type { NodeRef } from '@/generated/api-types';
import { cx } from '@/ui/cx';

/** How many places of the way are written out before the middle folds. */
const SHOWN_BEFORE_FOLDING = 3;

/** The projects and folders a node lies in directly, outermost first: the run of projects just above the last place of a path. */
export function placesAbove(path: readonly NodeRef[]): NodeRef[] {
  const above: NodeRef[] = [];
  for (let at = path.length - 2; at >= 0; at -= 1) {
    const place = path[at];
    if (place?.kind !== 'project') break;
    above.unshift(place);
  }
  return above;
}

export interface PlaceChainProps {
  /** The way through the projects and folders, outermost first. */
  places: readonly NodeRef[];
  /** Under a name in a list: the places in the colour of quiet text, underlined only where the pointer is. */
  quiet?: boolean;
  className?: string;
}

export function PlaceChain({ places, quiet = false, className }: PlaceChainProps) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  if (places.length === 0) return null;
  const folds = !expanded && places.length > SHOWN_BEFORE_FOLDING;
  const first = places[0];
  const last = places.at(-1);
  const shown = folds ? [first, last] : places;
  const hidden = folds ? places.slice(1, -1) : [];
  const draw = (place: NodeRef | undefined) =>
    place === undefined ? null : place.redacted || place.name === null ? (
      <span className="text-text-subtle">{t('folders.private')}</span>
    ) : (
      <PlaceLink
        place={place}
        className={cx('rounded-sm', quiet ? 'text-text-muted hover:text-text hover:underline' : 'text-accent-text underline underline-offset-2')}
      >
        {place.name}
      </PlaceLink>
    );
  return (
    <span className={cx('inline-flex min-w-0 flex-wrap items-center gap-x-1', className)}>
      {shown.map((place, index) => (
        <Fragment key={place?.id ?? index}>
          {index > 0 && (
            <>
              <ChevronRight className="shrink-0 text-text-disabled" aria-hidden />
              {folds && index === 1 && (
                <>
                  <button type="button" aria-label={t('ui.breadcrumbs.folded')} aria-expanded={false}
                    data-overflow-text={hidden.map((entry) => entry.redacted || entry.name === null ? t('folders.private') : entry.name).join(t('places.pathJoin'))}
                    className="focus-inset inline-flex min-h-6 min-w-6 items-center justify-center rounded-md text-text-muted hover:text-text"
                    onClick={() => setExpanded(true)}>
                    …
                  </button>
                  <ChevronRight className="shrink-0 text-text-disabled" aria-hidden />
                </>
              )}
            </>
          )}
          {draw(place)}
        </Fragment>
      ))}
    </span>
  );
}

/** The place a topic named by a reference lies in, as far as the trees held know it; nothing where none does. */
export function TopicPlace({ topic, className }: { topic: Pick<NodeRef, 'id'>; className?: string }) {
  const lookup = usePlaceLookup();
  return <PlaceChain quiet places={placesAbove(lookup.pathTo(topic.id))} {...(className === undefined ? {} : { className })} />;
}
