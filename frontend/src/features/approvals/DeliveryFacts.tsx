/** Native receipt is separate from the accepted answer, inside the approval's existing opening. */
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import type { Approval } from '@/generated/api-types';
import { FactDisclosureBody } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';

export function approvalDeliveryFragment(approval: Approval, t: TFunction): string | null {
  return approval.delivery?.state === 'pending' ? t('approvalDeliveryFacts.pending')
    : approval.delivery?.state === 'unknown' ? t('approvalDeliveryFacts.unknown') : null;
}

export function ApprovalDeliveryFacts({ approval }: { approval: Approval }) {
  const { t } = useTranslation();
  if (approval.kind !== 'harness') return null;
  const receipt = approval.delivery ?? null;
  return <FactDisclosureBody groups={[{ key: 'native-delivery', kind: 'recorded_outcome', facts: [
    { key: 'delivery', label: t('approvalDeliveryFacts.delivery'), value: receipt === null ? undefined : t(receipt.state === 'pending' ? 'approvalDeliveryFacts.pendingValue' : receipt.state === 'applied' ? 'approvalDeliveryFacts.applied' : 'approvalDeliveryFacts.unknownValue') },
    { key: 'recipient', label: t('approvalDeliveryFacts.recipient'), value: receipt === null ? undefined : t('approvalDeliveryFacts.native') },
    { key: 'attempt', label: t('approvalDeliveryFacts.attempt'), value: receipt?.last_attempt_at === undefined || receipt.last_attempt_at === null ? undefined : <RelativeTime at={receipt.last_attempt_at} /> },
    ...(receipt?.problem === null || receipt === null ? [] : [{ key: 'problem', label: t('approvalDeliveryFacts.problem'), value: t(`approvalDeliveryFacts.${receipt.problem}`) }]),
  ] }]} />;
}
