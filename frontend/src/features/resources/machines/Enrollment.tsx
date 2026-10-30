/** Platform installer command and enrollment progress.
 *
 * Each command belongs to one role/handover selection. Changing a choice replaces the token;
 * the dialog never appends permissions to an already issued command.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { LoaderCircle } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { createHostToken, createMachineToken, hostKey, machineEnrollmentInfoQuery, machineKey } from '@/api/resources';
import { enrollmentProgressQuery } from '@/api/enrollment';
import { applyRow } from '@/api/cache';
import { onUserEvent } from '@/realtime/stores/user';
import type { EnrollmentToken, MachineOs,NewResourceAccess } from '@/generated/api-types';
import { CommandBox } from '@/features/resources/Facts';
import { TaskGuideLink } from '@/features/help/TaskGuideLink';
import { HomePicker,useHomeChoices } from '@/features/resources/HomePicker';
import { NewAccess } from '@/features/resources/NewAccess';
import { CONNECTOR_ROLES, EnrollmentRoles, type ConnectorRole } from '@/features/resources/machines/EnrollmentRoles';
import { OS_NAMES } from '@/features/resources/machines/facts';
import { newActKey } from '@/lib/requests';
import { MINUTE_MS, useNow } from '@/lib/time';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert,ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { Tabs } from '@/ui/Tabs';
import { toast } from '@/ui/Toast';

/** The browser's platform is the initial connector command; a node's command starts on Linux. */
export function guessOs(agent: string): MachineOs {
  const text = agent.toLowerCase();
  if (text.includes('win')) return 'windows';
  if (text.includes('mac') || text.includes('iphone') || text.includes('ipad')) return 'macos';
  return 'linux';
}

/** Commands already contain the exact choices authenticated by their token. */
export interface InstallOptions {foreground:boolean;instance:string;node:boolean}
export function commandFor(token: EnrollmentToken, os: MachineOs,options?:InstallOptions): string {
  let command=token.commands[os];
  if(options===undefined) return command;
  if(options.node && os!=='linux') return command;
  const instance=/^[a-zA-Z0-9_-]+$/u.test(options.instance)?options.instance:'';
  const root=os==='macos'?'$HOME/Library/Application Support/Orch/connector':'${XDG_DATA_HOME:-$HOME/.local/share}/orch-connector';
  const data=os==='windows'?`(Join-Path $env:LOCALAPPDATA 'Orch/connector${instance===''?'':`/instances/${instance}`}')`
    :'"'+root+(instance===''?'':'/instances/'+instance)+'"';
  if(options.foreground) command+=os==='windows'?' -NoService':' --no-service';
  if(!options.node && instance!=='') command+=(os==='windows'?' -DataDir ':' --data-dir ')+data;
  // --no-service remains an install-only switch for scripts. The screen's Foreground choice
  // completes the workflow in this same command, without requiring a second manual step.
  if(options.foreground) {
    if(options.node) command+=' && /usr/local/bin/orch-node --config /etc/orch/orch.toml run';
    else if(os==='windows') command+=`; if ($?) { & (Join-Path $env:LOCALAPPDATA 'Orch/orch-connector.exe') --data-dir ${data} run }`;
    else command+=' && "$HOME/.local/bin/orch-connector" --data-dir '+data+' run';
  }
  return command;
}

type Selection =
  | { kind: 'connector'; home: string; roles: ConnectorRole[]; delegate: ConnectorRole[]; dedicated: boolean }
  | { kind: 'node'; home: string; access:NewResourceAccess; delegate_policy: boolean };

interface Made {
  kind: Selection['kind'];
  stamp: string;
  token: EnrollmentToken;
}

