/** The same next step after a saved second factor, without changing the page the person was opening. */
import { i18n } from '@/i18n';
import { toast } from '@/ui/Toast';

let putAway: (() => void) | undefined;

/** The person may have used their last code: keep the way to save a replacement visible. */
export function recoveryCodeUsed(): void {
  putAway?.();
  putAway = toast({ tone: 'info', persistent: true, title: i18n.t('security.recovery.used'), description: i18n.t('security.recovery.replaceSoon'),
    action: { label: i18n.t('security.recovery.open'), run: () => window.location.assign('/settings/security') } });
}

/** A new sheet is already being shown; the earlier reminder no longer applies. */
export function recoveryCodesIssued(): void { putAway?.(); putAway = undefined; }
