/** One explanation of actual sandbox capability, limits and permitted recovery acts, before and after binding. */
import { Link } from '@tanstack/react-router';
import type { TFunction } from 'i18next';
import { X } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { EnvironmentSandboxReadiness, SandboxReadiness } from '@/generated/api-types';
import { executorTitle } from '@/features/resources/executorNames';
import { currentLocale } from '@/i18n';
import { InlineAlert } from '@/ui/InlineAlert';
import { HelpHint } from '@/ui/HelpHint';
import { IconButton } from '@/ui/IconButton';

type Mode=SandboxReadiness['requested'];
const ORDER:Record<Mode,number>={read_only:0,workspace:1,open:2};

function needed(value:EnvironmentSandboxReadiness) {
  const report=value.sandbox;
  const usable=report.enforceable.filter((mode)=>report.agent_modes.length===0 || report.agent_modes.includes(mode));
  return usable.reduce<Mode|null>((lowest,mode)=>lowest===null || ORDER[mode]<ORDER[lowest]?mode:lowest,null);
}

/** The same words appear in the machine's readiness and the blocked thread's status. */
export function sandboxWords(value:EnvironmentSandboxReadiness,t:TFunction):string {
  const report=value.sandbox;
  const name=value.machine?.name ?? value.environment.name;
  const reference='executor_ref' in report?report.executor_ref:null;
  const executor=report.executor===null?t('sandboxReadiness.nativeTools'):executorTitle(reference,currentLocale()) ?? t('sandboxReadiness.executorUnknown');
  const mode=(mode:Mode)=>t(`sandboxReadiness.modes.${mode}.name`);
  const capability=report.enforceable.length===0?t('sandboxReadiness.unreported',{name,executor}):
    report.enforceable.length===1 && report.enforceable[0]==='open'?t('sandboxReadiness.unconfined',{name,executor}):
      t('sandboxReadiness.enforces',{name,executor,modes:report.enforceable.map(mode).join(', ')});
  const limit=t('sandboxReadiness.limit',{requested:mode(report.requested),widest:mode(report.widest),source:t(`sandboxReadiness.sources.${report.source}.name`)});
  const templateLimit=report.template_limit;
  const template=templateLimit===null?'':t('sandboxReadiness.templateLimit',{mode:mode(templateLimit)});
  if (report.ready) return `${capability} ${limit} ${template}`.trim();
  const target=needed(value);
  if (target===null) return `${capability} ${limit} ${t(report.enforceable.length===0?'sandboxReadiness.probe':'sandboxReadiness.changeAgent')}`;
  const acts:string[]=[];
  if (ORDER[report.requested]<ORDER[target]) acts.push(t('sandboxReadiness.changePlace',{mode:mode(target)}));
  if (ORDER[report.machine_limit]<ORDER[target]) acts.push(t('sandboxReadiness.changeMachine',{mode:mode(target)}));
  if (templateLimit!==null && ORDER[templateLimit]<ORDER[target]) acts.push(t('sandboxReadiness.changeTemplate',{mode:mode(target)}));
  acts.push(t('sandboxReadiness.alternative',{mode:mode(report.requested)}));
  return `${capability} ${limit} ${template} ${acts.join(' ')}`.trim();
}

/** Links only offer machine management when the server grants that authority. */
export function SandboxActions({value}:{value:EnvironmentSandboxReadiness}) {
  const {t}=useTranslation();
  if (value.sandbox.ready) return null;
  const target=needed(value);
  const machine=value.machine;
  const policy=target!==null && ORDER[value.sandbox.machine_limit]<ORDER[target] && value.policy_manage && machine!==null;
  return <div className="flex flex-wrap items-center gap-3 text-sm">
    {policy && <Link to="/resources/machines/$id" params={{id:machine.id}} search={{tab:'policy'}} className="text-accent underline">{t('sandboxReadiness.openPolicy',{name:machine.name})}</Link>}
    <Link to="/resources/environments/$id" params={{id:value.environment.id}} className="text-accent underline">{t('sandboxReadiness.openEnvironment')}</Link>
  </div>;
}

export function SandboxReadinessNotice({value}:{value:EnvironmentSandboxReadiness}) {
  const {t}=useTranslation();
  return <InlineAlert tone={value.sandbox.ready?'success':'warning'} title={t(value.sandbox.ready?'sandboxReadiness.ready':'sandboxReadiness.blocked',{name:value.environment.name})}
    action={<SandboxActions value={value} />}>
    <p>{sandboxWords(value,t)}</p>
    {value.sandbox.reason!==null && <p className="text-text-muted">{value.sandbox.reason}</p>}
  </InlineAlert>;
}

/** One line per environment, outside the machine's tabs. A changed boundary can be shown again;
 * repeated observations of the same boundary do not undo the reader's dismissal.
 */
export function MachineSandboxReadiness({ values }: { values: EnvironmentSandboxReadiness[] }) {
  const { t } = useTranslation();
  const [dismissed, setDismissed] = useState<Record<string, string>>({});
  const environments = new Map<string, EnvironmentSandboxReadiness>();
  for (const value of values) {
    const previous = environments.get(value.environment.id);
    if (previous === undefined || !value.sandbox.ready) environments.set(value.environment.id, value);
  }
  const visible = [...environments.values()].map((value) => ({ value, signature: JSON.stringify({ ...value.sandbox,
    enforceable: [...value.sandbox.enforceable].sort(), agent_modes: [...value.sandbox.agent_modes].sort(), policy_manage: value.policy_manage }) }))
    .filter(({ value, signature }) => dismissed[value.environment.id] !== signature);
  if (visible.length === 0) return null;
  return <ul className="flex flex-col gap-1 text-sm" aria-label={t('sandboxReadiness.summary')}>
    {visible.map(({ value, signature }) => <li key={value.environment.id} className="flex items-center gap-2">
      <Link to="/resources/environments/$id" params={{ id: value.environment.id }} className="min-w-0 flex-1 truncate text-accent underline">{value.environment.name}</Link>
      <span className={value.sandbox.ready ? 'shrink-0 whitespace-nowrap text-text-muted' : 'shrink-0 whitespace-nowrap text-warning-text'}>{t(value.sandbox.ready ? 'sandboxReadiness.modeReady' : 'sandboxReadiness.modeBlocked', { mode: t(`sandboxReadiness.modes.${value.sandbox.requested}.name`) })}</span>
      <HelpHint term={value.environment.name} what={<>{sandboxWords(value, t)}{value.sandbox.reason === null ? null : <p>{value.sandbox.reason}</p>}</>} where={value.sandbox.ready ? undefined : <SandboxActions value={value} />} />
      <IconButton size="sm" icon={<X aria-hidden />} label={t('sandboxReadiness.dismiss', { name: value.environment.name })} onClick={() => setDismissed((previous) => ({ ...previous, [value.environment.id]: signature }))} />
    </li>)}
  </ul>;
}
