/** System channels use the same notifier forms, tests and write-only secrets as an org's. */
import { useTranslation } from 'react-i18next';
import { SystemChannels } from '@/features/settings/org/Channels';
import { SettingsEditor } from '@/features/settings/Editor';

const POLICY_KEYS = ['mail.shared', 'outbound.allowed_private_hosts'] as const;

export function Channels() {
  const { t } = useTranslation();
  return <div className="flex min-w-0 flex-col gap-6"><SystemChannels />
    <SettingsEditor target={{ kind: 'system' }} scope="system" compact only={POLICY_KEYS}
      place={{ name: t('access.place.system'), owner: null, access: null, kind: null }} />
  </div>;
}
