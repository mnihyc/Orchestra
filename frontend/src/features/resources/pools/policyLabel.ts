import type { TFunction } from 'i18next';
import type { PoolPolicy } from '@/generated/api-types';

export function poolPolicyLabel(t: TFunction, policy: PoolPolicy | null | undefined): string {
  if (!policy) return t('pools.policy.choose');
  if (policy.type === 'least_loaded') {
    return t('pools.measuredPolicy', { measure: t(`pools.measure.${policy.measure}`) });
  }
  return t(`pools.policy.${policy.type}`);
}
