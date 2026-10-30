/** The harness's own terminal, watched first and controlled only after a recorded confirmation. */
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { api, ok } from '@/api/client';
import type { Thread, TerminalTicket } from '@/generated/api-types';
import { connectShell, type ShellConnection } from '@/realtime/terminal';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';

/** Match the ordinary terminal's retained screen without keeping unlimited harness output. */
const SCROLLBACK_LINES = 5_000;
const ticket = (id: string, control: boolean): Promise<TerminalTicket> => ok(api.POST('/threads/{id}/agent-terminal/ticket', { params: { path: { id } }, body: { control } }));

function Screen({ threadId, control }: { threadId: string; control: boolean }) {
  const { t } = useTranslation();
  const parent = useRef<HTMLDivElement>(null);
  const [failure, setFailure] = useState<unknown>(null);
  const [connection, setConnection] = useState<ShellConnection | null>(null);
  useEffect(() => {
    const host = parent.current;
    if (host === null) return;
    const life = { alive: true };
    let cleanup: (() => void) | undefined;
    void (async () => {
      const [first, { Terminal }, { FitAddon }] = await Promise.all([ticket(threadId, control), import('@xterm/xterm'), import('@xterm/addon-fit'), import('@xterm/xterm/css/xterm.css')]);
      if (!life.alive) return;
      Terminal.strings.promptLabel = t('threadViews.diagnostics.input');
      const style = getComputedStyle(document.documentElement);
      const terminal = new Terminal({ disableStdin: !control, scrollback: SCROLLBACK_LINES, fontFamily: style.getPropertyValue('--font-mono').trim(), theme: { background: style.getPropertyValue('--terminal-bg').trim(), foreground: style.getPropertyValue('--terminal-fg').trim() } });
      const fit = new FitAddon();
      terminal.loadAddon(fit);
      terminal.open(host);
      const made = connectShell({ terminalId: threadId, firstTicket: first.ticket,
        tickets: async () => (await ticket(threadId, control)).ticket,
        address: (id, value) => `${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}/api/v1/ws/agent-terminals/${encodeURIComponent(id)}?ticket=${encodeURIComponent(value)}`,
        listener: { onReady: () => terminal.reset(), onData: (bytes) => terminal.write(bytes) },
      });
      setConnection(made);
      const input = terminal.onData((text) => { if (control) made.send(new TextEncoder().encode(text)); });
      const resize = () => { if (host.clientWidth > 0 && host.clientHeight > 0) { fit.fit(); if (control) made.resize(terminal.cols, terminal.rows); } };
      const observer = new ResizeObserver(resize);
      observer.observe(host);
      resize();
      cleanup = () => { observer.disconnect(); input.dispose(); made.detach(); terminal.dispose(); };
    })().catch((error: unknown) => { if (life.alive) setFailure(error); });
    return () => { life.alive = false; cleanup?.(); };
  }, [threadId, control, t]);
  return <div className="flex min-h-0 flex-1 flex-col">{failure !== null && <ErrorAlert error={failure} />}{connection !== null && <Connection connection={connection} />}<div ref={parent} className="min-h-0 flex-1" data-testid="agent-terminal" /></div>;
}

function Connection({ connection }: { connection: ShellConnection }) {
  const { t } = useTranslation();
  const phase = useStore(connection.state, (state) => state.phase);
  return phase === 'attached' ? null : <InlineAlert tone="info">{t('threadViews.diagnostics.connection', { state: phase })}</InlineAlert>;
}

export function AgentTerminal({ thread, mayControl }: { thread: Thread; mayControl: boolean }) {
  const { t } = useTranslation();
  const [control, setControl] = useState(false);
  const [confirm, setConfirm] = useState(false);
  if (!thread.environment_permissions.includes('terminal.view')) return <InlineAlert tone="info">{t('threadViews.diagnostics.watchRequired')}</InlineAlert>;
  return <div className="flex h-full min-h-0 flex-col gap-2 p-2">
    <InlineAlert tone={control ? 'warning' : 'info'}>{t(control ? 'threadViews.diagnostics.controlling' : 'threadViews.diagnostics.watching')}</InlineAlert>
    <div>{control ? <Button onClick={() => setControl(false)}>{t('threadViews.diagnostics.release')}</Button> : <Button disabled={!mayControl || !thread.environment_permissions.includes('terminal.manage')} onClick={() => setConfirm(true)}>{t('threadViews.diagnostics.take')}</Button>}</div>
    <Screen key={String(control)} threadId={thread.id} control={control} />
    <Dialog open={confirm} onOpenChange={setConfirm} title={t('threadViews.diagnostics.take')} description={t('threadViews.diagnostics.confirm')} footer={<><Button data-autofocus onClick={() => setConfirm(false)}>{t('common.cancel')}</Button><Button variant="primary" onClick={() => { setControl(true); setConfirm(false); }}>{t('threadViews.diagnostics.take')}</Button></>} />
  </div>;
}