/** One open dialog owns its choices, command and progress; closing it retains no token in application state. */
export function AddMachineDialog({ onClose,initialKind='connector' }: { onClose: () => void;initialKind?:'connector'|'node' }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const opened = useRef(false);
  const now = useNow();
  const [roles, setRoles] = useState<ConnectorRole[]>(['environment', 'agent']);
  const [delegated, setDelegated] = useState<ConnectorRole[]>([]);
  const [node, setNode] = useState(initialKind==='node');
  const [nodeDelegated, setNodeDelegated] = useState(false);
  const [dedicated, setDedicated] = useState(false);
  const [home, setHome] = useState('');
  const [access,setAccess]=useState<NewResourceAccess>('defaults');
  const homeName=useHomeChoices(home).choices.find((choice)=>choice.id===home)?.label ?? '';
  const [os, setOs] = useState<MachineOs>(() => guessOs(typeof navigator === 'undefined' ? '' : navigator.userAgent));
  const [foreground,setForeground]=useState(false);
  const [instance,setInstance]=useState('');
  const info=useQuery(machineEnrollmentInfoQuery);
  const instanceValid=node || instance==='' || /^[a-zA-Z0-9_-]+$/u.test(instance);
  const selection = useMemo<Selection>(() => node
    ? { kind: 'node', home, access,delegate_policy: nodeDelegated }
    : { kind: 'connector', home, roles: CONNECTOR_ROLES.filter((role) => roles.includes(role)),
        delegate: CONNECTOR_ROLES.filter((role) => roles.includes(role) && delegated.includes(role)), dedicated },
  [node, home,access, nodeDelegated, roles, delegated, dedicated]);
  const stamp = JSON.stringify(selection);
  const valid = selection.kind === 'node' ? selection.home !== '' : selection.roles.length > 0;

  const make = useMutation({
    gcTime:0,
    mutationFn: async (selected: Selection): Promise<Made> => {
      const token = selected.kind === 'node'
        ? await createHostToken({ home_id: selected.home, access:selected.access, delegate_policy: selected.delegate_policy }, newActKey())
        : await createMachineToken(newActKey(), { roles: selected.roles, delegate: selected.delegate, dedicated: selected.dedicated,
            ...(selected.home === '' ? {} : { home_id: selected.home }) });
      return { kind: selected.kind, stamp: JSON.stringify(selected), token };
    },
  });
  const { mutate } = make;
  const made = make.data?.stamp === stamp ? make.data : undefined;
  const receipt = made?.token.receipt ?? undefined;
  const progress = useQuery({ ...enrollmentProgressQuery(receipt ?? ''), enabled: receipt !== undefined });
  const refetchProgress = progress.refetch;
  useEffect(() => {
    if (receipt === undefined) return;
    return onUserEvent('connected', () => { void refetchProgress(); });
  }, [receipt, refetchProgress]);
  const status = !progress.isError && progress.data?.receipt === receipt ? progress.data : undefined;
  const machine = made?.kind === 'connector' ? status?.machine ?? undefined : undefined;
  const host = made?.kind === 'node' ? status?.host ?? undefined : undefined;
  useEffect(() => {
    if (machine !== undefined) applyRow(queryClient, { kind: 'machine', id: machine.id, value: { ...machine }, insert: true });
    if (host !== undefined) applyRow(queryClient, { kind: 'host', id: host.id, value: { ...host }, insert: true });
  }, [machine, host, queryClient]);
  const expiresAt = made === undefined ? null : Date.parse(made.token.expires_at);
  const expired = expiresAt !== null && expiresAt <= now;
  const arrived = host ?? machine;
  const connected = host?.machine.online ?? machine?.online ?? false;
  const minutesLeft = expiresAt === null || expiresAt <= now ? 0 : Math.max(1, Math.floor((expiresAt - now) / MINUTE_MS));
  const failure = make.isError && JSON.stringify(make.variables) === stamp ? make.error : undefined;

  useEffect(() => {
    if (arrived === undefined || !connected || opened.current) return;
    opened.current = true;
    if (host !== undefined) queryClient.setQueryData(hostKey(host.id), host);
    else if (machine !== undefined) queryClient.setQueryData(machineKey(machine.id), machine);
    toast({ tone: 'success', title: t(host === undefined ? 'machines.add.connected' : 'hosts.add.connected') });
    onClose();
    void navigate({ to: host === undefined ? '/resources/machines/$id' : '/resources/hosts/$id', params: { id: arrived.id } });
  }, [arrived, connected, host, machine, navigate, onClose, queryClient, t]);

  const mayShow=info.data?.secure===true && !info.isError && valid && instanceValid;
  const install={foreground,instance,node};
  const directory=node?'/var/lib/orch-node':os==='windows'?'%LOCALAPPDATA%\\Orch\\connector':os==='macos'?'~/Library/Application Support/Orch/connector':'~/.local/share/orch-connector';
  const data=instance!=='' && !node?`${directory}/instances/${instance}`:directory;
  const bin=os==='windows'?'%LOCALAPPDATA%\\Orch':'~/.local/bin';
  // Keep the act that opened the dialog; role choices change the installer, not its name.
  const title = t(initialKind === 'node' ? 'hosts.add.title' : 'machines.add.title');
  return <Dialog open onOpenChange={(next) => !next && !make.isPending && onClose()} size="lg"
    title={title}
    footer={<><TaskGuideLink guide="connect-a-machine" heading={t('machineInstallation.connectHeading')} /><Button variant="secondary" onClick={onClose}>
      {t('common.close')}
    </Button>{arrived===undefined && (made===undefined || expired) && <Button variant="primary" pending={make.isPending} disabled={!mayShow} onClick={()=>mutate(selection)}>{t('machineInstallation.showCommand')}</Button>}</>}>
    <div className="flex flex-col gap-4 py-2" data-tutorial-target="machine-enrollment">
      {info.isPending && <SkeletonRows rows={1}/>}
      {info.isError && <ErrorState compact error={info.error} retrying={info.isRefetching} onRetry={()=>void info.refetch()}/>}
      {progress.isError && <ErrorState compact error={progress.error} retrying={progress.isRefetching} onRetry={()=>void progress.refetch()}/>}
      {info.data?.secure===false && <InlineAlert tone="warning" action={<Button size="sm" onClick={()=>void info.refetch()}>{t('machineInstallation.retry')}</Button>}>{t('machineInstallation.configureAddress',{setting:info.data.configure ?? 'public_url'})}</InlineAlert>}
      {failure!==undefined && <ErrorAlert error={failure}/>}
      {made!==undefined && expired && <p className="text-sm text-text-muted">{t('machines.add.expired')}</p>}
      {made!==undefined && !expired && mayShow && !progress.isError && arrived===undefined && <>
        {node?<CommandBox command={commandFor(made.token,'linux',install)} label={t('machines.add.copy')}/>:<Tabs label={t('machines.add.systems')} value={os} onValueChange={setOs}
          tabs={(['linux','macos','windows'] as const).map((value)=>({value,label:OS_NAMES[value],panel:<CommandBox command={commandFor(made.token,value,install)} label={t('machines.add.copy')}/>}))}/>}
        <p className="text-sm text-text-muted">{t('machines.add.validFor',{count:minutesLeft})}</p>
        <p role="status" className="flex items-center gap-2 text-sm text-text-muted"><LoaderCircle className="animate-spin" aria-hidden/>{t('machines.add.waiting')}</p>
      </>}
      {arrived === undefined && <>
        <p className="text-sm wrap-anywhere text-text-muted">{t(node?'machineInstallation.nodeSummary':'machineInstallation.connectorSummary',{data,bin})}</p>
        <Select label={t('machineInstallation.runMode')} value={foreground?'foreground':'service'} onValueChange={(value)=>setForeground(value==='foreground')}
          options={[{value:'service',name:t(node?'machineInstallation.systemService.name':'machineInstallation.userService.name'),description:t(node?'machineInstallation.systemService.description':'machineInstallation.userService.description')},
            {value:'foreground',name:t('machineInstallation.foreground.name'),description:t('machineInstallation.foreground.description')}]}/>
        {!node && <Input label={t('machineInstallation.instance')} optional error={!instanceValid?t('machineInstallation.instanceInvalid'):undefined}
          value={instance} onChange={(event)=>setInstance(event.target.value)} autoComplete="off"/>}
        <EnrollmentRoles roles={roles} delegated={delegated} node={node} nodeDelegated={nodeDelegated}
          onRole={(role, checked) => {
            if (node && checked) { setNode(false); setRoles([role]); }
            else setRoles((current) => checked ? [...current.filter((entry) => entry !== role), role] : current.filter((entry) => entry !== role));
            if (!checked) setDelegated((current) => current.filter((entry) => entry !== role));
          }}
          onDelegate={(role, checked) => setDelegated((current) => checked ? [...current.filter((entry) => entry !== role), role] : current.filter((entry) => entry !== role))}
          onNode={(checked) => { setNode(checked); if (checked) setOs('linux'); }}
          onNodeDelegate={setNodeDelegated} />
        <HomePicker value={home} onChange={setHome} preferPersonal={!node} hint={t(node ? 'hosts.add.homeHint' : 'machines.add.homeHint')} />
        {node && <NewAccess value={access} onChange={setAccess} home={homeName} disabled={make.isPending}/>}
        {!node && <Switch name={t('machines.policy.fullControl')}
          description={t('machines.policy.fullControlDescription')}
          checked={dedicated} onCheckedChange={setDedicated} />}
      </>}
      {!valid && <p className="text-sm text-text-muted">{t(node?'machines.add.chooseHome':'machines.add.chooseRole')}</p>}
      {arrived !== undefined && !connected && <InlineAlert tone="info" title={t('machines.add.arrived', { name: arrived.name })}>
        {t('machines.add.waiting')}
      </InlineAlert>}
    </div>
  </Dialog>;
}
