/** The displays of an environment and one of them in front: the list, "Open a display…", each row's Close with its own words for one Orch opened and one an agent announced, each way of being off and nothing found, and the display drawn by its viewer.
 *
 * Above the display: its name; for one Orch opened, the line a program is started with, to copy; for one
 * it did not open, that what is copied there may reach everybody who watches. "Work in it", for whoever
 * may, is a press apart from watching, as Drive is for a browser; its menu has Paste text…, Send
 * Ctrl+Alt+Del, Fit to this panel and Close…. The same view stands beside a thread and as the VNC tab of
 * the environment's page, following the environment's feed of displays.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Ellipsis, Hand, MonitorPlay, ScreenShare } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { closeDisplay, displayKeys, displaysQuery, lookAgainForDisplays, openDisplay } from '@/api/displays';
import { useBrowserRequests } from '@/features/browser/store';
import type { Display, Environment, EnvironmentDisplays } from '@/generated/api-types';
import { socket } from '@/realtime/live';
import { hostQuery, machinePolicyQuery, updateMachinePolicy } from '@/api/resources';
import { StateLine } from '@/ui/StateLine';
import { AvatarStack } from '@/ui/AvatarStack';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { CopyButton } from '@/ui/CopyButton';
import { cx } from '@/ui/cx';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { SkeletonRows } from '@/ui/Skeleton';
import { StatusDot } from '@/ui/StatusDot';
import { Textarea } from '@/ui/Textarea';

import { displayLine, GEOMETRY, QUALITY_BEST } from './viewer';
import { type RfbConstructor, Viewer, type ViewerControls, type ViewerState } from './Viewer';

/** The programs Orch looks for to serve a display. */
export const SERVERS_LOOKED_FOR = ['Xvnc', 'Xtigervnc'] as const;

function useDisplaysFeed(environmentId: string): void {
  const client = useQueryClient();
  useEffect(() => {
    const subscription = socket.subscribe(`env:${environmentId}:displays`, {
      since: () => null,
      onFrame: (frame) => {
        if (frame.op === 'event' || frame.op === 'sync') void client.invalidateQueries({ queryKey: displayKeys.list(environmentId) });
      },
    });
    return () => subscription.unsubscribe();
  }, [client, environmentId]);
}

export interface DisplaysPanelProps {
  environment: Environment;
  threadId: string | null;
  /** The viewer to draw with; noVNC, fetched when first needed, unless a test gives another. */
  load?: () => Promise<RfbConstructor>;
}

