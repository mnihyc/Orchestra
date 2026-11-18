/** The recorded manager and moment shared by execution confirmations. */
import { useTranslation } from 'react-i18next';
import type { UserRef } from '@/generated/api-types';
import { RelativeTime } from '@/ui/RelativeTime';
import { UserChip } from '@/ui/UserChip';

export function ExecutionApproval({ approval }: { approval: { by: UserRef; at: string } }) {
  const { t } = useTranslation();
  const name = approval.by.display_name || t('environmentTools.approverUnavailable');
  return <span className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-sm text-text-muted">
    <UserChip user={{ ...approval.by, display_name: name }} size={24} className="max-w-36" />
    <RelativeTime at={approval.at} compact />
  </span>;
}
