/** What a person allowed for a thread beyond each request, as its Parameters list it, and Take back.
 *
 * Each rule shows the matcher it answers, who allowed it and when, and whether it still answers (a rule taken back,
 * or ended when the thread moved to another agent, provider, environment or directory, is listed with why). Whoever
 * manages the thread takes one back: the next request that asks for the same waits for a person again.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import { api, ok } from '@/api/client';
import { matcherWords } from '@/features/approvals/Harness';
import { GuideLink } from '@/features/help/GuideLink';
import type { ThreadApprovalRule } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { RelativeTime } from '@/ui/RelativeTime';

const rulesKey = (threadId: string) => ['thread', threadId, 'approval-rules'] as const;

export function ThreadRules({ threadId, writable }: { threadId: string; writable: boolean }) {
  const { t } = useTranslation();
  const rules = useQuery({
    queryKey: rulesKey(threadId),
    queryFn: ({ signal }): Promise<ThreadApprovalRule[]> =>
      ok(api.GET('/threads/{id}/approval-rules', { params: { path: { id: threadId }, query: {} }, signal })).then((page) => page.items),
    enabled: writable,
  });
  const revoke = useMutation({
    mutationFn: (ruleId: string) => ok(api.DELETE('/threads/{id}/approval-rules/{rule_id}', { params: { path: { id: threadId, rule_id: ruleId } } })),
  });
  // Only whoever manages the thread reads its rules; nobody else is shown an empty list as if there were none.
  if (!writable || rules.data === undefined) return null;
  return (
    <section aria-label={t('approvals.rules.title')} className="flex flex-col gap-2 border-t border-border px-3 py-3" data-approval-rules>
      <h3 className="text-sm font-medium text-text">{t('approvals.rules.title')}</h3>
      <GuideLink guide="how-approvals-work" target={{ kind: 'node', id: threadId }} />
      <p className="text-xs text-text-muted">{t('approvals.rules.lead')}</p>
      {rules.data.length === 0 ? (
        <EmptyState compact title={t('approvals.rules.none')} />
      ) : (
        <ul className="flex flex-col gap-2">
          {rules.data.map((rule) => (
            <li key={rule.id} className="flex flex-col gap-1 text-sm">
              <span className="font-mono text-xs wrap-anywhere text-text">{matcherWords(rule.matcher, t)}</span>
              <span className="flex flex-wrap items-center gap-x-2 text-xs text-text-muted">
                {rule.by.display_name} · <RelativeTime at={rule.at} />
                {!rule.active && <span>{t('approvals.rules.inactive')}</span>}
              </span>
              {rule.active && (
                <Button size="sm" variant="secondary" className="self-start" pending={revoke.isPending && revoke.variables === rule.id} disabled={revoke.isPending} onClick={() => revoke.mutate(rule.id)}>
                  {t('approvals.rules.takeBack')}
                </Button>
              )}
            </li>
          ))}
        </ul>
      )}
      {revoke.error !== null && <ErrorAlert error={revoke.error} />}
    </section>
  );
}
