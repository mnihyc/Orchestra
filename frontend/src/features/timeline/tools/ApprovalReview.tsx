/** The harness's permission review, with the model, outcome and reason it reported. */
import { useTranslation } from 'react-i18next';
import { JsonTree } from '@/ui/JsonTree';

import { approvalReview, reviewOutcome } from './reviewFacts';
export { approvalReview } from './reviewFacts';

export function ApprovalReviewDisplay({ value }: { value: unknown }) {
  const { t } = useTranslation();
  const review = approvalReview(value);
  if (review === null) return null;
  return <div className="flex flex-col gap-2 text-sm">
    <p>{t('timeline.permissionReview.outcome', { outcome: reviewOutcome(review.outcome, t), model: review.model })}</p>
    {review.reason !== null && <p className="text-text-muted">{review.reason}</p>}
    {!review.reported && <p className="text-xs text-text-subtle">{t('timeline.permissionReview.declared')}</p>}
    <JsonTree value={review.action} label={t('timeline.permissionReview.action')} />
    {review.usage !== null && review.usage !== undefined && <JsonTree value={review.usage} label={t('timeline.permissionReview.usage')} />}
  </div>;
}
