/** Resource mentions use the finder's same authorized, root-scoped name search. */
import { useTranslation } from 'react-i18next';
import { ALL_NAME_KINDS } from '@/api/search';
import { NamePicker } from '@/features/search/NamePicker';
import { markdownReference } from '@/lib/links';
import { publicOrchLink } from '@/lib/orchLinks';
import { Dialog } from '@/ui/Dialog';

export function ResourcePicker({ root, onChoose, onClose }: { root: string; onChoose: (reference: string) => void; onClose: () => void }) {
  const { t } = useTranslation();
  return <Dialog open onOpenChange={(open) => { if (!open) onClose(); }} title={t('ask.resources')}>
    <NamePicker label={t('ask.findResource')} kinds={ALL_NAME_KINDS} orgId={root} chosen={null} placeholder={t('ask.findResource')} nothing={t('ask.noResources')} pickerKey="ask-reference" accepts={(hit) => hit.href != null}
      onChoose={(hit) => { if (hit?.href != null) { onChoose(markdownReference(hit.name, publicOrchLink(hit.href))); onClose(); } }} />
  </Dialog>;
}
