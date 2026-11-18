/** The page a screen of a place stands in: a column that scrolls by itself, a header with the place's name and what can be done with it, and sections under it.
 *
 * The screens of an org, a project and a topic are laid out alike so that they read as one product:
 * a mark and a name, a line that says what the place is, a few facts in quiet type, the acts that are
 * its own at the end of the line, and under them its sections, each with a heading of its own.
 */
import { ResourceLibrary, hasLibrary } from '@/features/resources/Library';
import { ArrowLeft, ChevronDown } from 'lucide-react';
import { createContext, type ReactNode, useContext, useId, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { TrailPart } from '@/shell/screen';
import { TrailLink } from '@/shell/TrailLink';
import { cx } from '@/ui/cx';
import type { AdminAccess } from '@/generated/api-types';
import { AsAdminMark, privateAdminAccess } from '@/ui/AsAdminMark';
import { ResourceOwner, type ResourceOwnership } from '@/features/resources/ResourceOwner';
import { useAdminOpening } from '@/features/access/opening';
import { ScrollArea } from '@/ui/ScrollArea';
import { IconButton } from '@/ui/IconButton';
import { ResourceHeaderExpanded } from '@/features/resources/headerContext';
import { compactControls } from '@/ui/FilterBar';

/** The scrolling column of a screen. It is as wide as reading is comfortable, and keeps a margin on a small screen. */
export function Page({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <ScrollArea className="h-full">
      <div className={cx('mx-auto flex w-full max-w-(--container-page) flex-col gap-6 p-(--page-padding)', className)}>{children}</div>
    </ScrollArea>
  );
}

export interface PageHeaderProps {
  /** The mark of the place: its avatar or its icon. */
  mark?: ReactNode;
  title: ReactNode;
  /** A resource leads with its name, first status fact and primary action. */
  compactResource?: boolean;
  /** Help for the screen belongs beside its title, separate from field/action rows. */
  help?: ReactNode;
  headingId?: string;
  /** A surrounding tab or scope already supplies this heading. Its facts and acts remain visible. */
  titleInContext?: boolean;
  /** One line under the name that says what the place is for. */
  lead?: ReactNode;
  /** Quiet facts under it: where it lies, how many things it holds. */
  facts?: readonly ReactNode[];
  /** What can be done with it: its own acts, at the end of the line. */
  actions?: ReactNode;
  /** The list the screen was reached from: a way back in one press, shown as an arrow beside the title where the title bar's path is too narrow to hold it. */
  back?: TrailPart | undefined;
  adminAccess?: AdminAccess | null | undefined;
  private?: boolean;
  ownership?: ResourceOwnership;
  opening?: { kind: string; id: string };
}

export function PageHeader({ compactResource = false, mark, title, help, headingId, titleInContext = false, lead, facts = [], actions, back, adminAccess, private: privatePage = false, ownership, opening }: PageHeaderProps) {
  const { t } = useTranslation();
  useAdminOpening(opening?.kind ?? '', opening?.id, adminAccess !== null && adminAccess !== undefined);
  if (titleInContext && mark === undefined && back === undefined && help === undefined && (adminAccess === null || adminAccess === undefined) && !privatePage && ownership === undefined && (lead === null || lead === undefined) && facts.length === 0 && actions === undefined) return null;
  const backLink = back === undefined ? null : (
    <TrailLink
      part={back}
      className="focus-inset -mr-2 inline-flex h-[1.875rem] w-(--control-sm) shrink-0 items-center justify-center rounded-md text-text-muted transition-colors hover:bg-bg-muted hover:text-text md:hidden"
    >
      <ArrowLeft aria-hidden />
      <span className="sr-only">{t('screens.heading.back', { place: back.label })}</span>
    </TrailLink>
  );
  const metadata = <>
    <AsAdminMark of={adminAccess?.place} sentence private={privatePage || privateAdminAccess(adminAccess)} />
    {ownership !== undefined && <div className="flex items-center gap-2 text-sm text-text-muted"><span className="shrink-0">{t('resources.listOwner')}</span><ResourceOwner {...ownership} /></div>}
    {lead !== undefined && lead !== null && <div className="max-w-prose text-base wrap-anywhere text-text-muted">{lead}</div>}
    {opening !== undefined && hasLibrary(opening.kind) && !privatePage && <ResourceLibrary object={{kind:opening.kind,id:opening.id}} />}
  </>;
  if (compactResource) return <CompactResourceHeader key={opening?.id} title={title} headingId={headingId} back={backLink} mark={mark}
    state={facts[0]} actions={actions} details={<>{help}{metadata}{facts.slice(1)}</>} />;
  return (
    <header className="flex flex-wrap items-start gap-x-4 gap-y-3">
      <div className="flex min-w-0 flex-1 basis-[min(100%,14rem)] items-start gap-4">
        {backLink}
        {mark !== undefined && <div className="flex h-[1.875rem] shrink-0 items-center">{mark}</div>}
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          {(!titleInContext || help !== undefined) && <div className="flex min-w-0 items-start gap-1">
            {!titleInContext && <h1 id={headingId} className="min-w-0 text-2xl leading-[1.875rem] font-semibold wrap-anywhere text-text">{title}</h1>}
            {help !== undefined && <span className="flex h-[1.875rem] shrink-0 items-center">{help}</span>}
          </div>}
          {metadata}
          {facts.length > 0 && (
            <ul className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-text-muted">
              {facts.map((fact, index) => (
                // The facts of a header are given whole and never reordered.
                <li key={index} className="flex items-center gap-1">
                  {fact}
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
      {actions !== undefined && <div data-page-actions className={cx('flex min-h-[1.875rem] max-w-full flex-wrap items-center gap-2', compactControls)}>{actions}</div>}
    </header>
  );
}

function CompactResourceHeader({ title, headingId, back, mark, state, actions, details }: {
  title: ReactNode; headingId: string | undefined; back: ReactNode; mark: ReactNode; state: ReactNode; actions: ReactNode; details: ReactNode;
}) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const body = useId();
  return <header className="flex min-w-0 flex-col gap-2">
    <div className="flex min-w-0 items-center gap-2">
      {back}
      {mark !== undefined && <span className="hidden shrink-0 md:inline-flex">{mark}</span>}
      <h1 id={headingId} className="min-w-0 flex-1 truncate text-lg font-semibold text-text">{title}</h1>
      {state !== undefined && <span className="min-w-0 max-w-1/3 truncate text-sm text-text-muted">{state}</span>}
      {!expanded && <div data-page-actions className={cx('flex min-w-0 max-w-1/3 items-center', compactControls)}>
        <ResourceHeaderExpanded value={false}>{actions}</ResourceHeaderExpanded>
      </div>}
      <IconButton size="sm" label={t('common.details')} icon={<ChevronDown aria-hidden className={expanded ? 'rotate-180' : undefined} />}
        aria-expanded={expanded} aria-controls={body} onClick={() => setExpanded(!expanded)} />
    </div>
    <div id={body} hidden={!expanded}>
      {expanded && <div className="flex min-w-0 flex-col items-start gap-2 text-sm text-text-muted">
        {details}
        {actions !== undefined && <div data-page-actions className={cx('flex max-w-full flex-wrap items-center gap-2', compactControls)}>{actions}</div>}
      </div>}
    </div>
  </header>;
}

/** The square a place's icon stands in, where it has no avatar: the same weight as an avatar of the largest size.
 * Given `onPress` and its `label`, it is the way to the place's colour for whoever may change it. */
export function PlaceMark({ children, onPress, label }: { children: ReactNode; onPress?: (() => void) | undefined; label?: string | undefined }) {
  const look = 'flex size-10 shrink-0 items-center justify-center rounded-lg bg-bg-muted text-text-muted [&_svg]:size-5';
  if (onPress === undefined) return <span className={look}>{children}</span>;
  return (
    <button type="button" aria-label={label} className={cx(look, 'hover:bg-bg-subtle hover:ring-1 hover:ring-border-strong')} onClick={onPress}>
      {children}
    </button>
  );
}

export interface SectionProps {
  /** A section inside another section uses the next heading rank. */
  level?: 2 | 3;
  title: ReactNode;
  /** The heading's id, which the section is named by for a screen reader. */
  id: string;
  /** One line under the heading. */
  lead?: ReactNode;
  /** What can be done in the section: at the end of the heading's line. */
  action?: ReactNode;
  children: ReactNode;
  className?: string;
}

/** A scope frame owns its section's title; a reused body can label itself from that heading. */
export const PageSectionContext = createContext<{ title: string; id: string; lead?: ReactNode } | null>(null);

export function Section({ title, id, lead, action, children, className, level = 2 }: SectionProps) {
  const page = useContext(PageSectionContext);
  const namedByPage = page !== null && title === page.title;
  const description = namedByPage && lead === page.lead ? undefined : lead;
  const Heading = level === 3 ? 'h3' : 'h2';
  return (
    <section id={namedByPage ? id : undefined} aria-labelledby={namedByPage ? page.id : id} className={cx('flex flex-col gap-3', className)}>
      {(!namedByPage || description !== undefined || action !== undefined) &&
      <div className="flex flex-wrap items-start justify-between gap-x-3 gap-y-1">
        <div className="flex min-w-0 flex-col">
          {!namedByPage && <Heading id={id} className="text-md font-semibold text-text">
            {title}
          </Heading>}
          {description !== undefined && <p className={cx('text-sm text-text-muted', namedByPage && 'leading-6')}>{description}</p>}
        </div>
        {action !== undefined && <div className={cx('flex max-w-full flex-wrap items-center gap-2', compactControls)}>{action}</div>}
      </div>}
      {children}
    </section>
  );
}

/** The look of a box on a page: a thin line round a raised ground. */
export const boxClasses = 'rounded-lg border border-border bg-surface';
