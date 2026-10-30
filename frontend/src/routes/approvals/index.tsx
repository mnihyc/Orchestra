/** Approval channel tabs. Loader, permission handling and search schema are pending.
 *
 * Everything that waits for the person, what they asked for, and what was decided. The tab that is open,
 * the kind it is limited to and whether everything the person could decide is listed are in the address, so
 * that a link reproduces them.
 */
import { usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';
import { Approvals } from '@/features/approvals/Approvals';
import { APPROVAL_TABS, type ApprovalsTab } from '@/features/approvals/tabs';
import { ApprovalDetail } from '@/features/approvals/ApprovalList';
import type { ApprovalKind } from '@/generated/api-types';
import { useRememberedView } from '@/lib/viewChoices';

interface ApprovalsSearch {
  approval?: string;
  tab?: ApprovalsTab;
  kind?: ApprovalKind|'all';
  all?: boolean;
}

const KINDS: readonly ApprovalKind[] = ['harness', 'capability', 'access', 'port','plan','input'];

export const Route = createFileRoute('/approvals/')({
  staticData: { screen: 'page' },
  validateSearch: (search: Record<string, unknown>): ApprovalsSearch => {
    const found: ApprovalsSearch = {};
    if (typeof search.approval === 'string' && search.approval.trim() !== '') found.approval = search.approval;
    if (APPROVAL_TABS.includes(search.tab as ApprovalsTab)) found.tab = search.tab as ApprovalsTab;
    if(search.kind==='all'||KINDS.includes(search.kind as ApprovalKind))found.kind=search.kind as ApprovalKind|'all';
    if(search.all===true||search.all==='true')found.all=true;else if(search.all===false||search.all==='false')found.all=false;
    return found;
  },
  component: ApprovalsRoute,
});

function ApprovalsRoute() {
  const search=usePageSearch(Route);
  const {approval}=search;
  const [tab,keepTab]=useRememberedView('approvals','tab',search.tab,'waiting',APPROVAL_TABS);
  const [kind,keepKind]=useRememberedView<ApprovalKind|'all'>('approvals','kind',search.kind,'all',['all',...KINDS]);
  const [all,keepAll]=useRememberedView('approvals','all',search.all===undefined?undefined:search.all?'all':'sent','sent',['sent','all']);
  const navigate = usePageNavigate(Route);
  const change=(patch:Partial<{tab:ApprovalsTab;kind:ApprovalKind|null;all:boolean}>)=>{
    const next={tab:patch.tab??tab,kind:patch.kind===undefined?kind:patch.kind??'all',all:patch.all??(all==='all')};
    keepTab(next.tab);keepKind(next.kind);keepAll(next.all?'all':'sent');
    void navigate({search:{tab:next.tab,kind:next.kind,all:next.all},replace:true});
  };
  if (approval !== undefined) return <ApprovalDetail key={approval} id={approval} />;
  return (
    <Approvals
      tab={tab}
      onTab={(next) => change({ tab: next })}
      all={all==='all'}
      onAll={(next) => change({ all: next })}
      kind={kind==='all'?null:kind}
      onKind={(next) => change({ kind: next })}
    />
  );
}
