/** One shell on the screen: the terminal, drawn from the bytes the shell prints and sending what is typed, with the states a shell goes through.
 *
 * The terminal fills the panel and follows its size. Colours come from the theme's tokens, so in the light
 * theme a shell has a light ground. What is selected is copied at once; a menu on the terminal copies it,
 * or puts it into the message to the agent. A shell that is open on another screen says so and can be taken
 * here; one that was closed says why, in words, and offers what it last printed and a new shell. The
 * round trip is shown on the tab by the panel when it is above 150 ms.
 */
import type { ILink, Terminal as Xterm } from '@xterm/xterm';
import type { TFunction } from 'i18next';
import { Copy, MessageSquarePlus, RotateCw } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { useMutation } from '@tanstack/react-query';

import { shellRecording } from '@/api/terminals';
import { addToMessage } from '@/features/files/AddToMessage';
import { localLinks, useLocalAddresses } from '@/features/ports/LocalAddresses';
import type { TerminalSession } from '@/generated/api-types';
import { type CloseReason, connectShell, type ShellConnection } from '@/realtime/terminal';
import { Button } from '@/ui/Button';
import { ContextMenu } from '@/ui/ContextMenu';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { SkeletonRows } from '@/ui/Skeleton';
import { TextPreview } from '@/ui/Preview';
import { ScrollArea } from '@/ui/ScrollArea';
import { copyText, writeToClipboard } from '@/ui/clipboard';

/** The control sequences of a shell's output, which a plain look at what it printed leaves out. */
const ESCAPES = new RegExp(`${String.fromCharCode(27)}\\[[0-9;?]*[ -/]*[@-~]`, 'g');

/** What the key handler lets the page have: the toggles of the bars; everything else goes to the shell. */
function isBarToggle(event: KeyboardEvent): boolean {
  return event.key === 'F6' || (event.ctrlKey || event.metaKey) && (event.key === '.' || event.key === '\\');
}

/** The theme's colours, read from the tokens, as the terminal takes them. */
function themeFromTokens(): Record<string, string> {
  const style = getComputedStyle(document.documentElement);
  const read = (token: string) => style.getPropertyValue(`--terminal-${token}`).trim();
  return {
    background: read('bg'),
    foreground: read('fg'),
    cursor: read('cursor'),
    cursorAccent: read('bg'),
    selectionBackground: read('selection'),
    black: read('black'),
    red: read('red'),
    green: read('green'),
    yellow: read('yellow'),
    blue: read('blue'),
    magenta: read('magenta'),
    cyan: read('cyan'),
    white: read('white'),
    brightBlack: read('bright-black'),
    brightRed: read('bright-red'),
    brightGreen: read('bright-green'),
    brightYellow: read('bright-yellow'),
    brightBlue: read('bright-blue'),
    brightMagenta: read('bright-magenta'),
    brightCyan: read('bright-cyan'),
    brightWhite: read('bright-white'),
  };
}

/** The words a shell's end is told in. */
export function endText(t: TFunction, reason: CloseReason | null, exitCode: number | null, session: TerminalSession, environment: string): string {
  switch (reason) {
    case 'exited':
      return t('terminal.closed.exited', { code: exitCode ?? session.exit_code ?? 0 });
    case 'closed_by_user':
      return t('terminal.closed.byUser');
    case 'detached':
      return t('terminal.closed.detached', { hours: 8 });
    case 'environment_stopped':
      return t('terminal.closed.stopped', { name: environment });
    case 'revoked':
      return t('terminal.closed.revoked');
    case 'link_lost':
      return t('terminal.closed.linkLost');
    default:
      return t('terminal.closed.generic');
  }
}

export interface TerminalScreenProps {
  session: TerminalSession;
  /** A ticket for the first attachment, where the shell was just opened. */
  firstTicket: string | null;
  /** The thread the output is added to a message of. */
  threadId: string | null;
  environmentName: string;
  /** Tells the panel what the connection says, for the tab, and which shell it is of: `null` when this window lets the shell go. */
  onConnection: (connection: ShellConnection | null, shellId: string) => void;
  onOpenNew?: () => void;
  /** Whether this window has the shell: false while it is taken elsewhere. */
  active: boolean;
}