export function DisplaysPanel({ environment, threadId, load }: DisplaysPanelProps) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const query = useQuery(displaysQuery(environment.id));
  useDisplaysFeed(environment.id);
  const [front, setFront] = useState<string | null>(null);
  const [opening, setOpening] = useState<'virtual' | 'server' | null>(null);
  const [closing, setClosing] = useState<Display | null>(null);
  const requests = useBrowserRequests();
  const look = useMutation({ mutationFn: () => lookAgainForDisplays(environment.id), onSuccess: (list) => client.setQueryData(displayKeys.list(environment.id), list) });
  const share = useMutation({
    mutationFn: () => openDisplay(environment.id, {desktop:true}, threadId),
    onSuccess: (display) => { setFront(display.id); void client.invalidateQueries({queryKey:displayKeys.list(environment.id)}); },
  });
  const mayWork = environment.access.permissions.includes('vnc.manage');

  if (query.isPending) return <SkeletonRows read={query} rows={4} />;
  if (query.isError) return <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />;
  const data = query.data;
  const displays = data.off === null ? data.displays.filter((display) => display.state !== 'closed') : [];
  const asked = requests.display !== null && displays.some((display) => display.id === requests.display) ? requests.display : null;
  const shown = displays.find((display) => display.id === (asked ?? front)) ?? displays[0] ?? null;
  const found = data.support.servers.length > 0;
  const choose = (id: string) => {
    requests.takeDisplay();
    setFront(id);
  };

  return (
    <div className="flex h-full min-h-0 flex-col gap-2 p-3" data-displays-view={environment.id}>
      <div className="flex flex-wrap items-center gap-2">
        <h2 className="min-w-0 flex-1 truncate text-sm font-medium">{t('vnc.list.title', { environment: environment.name })}</h2>
        {mayWork && data.off === null && data.support.desktop === true && <Button size="sm" pending={share.isPending} onClick={() => share.mutate()}>{t('vnc.attach.desktop')}</Button>}
        {mayWork && data.off === null && <Button size="sm" onClick={() => setOpening('server')}>{t('vnc.attach.title')}</Button>}
        {mayWork && data.off === null && found && (
          <Button size="sm" icon={<ScreenShare aria-hidden />} onClick={() => setOpening('virtual')}>
            {t('vnc.list.open')}
          </Button>
        )}
      </div>
      {data.off !== null && <OffNotice off={data.off} environment={environment} />}
      {data.off === null && !found && (
        <InlineAlert
          tone="warning"
          title={t('vnc.none.title', { environment: environment.name })}
          action={
            <Button action="refresh" size="sm" pending={look.isPending} onClick={() => look.mutate()}>
              {t('vnc.none.lookAgain')}
            </Button>
          }
        >
          <span data-displays-none>
            {t('vnc.none.body')} {t('vnc.none.lookedFor', { names: SERVERS_LOOKED_FOR.join(', ') })}
          </span>
        </InlineAlert>
      )}
      {share.error !== null && <ErrorAlert error={share.error} />}
      {look.error !== null && <ErrorAlert error={look.error} />}
      {displays.length > 0 && (
        <ul className="flex max-h-40 shrink-0 flex-col overflow-auto rounded-lg border border-border" aria-label={t('vnc.list.label')}>
          {displays.map((display) => (
            <li key={display.id} className={cx('flex items-start gap-2 border-b border-border px-2 py-1 last:border-0', display.id === shown?.id && 'bg-bg-subtle')} data-display-row={display.id}>
              <StatusDot className="h-(--control-sm)" tone={display.state === 'open' ? 'success' : 'info'} label={t(`vnc.state.${display.state}`)} />
              <button type="button" className="min-w-0 flex-1 text-left" aria-pressed={display.id === shown?.id} onClick={() => choose(display.id)}>
                <span className="block truncate text-sm font-medium leading-(--control-sm)">
                  {display.name}
                  {display.x_display !== null && <span className="ml-2 font-mono text-xs text-text-muted">{display.x_display}</span>}
                  {display.geometry !== null && <span className="ml-2 text-xs text-text-muted">{display.geometry}</span>}
                </span>
                <span className="flex flex-wrap gap-x-3 gap-y-1 text-xs text-text-muted">
                  <span>{display.opened_by !== null ? t('vnc.list.openedBy', { name: display.opened_by.display_name }) : display.thread !== null ? t('vnc.list.openedByAgent', { thread: display.thread.name ?? t('vnc.list.hiddenThread') }) : t('vnc.list.announced')}</span>
                  {display.browsers.length > 0 && <span>{t('vnc.list.browsers', { count: display.browsers.length })}</span>}
                  {display.note !== null && <span className="wrap-anywhere">{display.note}</span>}
                </span>
              </button>
              {mayWork && (
                <Menu
                  label={t('vnc.menu.label')}
                  align="end"
                  entries={[{ id: 'close', action: 'dismiss', label: t('vnc.menu.close'), danger: display.origin === 'started', onSelect: () => setClosing(display) }]}
                  trigger={<IconButton size="sm" label={t('vnc.menu.label')} icon={<Ellipsis aria-hidden />} />}
                />
              )}
            </li>
          ))}
        </ul>
      )}
      {displays.length === 0 && data.off === null && found && <EmptyState compact icon={<MonitorPlay aria-hidden />} title={t('vnc.list.empty')} description={mayWork ? t('vnc.list.emptyHint') : undefined} />}
      {shown !== null && <Front key={shown.id} display={shown} environment={environment} mayWork={mayWork} onClose={() => setClosing(shown)} {...(load === undefined ? {} : { load })} />}
      {opening === 'server' && <AttachDisplay environmentId={environment.id} threadId={threadId} onClose={() => setOpening(null)} onOpened={(display) => { setOpening(null); choose(display.id); }} />}
      {opening === 'virtual' && <OpenDisplay environment={environment} threadId={threadId} onClose={() => setOpening(null)} onOpened={(display) => { setOpening(null); choose(display.id); }} />}
      {closing !== null && <CloseDisplay display={closing} onClose={() => setClosing(null)} />}
    </div>
  );
}

