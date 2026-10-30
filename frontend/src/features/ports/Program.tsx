/** A port's shared program options can be repaired without changing its admitted audience. */
import { Save, X } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';
import { ActionIcon } from '@/ui/actions';
import { useViewChoice } from '@/lib/viewChoices';
import { updatePortProgram } from '@/api/ports';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Dialog } from '@/ui/Dialog';
import { HelpHint } from '@/ui/HelpHint';
import { Input } from '@/ui/Input';
import { RadioGroup } from '@/ui/RadioGroup';
import { PortFault } from './Fault';
import type { EnvironmentPort, PortHttp } from './presentation';
import { programFor } from './wire';

export const DEFAULT_PROGRAM: PortHttp = { scheme: 'http', host_header: null, proxy_protocol: false };
export function validProgram(value: PortHttp): boolean {
  if (value.host_header === null) return true;
  const port = /:(\d+)$/.exec(value.host_header)?.[1];
  return /^(?:[a-z\d](?:[a-z\d.-]*[a-z\d])?|\[[a-f\d:]+\])(?::\d{1,5})?$/i.test(value.host_header) && (port === undefined || Number(port) >= 1 && Number(port) <= 65535);
}
export function ProgramFields({ value, onChange, affected = [], tcp = true }: { value: PortHttp; onChange: (next: PortHttp) => void; affected?: string[]; tcp?: boolean }) {
  const { t } = useTranslation();
  const [open, setOpen] = useViewChoice<boolean>('ports', 'program', false);
  return <details open={open} onToggle={(event) => setOpen(event.currentTarget.open)} className="rounded-lg border border-border p-3"><summary className="cursor-pointer text-sm font-medium inline-flex items-center gap-2"><ActionIcon action={open ? 'hide' : 'show'} />{t('ports.programTitle')}</summary><div className="mt-3 flex flex-col gap-3">
    <RadioGroup<'http' | 'https'> label={t('ports.scheme')} value={value.scheme} onValueChange={(scheme) => onChange({ ...value, scheme })} variant="list" options={[{ value: 'http', ...choiceOption(t, 'programScheme.http') }, { value: 'https', ...choiceOption(t, 'programScheme.https'), description: t('ports.tlsInside') }]} />
    <div className="flex items-center gap-1 text-sm">{t('ports.hostHeader')}<HelpHint term={t('ports.hostHeader')} what={t('ports.hostHeaderHint')} /></div>
    <RadioGroup<'caller' | 'custom'> label={t('ports.hostHeader')} value={value.host_header === null ? 'caller' : 'custom'} onValueChange={(choice) => onChange({ ...value, host_header: choice === 'caller' ? null : '' })} variant="list" options={[{ value: 'caller', ...choiceOption(t, 'hostHeader.caller') }, { value: 'custom', ...choiceOption(t, 'hostHeader.custom') }]} />
    {value.host_header !== null && <Input label={t('ports.customHostValue')} value={value.host_header} onChange={(event) => onChange({ ...value, host_header: event.target.value })} error={!validProgram(value) ? t('ports.invalidHost') : undefined} />}
    {tcp && <Checkbox label={t('ports.proxyHeader')} checked={value.proxy_protocol === true} onCheckedChange={(proxy_protocol) => onChange({ ...value, proxy_protocol })} hint={t('ports.proxyHelp')} />}
    {!tcp && <p className="text-xs text-text-muted">{t('ports.udpNoProxy')}</p>}
    {affected.length > 0 && <p className="break-words text-sm text-warning-text">{t('ports.affectsOpenings', { names: affected.join(', ') })}</p>}
  </div></details>;
}
export function ProgramDialog({ environmentId, port, onClose, onChanged }: { environmentId: string; port: EnvironmentPort; onClose: () => void; onChanged: () => void }) {
  const { t } = useTranslation(); const [value, setValue] = useState(port.program ?? DEFAULT_PROGRAM); const [busy, setBusy] = useState(false); const [error, setError] = useState<unknown>(null);
  const save = async () => { if (busy || !validProgram(value) || port.revision === undefined) return; setBusy(true); setError(null); try { await updatePortProgram(environmentId, port.port, programFor(value), port.revision); onChanged(); onClose(); } catch (failed) { setError(failed); } finally { setBusy(false); } };
  return <Dialog open title={t('ports.editProgram')} description={t('ports.programOnly')} onOpenChange={(open) => { if (!open) onClose(); }} dismissible={!busy} footer={<><Button icon={<X aria-hidden />} pending={busy} onClick={onClose}>{t('ports.cancel')}</Button><Button icon={<Save aria-hidden />} variant="primary" pending={busy} blocked={!validProgram(value) || port.revision === undefined} onClick={() => void save()}>{t('ports.saveProgram')}</Button></>}><ProgramFields value={value} onChange={setValue} affected={port.openings.filter((opening) => opening.desired).map((opening) => opening.label === undefined || opening.label === '' ? opening.place?.name ?? opening.host?.name ?? opening.id : opening.label)} /><PortFault error={error} /></Dialog>;
}
