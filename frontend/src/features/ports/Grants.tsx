/** Who agents may let in without asking: each audience's port capability, granted here, from the template or by a place's policy, with Grant and Take back for those who may. */
import { Check, X } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { grantPortCapability, revokePortCapability } from '@/api/ports';
import { Button } from '@/ui/Button';
import { Badge } from '@/ui/Badge';
import { capabilityName } from '@/features/resources/capabilityWords';
import { Dialog } from '@/ui/Dialog';
import { PortFault } from './Fault';
import { PortScope } from './Policy';
import type { PortCapability } from '@/generated/api-types';
import type { TFunction } from 'i18next';

function unavailable(capability: PortCapability | undefined, act: 'grant' | 'revoke', t: TFunction): string {
  const reason = act === 'grant'
    ? capability !== undefined && 'grant_unavailable_reason' in capability ? capability.grant_unavailable_reason : undefined
    : capability !== undefined && 'revoke_unavailable_reason' in capability ? capability.revoke_unavailable_reason : undefined;
  const words: Record<string, string> = {
    manage_required: 'ports.grants.reasons.manage', not_offered: 'ports.grants.reasons.notOffered',
    policy_denied: 'ports.grants.reasons.policy', already_granted: 'ports.grants.reasons.granted',
    no_grant: 'ports.grants.reasons.noGrant', approver_required: 'ports.grants.reasons.approver',
  };
  return t((typeof reason === 'string' ? words[reason] : undefined) ?? 'ports.grants.reasons.unknown');
}

/** What lets an agent open this audience now, or why it asks, or why nobody opens it. */
export function GrantState({ capability }: { capability: PortCapability }) {
  const { t } = useTranslation();
  const admission = capability.admission;
  if (!capability.offered) return <>{t('ports.grants.notOffered')}</>;
  if (capability.decision === 'deny') return <>{t('ports.grants.denied')}</>;
  if (admission === null) return <>{t('ports.grants.asks')}</>;
  if (admission.source === 'grant') return <>{t('ports.grants.granted', { name: admission.by?.display_name ?? t('ports.unreported') })}</>;
  if (admission.source === 'template') return <>{t('ports.grants.fromTemplate')}</>;
  return <span className="flex flex-wrap items-center gap-x-2 gap-y-1"><span>{t('ports.grants.byPolicy')}</span>{admission.scope === null ? t('ports.system') : <PortScope scope={admission.scope} />}{admission.by !== null && <span>{t('ports.grants.granted', { name: admission.by.display_name })}</span>}</span>;
}

export function PortsGrants({ environmentId, capabilities, onChanged }: { environmentId: string; capabilities: readonly PortCapability[]; onChanged?: () => void }) {
  const { t } = useTranslation();
  const [chosen, setChosen] = useState<{ capability: PortCapability; act: 'grant' | 'revoke' } | null>(null);
  const [busy, setBusy] = useState(false); const [error, setError] = useState<unknown>(null);
  const current = capabilities.find((capability) => capability.key === chosen?.capability.key);
  const available = chosen !== null && (chosen.act === 'grant' ? current?.can_grant === true : current?.can_revoke === true);
  const act = async () => {
    if (chosen === null || busy || !available) return;
    setBusy(true); setError(null);
    try {
      if (chosen.act === 'grant') await grantPortCapability(environmentId, chosen.capability.key);
      else await revokePortCapability(environmentId, chosen.capability.key);
      setChosen(null); onChanged?.();
    } catch (failed) { setError(failed); } finally { setBusy(false); }
  };
  if (capabilities.length === 0) return null;
  const granting = chosen?.act === 'grant';
  return <section aria-label={t('ports.grants.title')} className="flex min-w-0 flex-col gap-2 rounded-lg border border-border p-3" data-testid="port-grants">
    <h3 className="text-sm font-medium">{t('ports.grants.title')}</h3>
    <p className="text-xs text-text-muted">{t('ports.grants.about')}</p>
    <ul className="flex min-w-0 flex-col gap-2">{capabilities.map((capability) => <li key={capability.key} data-testid={`port-grant-${capability.kind}`} className="flex min-w-0 flex-col gap-2 border-t border-border pt-2 sm:flex-row sm:items-start sm:justify-between">
      <div className="min-w-0">
        <div className="flex flex-wrap items-center gap-2 text-sm"><strong>{capabilityName(capability.key, capability.label, t)}</strong><Badge>{t(`approvals.risk.${capability.risk}`)}</Badge></div>
        <p className="text-xs text-text-muted"><GrantState capability={capability} /></p>
      </div>
      <div className="flex flex-wrap gap-2">
        <Button size="sm" blocked={!capability.can_grant} reason={!capability.can_grant ? unavailable(capability, 'grant', t) : undefined} icon={<Check aria-hidden />} onClick={() => { setError(null); setChosen({ capability, act: 'grant' }); }}>{t('ports.grants.grant')}</Button>
        <Button size="sm" variant="danger-ghost" blocked={!capability.can_revoke} reason={!capability.can_revoke ? unavailable(capability, 'revoke', t) : undefined} icon={<X aria-hidden />} onClick={() => { setError(null); setChosen({ capability, act: 'revoke' }); }}>{t('ports.grants.revoke')}</Button>
      </div>
    </li>)}</ul>
    <Dialog open={chosen !== null} onOpenChange={(open) => { if (!open && !busy) setChosen(null); }} dismissible={!busy}
      title={chosen === null ? '' : t(granting ? 'ports.grants.grantTitle' : 'ports.grants.revokeTitle', { label: capabilityName(chosen.capability.key, chosen.capability.label, t) })}
      description={chosen === null ? '' : granting ? t(`ports.grants.grantAbout.${chosen.capability.kind}`) : t('ports.grants.revokeAbout')}
      footer={<><Button onClick={() => setChosen(null)} blocked={busy} reason={busy ? t('ui.unavailable.saving') : undefined}>{t('ports.cancel')}</Button><Button variant={granting ? 'primary' : 'danger'} icon={granting ? <Check aria-hidden /> : <X aria-hidden />} pending={busy} blocked={!available} reason={busy ? t('ui.unavailable.saving') : !available ? unavailable(current, granting ? 'grant' : 'revoke', t) : undefined} onClick={() => void act()}>{t(granting ? 'ports.grants.grant' : 'ports.grants.revoke')}</Button></>}>
      <PortFault error={error} />
    </Dialog>
  </section>;
}