function OffNotice({ off, environment }: { off: NonNullable<EnvironmentDisplays['off']>; environment: Environment }) {
  const { t } = useTranslation();
  if (off === 'machine_policy') return <PolicyOff environment={environment} />;
  const words = {
    disabled_by_policy: t('vnc.off.policy', { environment: environment.name }),
    environment_switch_off: t('vnc.off.switch', { environment: environment.name, owner: environment.owner.display_name }),
    environment_stopping: t('terminal.refused.environment_stopping'),
    environment_offline: t('vnc.off.offline', { environment: environment.name }),
  }[off];
  return (
    <InlineAlert tone={off === 'environment_offline' || off === 'environment_stopping' ? 'info' : 'warning'}>
      <span data-displays-off={off}>{words}</span>
    </InlineAlert>
  );
}

function PolicyOff({environment}:{environment: Environment}) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const hostId = environment.host?.id;
  const host = useQuery({...hostQuery(hostId ?? ''), enabled: hostId !== undefined, retry:false});
  const machine = (environment.placement === 'managed' ? host.data?.machine : environment.machine) ?? null;
  const query = machinePolicyQuery(machine?.id ?? '');
  const policy = useQuery({...query, enabled: machine !== null, retry:false});
  const key = policy.data?.keys.find((row) => row.key === 'environment.vnc');
  const enable = useMutation({
    mutationFn: () => {
      if (machine === null || key?.writable !== true) throw new Error(t('vnc.off.machine', {machine: environment.name}));
      return updateMachinePolicy(machine.id, {'environment.vnc': true});
    },
    onSuccess: (ack) => {
      client.setQueryData(query.queryKey, ack);
      void client.invalidateQueries({queryKey: displayKeys.list(environment.id)});
    },
  });
  return <><StateLine compact message={<span data-displays-off="machine_policy">{t('vnc.off.machine', {machine: machine?.name ?? environment.name})}</span>}
    action={key?.writable === true && key.value !== true ? <Button size="sm" pending={enable.isPending} onClick={() => enable.mutate()}>{t('vnc.attach.enable')}</Button> : undefined} />
    {enable.isError && <ErrorAlert error={enable.error} />}</>;
}

function AttachDisplay({ environmentId, threadId, onClose, onOpened }: {environmentId:string; threadId:string | null; onClose:()=>void; onOpened:(row:Display)=>void}) {
  const { t } = useTranslation();
  const [port, setPort] = useState(''), [password, setPassword] = useState('');
  const number = Number(port);
  const valid = /^\d+$/.test(port) && Number.isInteger(number) && number > 0 && number <= 65535;
  const attach = useMutation({
    mutationFn: () => openDisplay(environmentId, {attach:{port:number, ...(password === '' ? {} : {password})}}, threadId),
    onSuccess: onOpened,
  });
  return <Dialog open size="sm" title={t('vnc.attach.title')} onOpenChange={(next) => { if (!next) onClose(); }}
    footer={<><Button onClick={onClose}>{t('common.cancel')}</Button><Button variant="primary" pending={attach.isPending} disabled={!valid} onClick={() => attach.mutate()}>{t('vnc.attach.apply')}</Button></>}>
    <form className="flex flex-col gap-3" onSubmit={(event) => { event.preventDefault(); if (valid && !attach.isPending) attach.mutate(); }}>
      <Input label={t('vnc.attach.port')} inputMode="numeric" placeholder="5900" value={port} onChange={(event) => setPort(event.target.value)} />
      <Input label={t('vnc.attach.password')} optional type="password" autoComplete="off" value={password} onChange={(event) => setPassword(event.target.value)} />
      {attach.isError && <ErrorAlert error={attach.error} />}
    </form>
  </Dialog>;
}

