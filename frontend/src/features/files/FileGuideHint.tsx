/** One file help hint opens the matching heading in the reader's authored guide. */
import { useTranslation } from 'react-i18next';
import { useGuideEntry } from '@/features/help/guideEntries';
import { useHelpOpener } from '@/features/help/open';
import { FILE_GUIDE, fileGuideTarget, type FileGuideSection } from '@/help/files';
import { HelpHint } from '@/ui/HelpHint';

export function FileGuideHint({ section }: { section: FileGuideSection }) {
  const { t, i18n } = useTranslation();
  const open = useHelpOpener();
  const target = fileGuideTarget(section, i18n.resolvedLanguage ?? i18n.language);
  const words = FILE_GUIDE.sections[section];
  const { ref, hidden } = useGuideEntry(target.article, target.heading, 'group');
  return <span ref={ref} className="contents">{!hidden && <HelpHint term={t(words.term)} what={t(words.what)} guide={target}
    more={{ label: t('help.hint.more'), onOpen: () => open(target.article, target.heading) }} />}</span>;
}
