/** Opens the display door with the selected port after an explicit greeting check. */
import { useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Button } from '@/ui/Button';
import { PortFault } from './Fault';
import { checkPortDisplay } from './displayGreeting';

export type OpenPortDisplay = (environment: string, port: number) => void;

export function PortDisplayAction({ environment, port, attach, probe = checkPortDisplay }: {
  environment: string;
  port: number;
  attach: OpenPortDisplay;
  probe?: typeof checkPortDisplay;
}) {
  const { t } = useTranslation();
  const [state, setState] = useState<'ready' | 'checking' | 'unavailable' | 'other' | 'rfb'>('ready');
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
  return <><Button size="sm" pending={state === 'checking'} onClick={() => state === 'rfb' ? attach(environment, port) : void check()}>
    {t(state === 'rfb' ? 'ports.openAsDisplay' : state === 'other' ? 'ports.noVncGreeting' : state === 'unavailable' ? 'ports.portState.unavailable' : 'ports.checkForDisplay')}
  </Button><PortFault error={error} /></>;
}
