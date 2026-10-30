/** Shared HelpHint component (help): the "?" beside a term, a choice or a field, with its card in place.
 *
 * The card has up to four parts, each only where there is something to say: what the thing is, which one to
 * choose by what the reader wants, where the value in front of them is set (a link, given by the screen),
 * and more, which leads to the article that treats it. It opens beside the "?" and closes on Escape and on a
 * press beside it; the same bounded card serves every screen width.
 */
import { CircleQuestionMark } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { cx } from '@/ui/cx';
import { linkButtonClasses } from '@/ui/link';
import { Popover } from '@/ui/Popover';

export interface HelpHintProps {
  /** The term, choice or field it stands beside: it names the button and the card. */
  term: string;
  /** What this is, in one or two sentences. */
  what: ReactNode;
  /** Which one for me: the choice by what the reader wants, not by its name. */
  which?: ReactNode;
  /** Where the value in front of the reader is set, as a link. */
  where?: ReactNode;
  /** The article that treats it, at the right heading. */
  more?: { label: string; onOpen: () => void };
  /** A group of setting entries closes its question before opening one of those entries. */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** The actual destination of More, shared with the screen's guide-entry audit. */
  guide?: { article: string; heading: string | null };
  className?: string;
}

function Part({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-0.5">
      <h3 className="text-xs font-medium text-text-muted">{title}</h3>
      <div className="text-base text-text">{children}</div>
    </div>
  );
}

export function HelpHint({ term, what, which, where, more, open: suppliedOpen, onOpenChange, guide, className }: HelpHintProps) {
  const { t } = useTranslation();
  const [localOpen, setLocalOpen] = useState(false);
  const open = suppliedOpen ?? localOpen;
  const setOpen = (next: boolean) => {
    if (suppliedOpen === undefined) setLocalOpen(next);
    onOpenChange?.(next);
  };

  const card = (
    <div className="flex flex-col gap-3">
      <Part title={t('help.hint.what')}>{what}</Part>
      {which !== undefined && <Part title={t('help.hint.which')}>{which}</Part>}
      {where !== undefined && <Part title={t('help.hint.where')}>{where}</Part>}
      {more !== undefined && (
        <button
          type="button"
          className={cx('self-start text-base', linkButtonClasses)}
          onClick={() => {
            setOpen(false);
            more.onOpen();
          }}
        >
          {more.label}
        </button>
      )}
    </div>
  );

  const trigger = (
    <button
      type="button"
      aria-label={t('help.hint.label', { term })}
      aria-expanded={open}
      data-help-guide={guide?.article}
      data-help-article={guide?.article}
      data-help-heading={guide?.heading ?? undefined}
      className={cx(
        'inline-flex size-6 shrink-0 items-center justify-center rounded-full text-text-subtle transition-colors hover:bg-bg-muted hover:text-text',
        className,
      )}
    >
      <CircleQuestionMark size={14} aria-hidden />
    </button>
  );

  return (
    <Popover label={term} open={open} onOpenChange={setOpen} trigger={trigger} className="w-(--hint-width)">
      {card}
    </Popover>
  );
}
