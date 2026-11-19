/** Current Ports policies and refused acts, with the deciding places and holders the server supplied. */
import { Link } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';
import { ActionIcon } from '@/ui/actions';
import { useViewChoice } from '@/lib/viewChoices';
import type { Holder, NodeRef } from '@/generated/api-types';
import { holderName } from '@/features/access/holders';
import { pathFor } from '@/features/access/target';
import { HolderChip } from '@/ui/HolderChip';
import { InlineAlert } from '@/ui/InlineAlert';
import type { EnvironmentPorts, PortReason } from './presentation';

export function PortScope({ scope }: { scope: NodeRef }) {
  const path = pathFor(scope);
  return path === null ? <span>{scope.name}</span> : <Link to={path} className="text-accent-text hover:underline">{scope.name}</Link>;
}

export function PortHolders({ holders }: { holders: readonly Holder[] }) {
  const { t } = useTranslation();
  return <ul className="flex flex-wrap gap-1">{holders.map((holder, index) => {
    const name = holderName(t, holder, holder.kind === 'role' ? holder.at?.name ?? '' : '');
    return <li key={index}><HolderChip name={name} {...(holder.kind === 'role' ? { count: holder.count } : {})} /></li>;
  })}</ul>;
}

export function PortGate({ reason }: { reason: PortReason }) {
  const { t } = useTranslation();
  return <InlineAlert tone="warning" title={reason.message}>
    <span className="flex flex-col gap-1">
      {reason.scope !== null && <span>{t('ports.decidedAt')} <PortScope scope={reason.scope} /></span>}
      {reason.holders.length > 0 && <span>{t('ports.ask')}<PortHolders holders={reason.holders} /></span>}
    </span>
  </InlineAlert>;
}

export function PortsPolicy({ data }: { data: EnvironmentPorts }) {
  const { t } = useTranslation();
  const [open, setOpen] = useViewChoice<boolean>('ports', 'policy', false);
  const reasons = data.reasons.filter((reason, index, all) => all.findIndex((other) => other.message === reason.message && other.scope?.id === reason.scope?.id && JSON.stringify(other.holders) === JSON.stringify(reason.holders)) === index);
  return <details open={open} onToggle={(event) => setOpen(event.currentTarget.open)} className="rounded-lg border border-border p-3">
    <summary className="cursor-pointer text-sm font-medium text-text inline-flex items-center gap-2"><ActionIcon action={open ? 'hide' : 'show'} />{t('ports.policy.title')}</summary>
    <dl className="mt-3 grid gap-2 text-sm sm:grid-cols-2">
      {Object.entries(data.policy).map(([name, setting]) => <div data-item key={name} className="min-w-0 rounded-lg bg-bg-subtle p-2">
        <dt className="text-text-muted">{t(`ports.policy.${name as keyof EnvironmentPorts['policy']}`)}</dt>
        <dd className="flex flex-wrap items-baseline gap-x-2 text-text"><strong>{setting.value === null ? t('ports.unreported') : typeof setting.value === 'boolean' ? t(setting.value ? 'ports.on' : 'ports.off') : typeof setting.value === 'number' || typeof setting.value === 'string' ? String(setting.value) : t('ports.unreported')}</strong>
          {setting.value !== null && (setting.source.node === null ? <span>{t('ports.system')}</span> : <PortScope scope={setting.source.node} />)}
        </dd>
      </div>)}
    </dl>

    {reasons.map((reason, index) => <div key={index} className="mt-2"><PortGate reason={reason} /></div>)}
  </details>;
}
