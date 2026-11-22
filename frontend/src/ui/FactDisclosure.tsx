/** One subject's authorized facts, beside its value or inside its already open menu. */
import { Info } from 'lucide-react';
import { Children, type HTMLAttributes, type ReactNode, type Ref, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { IconButton } from '@/ui/IconButton';
import { Popover } from '@/ui/Popover';
import { cx } from '@/ui/cx';

export interface DisclosureFact {
  key: string;
  label: ReactNode;
  /** Already authorized words, formatted figures or shared identity chips; absence is not zero. */
  value?: ReactNode;
  /** An effective default's existing SourceSettingLink, when that source is readable. */
  source?: ReactNode;
}

const groupWords = {
  last_result: 'factDisclosure.groups.lastResult',
  next_refresh: 'factDisclosure.groups.nextRefresh',
  current_setting: 'factDisclosure.groups.currentSetting',
  recorded_outcome: 'factDisclosure.groups.recordedOutcome',
} as const;

export interface FactGroup {
  key: string;
  kind?: keyof typeof groupWords;
  title?: ReactNode;
  facts: readonly DisclosureFact[];
}

export interface FactDisclosureBodyProps extends Omit<HTMLAttributes<HTMLDivElement>, 'children'> {
  /** Omit an unauthorized/inapplicable group; an uncaptured value within a group stays explicit. */
  groups: readonly (FactGroup | null | undefined | false)[];
  header?: ReactNode;
  children?: ReactNode;
  className?: string;
  ref?: Ref<HTMLDivElement>;
}

const present = (value: ReactNode): boolean => Children.toArray(value).some((part) => typeof part !== 'string' || part.trim() !== '');

/** Use this body directly in an open picker/menu, without another popover or read. */
export function FactDisclosureBody({ groups, header, children, className, ref, tabIndex = -1, onKeyDown, ...rest }: FactDisclosureBodyProps) {
  const { t } = useTranslation();
  return <div {...rest} ref={ref} tabIndex={tabIndex} data-fact-disclosure-body onKeyDown={(event) => { onKeyDown?.(event); if (rest.role === 'menuitem' && event.key === 'Tab') event.stopPropagation(); }} className={cx('flex min-w-0 flex-col gap-2 p-1 outline-none', className)}>
    {header}
    {children}
    {groups.map((group) => {
      if (!group || group.facts.length === 0) return null;
      const title = group.title ?? (group.kind === undefined ? undefined : t(groupWords[group.kind]));
      return <div key={group.key} className="flex min-w-0 flex-col gap-2" data-fact-group={group.key}>
        {present(title) && <p className="text-sm font-medium text-text">{title}</p>}
        <dl aria-label={typeof title === 'string' ? title : undefined} className="grid min-w-0 grid-cols-1 gap-2">
          {group.facts.map((fact) => <div key={fact.key} className="flex min-w-0 flex-wrap justify-between gap-x-4 gap-y-1 text-sm">
            <dt className="min-w-0 wrap-anywhere text-text-muted">{fact.label}</dt>
            <dd className="m-0 flex min-w-0 max-w-full flex-wrap items-baseline gap-1 wrap-anywhere text-text">
              {present(fact.value) ? fact.value : t('factDisclosure.notReported')}{fact.source}
            </dd>
          </div>)}
        </dl>
      </div>;
    })}
  </div>;
}

export interface FactDisclosureProps extends Pick<FactDisclosureBodyProps, 'groups' | 'header' | 'children'> {
  label?: string;
  /** One current exception/applied-outcome fragment; routine healthy work supplies none. */
  fragment?: ReactNode;
  open?: boolean;
  /** Activate at most one existing ordinary authorized read; never refresh, retry or mutate. */
  onOpenChange?: (open: boolean) => void;
  className?: string;
}

export function FactDisclosure({ label, fragment, groups, header, children, open, onOpenChange, className }: FactDisclosureProps) {
  const { t } = useTranslation();
  const frame = useRef<HTMLDivElement>(null);
  const name = label ?? t('common.details');
  const hasBody = present(header) || present(children) || groups.some((group) => group && group.facts.length > 0);
  if (!hasBody && !present(fragment)) return null;
  return <span className={cx('inline-flex min-w-0 max-w-full flex-wrap items-center gap-1', className)}>
    {present(fragment) && <span className="min-w-0 wrap-anywhere text-sm text-text-muted" data-fact-disclosure-fragment>{fragment}</span>}
    {hasBody && <Popover label={name} {...(open === undefined ? {} : { open })} {...(onOpenChange === undefined ? {} : { onOpenChange })}
      trigger={<IconButton size="sm" label={name} icon={<Info aria-hidden />} />}
      onOpenAutoFocus={(event) => { event.preventDefault(); frame.current?.focus(); }}>
      <FactDisclosureBody ref={frame} groups={groups} header={header}>{children}</FactDisclosureBody>
    </Popover>}
  </span>;
}
