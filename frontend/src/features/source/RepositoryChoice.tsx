/** The topic's repository as an optional clone into a new directory, the same on every destination; nothing when the topic names none.
 *
 * The label says what ticking it does, and an existing directory is never touched, so nothing else stands
 * beside it. A machine that refuses the writes says so on the thread, with Prepare again, once it is known.
 */
import { useTranslation } from 'react-i18next';
import type { BindingOptions } from '@/generated/api-types';
import { Checkbox } from '@/ui/Checkbox';

export function RepositoryChoice({ source, checked, onChange }: {
  source: BindingOptions['source']; checked: boolean; onChange: (checked: boolean) => void;
}) {
  const { t } = useTranslation();
  if (source.repository_url === null) return null;
  return <div className="flex flex-col gap-1">
    <Checkbox label={t('source.prepare.choice')} checked={checked} onCheckedChange={onChange} />
    <p className="wrap-anywhere text-sm text-text-muted">{source.repository_url}</p>
  </div>;
}
