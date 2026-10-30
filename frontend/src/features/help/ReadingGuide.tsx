/** The workbench's existing layout menu links to the same bounded-card guidance at every width. */
import { useTranslation } from 'react-i18next';
import { useGuideEntry } from '@/features/help/guideEntries';
import { useHelpOpener } from '@/features/help/open';
import { READING_GUIDE, readingGuideHeading } from '@/help/reading';
import { ActionIcon } from '@/ui/actions';
import { MenuItem } from '@/ui/Menu';

export function ReadingGuideMenuItem() {
  const { t, i18n } = useTranslation();
  const open = useHelpOpener();
  const heading = readingGuideHeading(i18n.resolvedLanguage ?? i18n.language, 'cards');
  const { ref, hidden } = useGuideEntry(READING_GUIDE.id, heading, 'guide');
  return <span ref={ref} className="contents">{!hidden && <MenuItem textValue={t('readingGuide.cards')}
    data-help-guide={READING_GUIDE.id} data-help-article={READING_GUIDE.id} data-help-heading={heading}
    onSelect={() => open(READING_GUIDE.id, heading)}>
    <ActionIcon action="help" /><span>{t('readingGuide.cards')}</span>
  </MenuItem>}</span>;
}
