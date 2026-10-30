/** A refused setting choice names the reported changer and current model while leaving its controls open. */
import { useTranslation } from 'react-i18next';

import type { SettingRefusal } from '@/features/composer/runsWith';
import { InlineAlert } from '@/ui/InlineAlert';

export function SettingConflict({ refusal }: { refusal: SettingRefusal | null }) {
  const { t } = useTranslation();
  if (refusal === null) return null;
  const model = refusal.model === '' ? t('composer.model.none') : refusal.model;
  const words = refusal.name === null
    ? t('composer.model.conflict.current', { model })
    : refusal.key === 'agent.model'
      ? t('composer.model.conflict.model', { name: refusal.name, model })
      : t('composer.model.conflict.setting', { name: refusal.name, model });
  return <InlineAlert tone="info" title={t('composer.model.conflict.title')}><p>{words}</p></InlineAlert>;
}
