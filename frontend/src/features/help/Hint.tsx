/** A hint beside a term, a choice or a field, made from the catalogue by its id.
 *
 * The control (`ui/HelpHint.tsx`) draws the "?" and its card; this one knows where the words are: the
 * catalogue holds what it is and which one to choose, and the guide holds the article that "More" opens.
 * A screen that knows where the value in front of the reader is set gives that as `where`. A hint that is
 * drawn is listed in the help of the screen it is on.
 */
import { type ReactNode, useEffect } from 'react';
import { useTranslation } from 'react-i18next';

import { type HintId, hintOf } from '@/features/help/hints';
import { noteHint } from '@/features/help/mounted';
import { useGuideEntry } from '@/features/help/guideEntries';
import { useHelpOpener } from '@/features/help/open';
import { useGuide } from '@/help';
import { articleAvailable } from '@/help/guide';
import { HelpHint } from '@/ui/HelpHint';

export interface HintProps {
  id: HintId;
  /** Where the value in front of the reader is set, as a link. */
  where?: ReactNode;
  /** Keep the question beside its control in the shared bounded card at every width. */
  popover?: boolean;
  className?: string;
}

export function Hint({ id, where, className }: HintProps) {
  const { t, i18n } = useTranslation();
  const openHelp = useHelpOpener();
  const parts = hintOf(id, t, i18n.resolvedLanguage ?? i18n.language);
  useEffect(() => noteHint(id), [id]);
  const { article } = parts;
  // Read an already loaded guide without fetching prose alongside an unrelated control.
  const guide = useGuide(false);
  // Destinations are validated when built; the reading view loads their body when More is chosen.
  const ready = article !== null && (guide === undefined || guide.articles.some((entry) => entry.id === article.id && articleAvailable(entry)));
  const { ref, hidden } = useGuideEntry(ready ? article.id : null, article?.heading ?? null);
  return (
    <span ref={ref} className="contents" data-help-control={id}>{!hidden && <HelpHint
      term={parts.title}
      what={parts.what}
      {...(parts.which === null ? {} : { which: parts.which })}
      {...(where === undefined ? {} : { where })}
      {...(article === null || !ready ? {} : { guide: { article: article.id, heading: article.heading }, more: { label: t('help.hint.more'), onOpen: () => article.heading === null ? openHelp(article.id) : openHelp(article.id, article.heading) } })}
      {...(className === undefined ? {} : { className })}
    />}</span>
  );
}
