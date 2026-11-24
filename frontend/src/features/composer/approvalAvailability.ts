/** Native approval refusals share the same point-of-use words in every control. */
import type { TFunction } from 'i18next';
import type { TranslationKey } from '@/i18n';

const WORDS: Readonly<Record<string, TranslationKey | undefined>> = {
  reviewer_unavailable: 'composer.approvalUnavailable.reviewer',
  reviewer_access_required: 'composer.approvalUnavailable.reviewerAccess',
  not_allowed_here: 'composer.approvalUnavailable.policy',
  required_isolation_unavailable: 'composer.approvalUnavailable.isolation',
  not_supported_by_build: 'composer.approvalUnavailable.build',
  auto_not_proven: 'composer.approvalUnavailable.autoNotProven',
  auto_check_needs_host: 'composer.approvalUnavailable.autoNeedsHost',
  auto_check_failed: 'composer.approvalUnavailable.autoCheckFailed',
  auto_check_unavailable: 'composer.approvalUnavailable.autoCheckUnavailable',
};

export function approvalUnavailableWords(reason: string | null | undefined, t: TFunction): string | null {
  const key = reason === null || reason === undefined ? undefined : WORDS[reason];
  return key === undefined ? null : t(key);
}