function Front({ display, environment, mayWork, onClose, load }: { display: Display; environment: Environment; mayWork: boolean; onClose: () => void; load?: () => Promise<RfbConstructor> }) {
  const { t } = useTranslation();
  const [working, setWorking] = useState(false);
  const [state, setState] = useState<ViewerState>({ phase: 'loading', ended: null, quality: QUALITY_BEST });
  const [pasting, setPasting] = useState(false);
  const controls = useRef<ViewerControls>(null);
  const work = working && mayWork;
  const ended = state.phase === 'ended';
  const entries: MenuEntry[] = [
    { id: 'paste', label: t('vnc.menu.paste'), disabled: work && !ended ? false : t('vnc.menu.workFirst'), onSelect: () => setPasting(true) },
    { id: 'ctrl-alt-del', label: t('vnc.menu.ctrlAltDel'), disabled: work && !ended ? false : t('vnc.menu.workFirst'), onSelect: () => controls.current?.ctrlAltDel() },
    { id: 'fit', label: t('vnc.menu.fit'), disabled: work && !ended ? false : t('vnc.menu.workFirst'), onSelect: () => controls.current?.fit() },
    { type: 'separator', id: 'end' },
    { id: 'close', action: 'dismiss', label: t('vnc.menu.close'), danger: display.origin === 'started', disabled: mayWork ? false : t('vnc.menu.manageOnly', { environment: environment.name }), onSelect: onClose },
  ];
  const drivers = display.driving.map((person) => person.display_name);
  return (
    <section className="flex min-h-0 flex-1 flex-col gap-1" aria-label={t('vnc.front.label', { name: display.name })} data-display-front={display.id}>
      <div className="flex flex-col gap-2">
        <div className="flex min-w-0 items-center gap-2"><h3 className="min-w-0 flex-1 truncate text-sm font-medium leading-(--control-sm)">{display.name}</h3>
          <Menu label={t('vnc.menu.label')} align="end" entries={entries} trigger={<IconButton size="sm" label={t('vnc.menu.label')} icon={<Ellipsis aria-hidden />} />} />
        </div>
        {display.origin === 'started' && display.x_display !== null && (
          <span className="inline-flex items-center gap-1 text-xs text-text-muted">
            {t('vnc.startWith')} <code className="rounded bg-bg-subtle px-1 font-mono text-text">{displayLine(display.x_display)}</code>
            <CopyButton value={displayLine(display.x_display)} label={t('vnc.menu.copyLine')} />
          </span>
        )}

      </div>
      {display.origin === 'announced' && <p className="text-xs text-text-muted" data-display-copied>{t('vnc.copied')}</p>}
      <Viewer key={String(work)} displayId={display.id} control={work} label={t('vnc.viewer.label', { name: display.name })} onState={setState} onGiveBack={() => setWorking(false)} controls={controls} {...(load === undefined ? {} : { load })} />
      <div className="flex flex-wrap items-center gap-2 text-sm" data-display-people>
        {display.watching > 0 && <AvatarStack users={display.driving} total={display.watching} size={24} />}
        <span className="flex flex-wrap gap-x-3 gap-y-1 text-text-muted">
          <span>{t('vnc.people.watching', { count: display.watching })}</span>
          {drivers.length > 0 && <span>{drivers.length === 1 ? t('vnc.people.drivesOne', { name: drivers[0] }) : t('vnc.people.drivesMany', { names: drivers.join(', ') })}</span>}
        </span>
        <span className="flex-1" />
        {state.phase === 'loading' && <span role="status" className="text-text-muted">{t('vnc.viewer.loading')}</span>}
        {state.phase === 'connecting' && <span role="status" className="text-text-muted">{t('vnc.phase.connecting')}</span>}
        {state.phase === 'lost' && <span role="status" className="text-text-muted">{t('vnc.phase.lost')}</span>}
        {state.phase === 'failed' && <span role="status" className="text-danger-text">{t('vnc.viewer.failed')}</span>}
        {state.quality < QUALITY_BEST && (
          <span className="text-text-muted" title={t('vnc.reducedHint')} data-display-reduced>
            {t('vnc.reduced')}
          </span>
        )}
        {mayWork ? (
          working ? (
            <Button size="sm" variant="primary" icon={<Hand aria-hidden />} onClick={() => setWorking(false)}>
              {t('vnc.stopWork')}
            </Button>
          ) : (
            <Button size="sm" icon={<ScreenShare aria-hidden />} disabled={ended} onClick={() => setWorking(true)}>
              {t('vnc.work')}
            </Button>
          )
        ) : (
          <span className="text-text-muted">{t('vnc.watchOnly', { environment: environment.name, managers: environment.owner.display_name })}</span>
        )}
      </div>
      {work && <p className="text-xs text-text-muted">{t('vnc.keysBack')}</p>}
      {ended && <InlineAlert tone="info"><span data-display-ended={state.ended ?? 'unknown'}>{t(`vnc.ended.${state.ended ?? 'unknown'}`)}</span></InlineAlert>}
      {pasting && <PasteDialog onClose={() => setPasting(false)} onPaste={(text) => controls.current?.paste(text)} />}
    </section>
  );
}

