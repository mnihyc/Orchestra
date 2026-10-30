/** What stops a plan, each with what would lift it; one that is missing access names what is missing and whom to ask.
 *
 * A plan with a blocker cannot be carried out, and the button that does says so. Every blocker is a line in danger
 * style: what is wrong, in words of the page, then what would lift it ("Choose another provider"). Where access is
 * missing, the permission, the thing it is on and the people who can give it are listed as everywhere else a refusal
 * names them. A switch that is already under way for the thread offers to show how far it has come.
 */
import { useTranslation } from 'react-i18next';

import { blockerWords } from '@/features/change/plan';
import type { ForkPlan, PlanBlocker } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { MissingAccessList } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';

export interface BlockersProps {
  blockers: readonly PlanBlocker[];
  plan: { to?: ForkPlan['to'] } | null;
  /** Opens how far the job that holds the thing has come, where one is known. */
  onShow?: (() => void) | undefined;
}

export function Blockers({ blockers, plan, onShow }: BlockersProps) {
  const { t } = useTranslation();
  if (blockers.length === 0) return null;
  return (
    <div className="flex flex-col gap-2" data-blockers>
      {blockers.map((blocker, index) => {
        const words = blockerWords(blocker, plan, t);
        return (
          <InlineAlert
            key={`${blocker.code}-${index}`}
            tone="danger"
            title={words.text}
            {...(blocker.code === 'already_running' && onShow !== undefined
              ? {
                  action: (
                    <Button size="sm" onClick={onShow}>
                      {t('change.blocker.showProgress')}
                    </Button>
                  ),
                }
              : {})}
          >
            <span className="flex flex-col gap-1" data-blocker={blocker.code}>
              {words.lifts !== null && <span>{words.lifts}</span>}
              {blocker.missing !== undefined && blocker.missing !== null && blocker.missing.length > 0 && <MissingAccessList missing={blocker.missing} />}
            </span>
          </InlineAlert>
        );
      })}
    </div>
  );
}
