/** Opens the display door with the selected port after an explicit greeting check. */
import { useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Button } from '@/ui/Button';
import { Input } from '@/ui/Input';
import { client } from '@/api/client';
import { openDisplay } from '@/api/displays';
import { useBrowserRequests } from '@/features/browser/store';
import { PortFault } from './Fault';
import { checkPortDisplay } from './displayGreeting';

export type OpenPortDisplay = (environment: string, port: number, password: string, signal: AbortSignal) => Promise<void>;

/** Adopt through the ordinary VNC door and select the display returned by that request. */
export function portDisplayOpener(threadId: string | null, reveal: () => void): OpenPortDisplay {
  return async (environment, port, password, signal) => {
    const reader = client.session.getState().readerVersion;
    const display = await openDisplay(environment, { attach: { port, ...(password ? { password } : {}) } }, threadId);
    if (signal.aborted || client.session.getState().readerVersion !== reader) return;
    useBrowserRequests.getState().showDisplay(display.id);
    reveal();
  };
}

export function PortDisplayAction({ environment, port, attach, probe = checkPortDisplay }: {
  environment: string;
  port: number;
  attach: OpenPortDisplay;
  probe?: typeof checkPortDisplay;
}) {
  const { t } = useTranslation();
  const [state, setState] = useState<'ready' | 'checking' | 'unavailable' | 'other' | 'rfb'>('ready');
  const [password, setPassword] = useState('');
  const [attaching, setAttaching] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const pending = useRef<AbortController | null>(null);
  useLayoutEffect(() => () => { pending.current?.abort(); }, []);
  const check = async () => {
    if (pending.current !== null) return;
    const request = new AbortController();
    pending.current = request;
    setError(null);
    setState('checking');
    try {
      const rfb = await probe(environment, port, request.signal);
      if (!request.signal.aborted) setState(rfb === null ? 'unavailable' : rfb ? 'rfb' : 'other');
    } catch (failed) {
      if (!request.signal.aborted) { setError(failed); setState('ready'); }
    } finally {
      pending.current = null;
    }
  };
  const open = async () => {
    if (pending.current !== null) return;
    const request = new AbortController();
    pending.current = request;
    setAttaching(true);
    setError(null);
    setPassword('');
    try {
      await attach(environment, port, password, request.signal);
    } catch (failed) {
      if (!request.signal.aborted) setError(failed);
    } finally {
      pending.current = null;
      if (!request.signal.aborted) setAttaching(false);
    }
  };
  return <>
    {state === 'rfb' && <Input type="password" autoComplete="off" optional disabled={attaching}
      label={t('ports.vncPassword')} value={password} onChange={(event) => setPassword(event.target.value)} />}
    <Button size="sm" pending={attaching || state === 'checking'} onClick={() => state === 'rfb' ? void open() : void check()}>
      {t(state === 'rfb' ? 'ports.openAsDisplay' : state === 'other' ? 'ports.noVncGreeting' : state === 'unavailable' ? 'ports.portState.unavailable' : 'ports.checkForDisplay')}
    </Button>
    <PortFault error={error} />
  </>;
}
