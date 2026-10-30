/** Upload help uses the shared question card and opens its exact guide section beside the draft. */
import { useTranslation } from 'react-i18next';
import { useGuideEntry } from '@/features/help/guideEntries';
import { useHelpOpener } from '@/features/help/open';
import { UPLOAD_HELP, uploadHelpHeading, type UploadHelpTopic } from '@/help/uploads';
import { HelpHint } from '@/ui/HelpHint';

export function UploadHelp({ topic }: { topic: UploadHelpTopic }) {
  const { t } = useTranslation();
  const open = useHelpOpener();
  const entry = UPLOAD_HELP[topic];
  const heading = uploadHelpHeading(topic);
  const { ref, hidden } = useGuideEntry(entry.article, heading);
  return <span ref={ref} className="contents" data-help-control={`upload-${topic}`}>{!hidden && <HelpHint term={t(entry.title)} what={t(entry.what)}
    guide={{ article: entry.article, heading }} more={{ label: t('help.hint.more'), onOpen: () => open(entry.article, heading) }} />}</span>;
}
