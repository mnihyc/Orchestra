/** Recovery terms use the shared question card and open the authored section in the reader's language. */
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { useGuideEntry } from '@/features/help/guideEntries';
import { useHelpOpener } from '@/features/help/open';
import { useGuide } from '@/help';
import { articleAvailable, articleInLanguage } from '@/help/guide';
import { HelpHint } from '@/ui/HelpHint';

const SECTIONS = {
  rules: { article: 'model-call-recovery', heading: 'set-rules' },
  candidates: { article: 'model-call-recovery', heading: 'choose-candidates' },
  recovery: { article: 'model-call-recovery', heading: 'during-recovery' },
  guard: { article: 'model-call-recovery', heading: 'stop-repeated-failures' },
  automatic: { article: 'model-call-recovery', heading: 'automatic-work-and-access' },
  services: { article: 'model-call-recovery', heading: 'summaries-and-descriptions' },
} as const;

/** The same destination is coalesced within a screen, while a rule editor keeps its own question. */
export function CallHelp({ section, term, what }: { section: keyof typeof SECTIONS; term?: string; what?: ReactNode }) {
  const { t, i18n } = useTranslation();
  const guide = useGuide();
  const open = useHelpOpener();
  const destination = SECTIONS[section];
  const source = guide?.articles.find((article) => article.id === destination.article && articleAvailable(article));
  // The two authored bodies keep the same section order; their actual heading ids differ by language.
  const position = source?.headings.findIndex((heading) => heading.id === destination.heading) ?? -1;
  const localized = source === undefined ? undefined : articleInLanguage(source, i18n.resolvedLanguage ?? i18n.language);
  const heading = localized?.headings[position]?.id;
  const { ref, hidden } = useGuideEntry(heading === undefined ? null : destination.article, heading ?? null);
  return <span ref={ref} className="contents">{!hidden && <HelpHint
    term={term ?? t(`calls.help.${section}.title`)}
    what={what ?? t(`calls.help.${section}.what`)}
    {...(heading === undefined ? {} : {
      guide: { article: destination.article, heading },
      more: { label: t('help.hint.more'), onOpen: () => open(destination.article, heading) },
    })}
  />}</span>;
}