function OpenDisplay({ environment, threadId, onClose, onOpened }: { environment: Environment; threadId: string | null; onClose: () => void; onOpened: (display: Display) => void }) {
  const { t } = useTranslation();
  const [name, setName] = useState('');
  const [size, setSize] = useState('');
  const open = useMutation({
    mutationFn: () => openDisplay(environment.id, { ...(name.trim() === '' ? {} : { name: name.trim() }), ...(size.trim() === '' ? {} : { geometry: size.trim() }) }, threadId),
    onSuccess: (display) => {
      onOpened(display);
    },
  });
  const sizeFault = size.trim() === '' || GEOMETRY.test(size.trim()) ? null : t('vnc.open.sizeFault');
  return (
    <Dialog
      open
      size="sm"
      title={t('vnc.open.title', { environment: environment.name })}
      onOpenChange={(next) => { if (!next) onClose(); }}
      footer={
        <>
          <Button onClick={onClose}>{t('common.cancel')}</Button>
          <Button variant="primary" icon={<ScreenShare aria-hidden />} pending={open.isPending} disabled={sizeFault !== null} onClick={() => open.mutate()}>
            {t('vnc.open.do')}
          </Button>
        </>
      }
    >
      <form className="flex flex-col gap-3" onSubmit={(event) => { event.preventDefault(); if (sizeFault === null) open.mutate(); }}>
        <Input label={t('vnc.open.name')} optional hint={t('vnc.open.nameHint')} value={name} onChange={(event) => setName(event.target.value)} />
        <Input label={t('vnc.open.size')} optional mono hint={t('vnc.open.sizeHint')} error={sizeFault ?? undefined} value={size} onChange={(event) => setSize(event.target.value)} />
        {open.error !== null && <ErrorAlert error={open.error} />}
      </form>
    </Dialog>
  );
}

function CloseDisplay({ display, onClose }: { display: Display; onClose: () => void }) {
  const { t } = useTranslation();
  const close = useMutation({
    mutationFn: () => closeDisplay(display.id),
    onSuccess: () => {
      onClose();
    },
  });
  const started = display.origin === 'started';
  return (
    <ConfirmDialog
      open
      onOpenChange={(next) => { if (!next) onClose(); }}
      title={t('vnc.close.title', { name: display.name })}
      confirmLabel={started ? t('vnc.close.confirmStarted') : t('vnc.close.confirmAnnounced')}
      danger={started}
      pending={close.isPending}
      error={close.error}
      onConfirm={() => close.mutate()}
    >
      <p data-close-words={display.origin}>{started ? t('vnc.close.started') : t('vnc.close.announced')}</p>
    </ConfirmDialog>
  );
}

function PasteDialog({ onClose, onPaste }: { onClose: () => void; onPaste: (text: string) => void }) {
  const { t } = useTranslation();
  const [text, setText] = useState('');
  return (
    <Dialog
      open
      size="md"
      title={t('vnc.paste.title')}
      description={t('vnc.paste.lead')}
      onOpenChange={(next) => { if (!next) onClose(); }}
      footer={
        <>
          <Button onClick={onClose}>{t('common.cancel')}</Button>
          <Button variant="primary" disabled={text === ''} onClick={() => { onPaste(text); onClose(); }}>
            {t('vnc.paste.do')}
          </Button>
        </>
      }
    >
      <Textarea label={t('vnc.paste.text')} value={text} rows={5} onChange={(event) => setText(event.target.value)} />
    </Dialog>
  );
}
