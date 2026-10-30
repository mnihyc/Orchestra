/** Waiting, requested and history views.
 *
 * Everything that waits for the person, of the three kinds (what an agent asks leave for, what an
 * environment asks to be given, what somebody asks access to), decided from the list with the same choices
 * as the card in a conversation; what they asked for, waiting and answered; and what was decided. A toggle
 * lists everything the person could decide, which takes in what was sent to people nearer the thing.
 */
import { ShieldCheck } from 'lucide-react';
import { APPROVAL_TABS, type ApprovalsTab } from '@/features/approvals/tabs';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';
import { DecidedList, RequestedWaitingList, type ListShape, WaitingList } from '@/features/approvals/ApprovalList';
import { ApprovalHeadings } from '@/features/approvals/Card';
import { useUserFeed } from '@/realtime/live';
import { useCounters } from '@/realtime/stores/user';
import { Page, PageHeader, PlaceMark, Section } from '@/shell/Page';
import { useScreenHeading } from '@/shell/screen';
import { Select } from '@/ui/Select';
import { Tabs } from '@/ui/Tabs';
import type { ApprovalKind } from '@/generated/api-types';

export { APPROVAL_TABS, type ApprovalsTab };

export interface ApprovalsProps {
  tab: ApprovalsTab;
  onTab: (tab: ApprovalsTab) => void;
  /** Everything the person could decide, not only what was sent to them. */
  all: boolean;
  onAll: (all: boolean) => void;
  /** Only this kind. */
  kind: ApprovalKind | null;
  onKind: (kind: ApprovalKind | null) => void;
}

export function Approvals({ tab, onTab, all, kind, onKind }: ApprovalsProps) {
  const { t } = useTranslation();
  useUserFeed(false);
  useScreenHeading({ title: t('screens.approvals.title'), icon: ShieldCheck });
  const { approvalsPending } = useCounters();
  const only = kind === null ? {} : { kind };

  const waiting: ListShape = {
    filter: { scope: 'mine', state: 'pending', ...only },
    empty: { title: t('approvals.empty.waiting.title'), description: t('approvals.empty.waiting.description') },
  };
  const others: ListShape = {filter:{scope:'others',state:'pending',...only},empty:{title:t('approvals.empty.others'),description:''}};
  const asked: ListShape = {
    filter: { scope: 'requested', state: 'pending', ...only },
    empty: { title: t('approvals.empty.requested.title'), description: t('approvals.empty.requested.description') },
  };
  const answered: ListShape = {
    filter: { scope: 'requested', state: 'decided', ...only },
    empty: { title: t('approvals.empty.requested.title'), description: t('approvals.empty.requested.description') },
  };
  const history: ListShape = {
    filter: { scope: 'mine', state: 'decided', ...(all ? { all: true } : {}), ...only },
    empty: { title: t('approvals.empty.history.title'), description: t('approvals.empty.history.description') },
  };

  return (
    <Page>
      <PageHeader
        mark={
          <PlaceMark>
            <ShieldCheck aria-hidden />
          </PlaceMark>
        }
        title={t('screens.approvals.title')}
        actions={
          <Select
            size="sm"
            label={t('approvals.kindFilter')}
            value={kind ?? 'all'}
            onValueChange={(next) => onKind(next === 'all' ? null : next)}
            options={[
              { value: 'all', ...choiceOption(t, 'approvalKind.all') },
              { value: 'harness', ...choiceOption(t, 'approvalKind.harness') },
              { value: 'capability', ...choiceOption(t, 'approvalKind.capability') },
              { value: 'access', ...choiceOption(t, 'approvalKind.access') },
              { value: 'port', ...choiceOption(t, 'approvalKind.port') },
              { value: 'plan', ...choiceOption(t, 'approvalKind.plan') },
              { value: 'input', ...choiceOption(t, 'approvalKind.input') },
            ]}
          />
        }
      />
      <Tabs
        label={t('approvals.tabs.label')}
        value={tab}
        onValueChange={onTab}
        tabs={[
          {
            value: 'waiting',
            label: t('approvals.tabs.waiting'),
            count: approvalsPending,
            panel: (
              <div className="flex flex-col gap-4 pt-4">
                <ApprovalHeadings level={2}><WaitingList key={`waiting-${kind ?? ''}`} shape={waiting} /></ApprovalHeadings>
                <Section id="others-you-may-decide" title={t('approvals.others')}><ApprovalHeadings level={3}><WaitingList key={`others-${kind ?? ''}`} shape={others} /></ApprovalHeadings></Section>
              </div>
            ),
          },
          {
            value: 'requested',
            label: t('approvals.tabs.requested'),
            panel: (
              <div className="flex flex-col gap-6 pt-4">
                <Section id="requested-waiting" title={t('approvals.sections.waiting')}>
                  <RequestedWaitingList key={`asked-${kind ?? ''}`} shape={asked} />
                </Section>
                <Section id="requested-answered" title={t('approvals.sections.answered')}>
                  <DecidedList key={`answered-${kind ?? ''}`} shape={answered} />
                </Section>
              </div>
            ),
          },
          {
            value: 'history',
            label: t('approvals.tabs.history'),
            panel: (
              <div className="pt-4">
                <DecidedList key={`history-${String(all)}-${kind ?? ''}`} shape={history} />
              </div>
            ),
          },
        ]}
      />
    </Page>
  );
}