export function TerminalScreen({ session, firstTicket, threadId, environmentName, onConnection, onOpenNew, active }: TerminalScreenProps) {
  const { t } = useTranslation();
  const host = useRef<HTMLDivElement>(null);
  const term = useRef<Xterm | null>(null);
  const [connection, setConnection] = useState<ShellConnection | null>(null);
  const [selection, setSelection] = useState('');
  const [last, setLast] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [loadError, setLoadError] = useState<unknown>(null);
  const [attempt, setAttempt] = useState(0);
  const recording = useMutation({ mutationFn: () => shellRecording(session.id), onSuccess: (bytes) => setLast(bytes === null ? '' : new TextDecoder().decode(bytes)) });
  const tickets = useRef(firstTicket);
  const closedAlready = session.state === 'closed';
  // A local address the shell prints leads to the environment's port through Orch, as one in a message does.
  const rewrite = useLocalAddresses(session.environment_id, !closedAlready && active);
  const rewriting = useRef(rewrite);
  useEffect(() => { rewriting.current = rewrite; }, [rewrite]);

  // The terminal and its connection are made when the screen is shown and let go when it is not.
  useEffect(() => {
    const parent = host.current;
    if (parent === null || closedAlready || !active) return;
    const life = { alive: true };
    let made: ShellConnection | null = null;
    let cleanup: (() => void) | undefined;
    void (async () => {
      const [{ Terminal }, { FitAddon }] = await Promise.all([import('@xterm/xterm'), import('@xterm/addon-fit'), import('@xterm/xterm/css/xterm.css')]);
      if (!life.alive) return;
      Terminal.strings.promptLabel = t('terminal.screen.input');
      Terminal.strings.tooMuchOutput = t('terminal.screen.tooMuch');
      const screen = new Terminal({
        cursorBlink: true,
        fontFamily: getComputedStyle(document.documentElement).getPropertyValue('--font-mono').trim() || 'monospace',
        fontSize: 13,
        scrollback: 5_000,
        theme: themeFromTokens(),
        convertEol: false,
      });
      cleanup = () => { made?.detach(); screen.dispose(); term.current = null; };
      const fit = new FitAddon();
      screen.loadAddon(fit);
      screen.open(parent);
      term.current = screen;
      const resize = () => {
        if (parent.clientWidth === 0 || parent.clientHeight === 0) return;
        fit.fit();
        made?.resize(screen.cols, screen.rows);
      };
      resize();
      setLoaded(true);

      screen.attachCustomKeyEventHandler((event) => !isBarToggle(event));
      screen.registerLinkProvider({
        provideLinks: (line, callback) => {
          const text = screen.buffer.active.getLine(line - 1)?.translateToString(true) ?? '';
          const links: ILink[] = localLinks(text, rewriting.current).map((found) => ({
            range: { start: { x: found.start + 1, y: line }, end: { x: found.end, y: line } },
            text: found.text,
            activate: () => { window.open(found.target, '_blank', 'noopener,noreferrer'); },
          }));
          callback(links.length > 0 ? links : undefined);
        },
      });
      screen.onData((data) => made?.send(new TextEncoder().encode(data)));
      screen.onBinary((data) => made?.send(Uint8Array.from(data, (character) => character.charCodeAt(0))));
      screen.onSelectionChange(() => {
        const text = screen.getSelection();
        setSelection(text);
        // What is selected is copied at once, as a terminal does.
        if (text !== '') void writeToClipboard(text);
      });

      made = connectShell({
        terminalId: session.id,
        ...(tickets.current === null ? {} : { firstTicket: tickets.current }),
        listener: {
          onReady: () => {
            // The shell sends the end of what it printed first; the screen is drawn from that.
            screen.reset();
            resize();
          },
          onData: (bytes) => screen.write(bytes),
        },
      });
      tickets.current = null;
      setConnection(made);
      onConnection(made, session.id);

      const watch = new ResizeObserver(resize);
      watch.observe(parent);
      const themes = new MutationObserver(() => {
        screen.options.theme = themeFromTokens();
      });
      themes.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
      const stopScreen = cleanup;
      cleanup = () => {
        watch.disconnect();
        themes.disconnect();
        stopScreen();
      };
    })().catch((error: unknown) => { if (life.alive) { cleanup?.(); cleanup = undefined; onConnection(null, session.id); setLoadError(error); } });
    return () => {
      life.alive = false;
      cleanup?.();
      setConnection(null);
      onConnection(null, session.id);
      setLoaded(false);
    };
  }, [session.id, closedAlready, active, attempt]); // eslint-disable-line react-hooks/exhaustive-deps

  const state = useStore(connection?.state ?? EMPTY, (value) => value);

  if (closedAlready || state.phase === 'closed') {
    const reason = state.reason ?? (session.close_reason as CloseReason | null);
    return (
      <ScrollArea className="h-full"><EmptyState
        compact
        className="min-h-full"
        icon={<RotateCw aria-hidden />}
        title={endText(t, reason, state.exitCode, session, environmentName)}
        description={last === null ? undefined : t('terminal.closed.lastPrinted')}
        action={
          <div className="flex flex-col items-center gap-2">
            {session.recorded && last === null && (
              <Button
                size="sm"
                pending={recording.isPending}
                onClick={() => recording.mutate()}
              >
                {t('terminal.closed.show')}
              </Button>
            )}
            {last !== null && (
              <div data-testid="last-printed" className="min-w-0 max-w-full text-left"><TextPreview value={last === '' ? t('terminal.closed.nothingKept') : last.replace(ESCAPES, '')} className="rounded-md border border-border bg-bg-inset p-2 font-mono text-xs" /></div>
            )}
            {recording.error !== null && <ErrorAlert error={recording.error} />}
            {onOpenNew !== undefined && <Button variant="primary" onClick={onOpenNew}>
              {t('terminal.closed.openNew')}
            </Button>}
          </div>
        }
      /></ScrollArea>
    );
  }
  if (!active) return null;
  if (loadError !== null) return <ErrorState compact className="h-full" error={loadError} onRetry={() => { setLoadError(null); setAttempt((value) => value + 1); }} />;

  const addSelection = () => { if (threadId !== null) addToMessage(threadId, { kind: 'output', command: null, text: selection }); };
  return (
    <div className="relative flex h-full min-h-0 flex-col" data-testid="terminal-screen">
      {state.phase === 'lost' && (
        <InlineAlert tone="warning" className="m-1">
          {t('terminal.screen.reconnecting')}
        </InlineAlert>
      )}
      {state.phase === 'elsewhere' && (
        <InlineAlert
          tone="info"
          className="m-1"
          action={
            <Button size="sm" onClick={() => connection?.attach()}>
              {t('terminal.screen.takeHere')}
            </Button>
          }
        >
          {t('terminal.screen.elsewhere')}
        </InlineAlert>
      )}
      {(!loaded || state.phase === 'connecting') && <SkeletonRows rows={2} />}
      <ContextMenu
        label={t('terminal.screen.menu')}
        entries={[
          { id: 'copy', label: t('terminal.screen.copy'), icon: <Copy aria-hidden />, disabled: selection === '' ? t('terminal.screen.noSelection') : false, onSelect: () => void copyText(selection) },
          ...(threadId === null ? [] : [{ id: 'add', label: t('files.addToMessage'), icon: <MessageSquarePlus aria-hidden />, disabled: selection === '' ? t('terminal.screen.noSelection') : false as const, onSelect: addSelection }]),
        ]}
      >
        <div ref={host} className="min-h-0 min-w-0 flex-1 overflow-hidden bg-(--terminal-bg) p-1" data-testid="terminal-host" />
      </ContextMenu>
    </div>
  );
}

import { createStore } from 'zustand/vanilla';

/** What a screen reads while it has no connection yet. */
const EMPTY = createStore<{ phase: 'connecting'; reason: null; exitCode: null; roundTripMs: null }>(() => ({ phase: 'connecting', reason: null, exitCode: null, roundTripMs: null }));
