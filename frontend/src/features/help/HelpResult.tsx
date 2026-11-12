/** A help result keeps its short name, parent article and written summary distinct. */
import { useId } from 'react';
import { useTranslation } from 'react-i18next';
import type { HelpEntry } from '@/features/finder/local';
import { SummaryText } from '@/features/summaries/SummaryText';

export function HelpResult({ article, heading, onOpen }: HelpEntry & { onOpen: () => void }) {
  const { t } = useTranslation();
  const description = useId();
  const hasSummary = article.summary !== undefined && article.summary.trim() !== '';
  return <li data-help-result={article.id}>
    <button type="button" aria-label={heading?.text ?? article.title} aria-describedby={description}
      className="focus-inset flex w-full min-w-0 flex-col items-start gap-1 rounded-md px-2 py-2 text-left hover:bg-bg-muted" onClick={onOpen}>
      <span className="text-base wrap-anywhere text-text">{heading?.text ?? article.title}</span>
      <span id={description} className="block w-full min-w-0 text-sm text-text-muted">
        {heading !== undefined && <><span className="block wrap-anywhere">{article.title}</span>{' '}</>}
        {hasSummary ? <SummaryText inline text={article.summary} /> : heading === undefined ? t('help.search.article') : null}
      </span>
    </button>
  </li>;
}
