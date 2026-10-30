/** One browser in front: its bar (back, forward, reload or stop, the address, the window's size and its menu), its tabs, the page, who watches and who drives, and what a page asks of a person.
 *
 * Watching and driving are two states, a press apart. Whoever may drive sees Drive; until it is pressed
 * nothing they do reaches the page, and the socket they hold is one that only watches. Driving is a
 * socket of its own; Stop driving, Ctrl+Alt+Esc and leaving the view end it. Several may drive at once
 * and an agent may work in the same tab: nothing locks, and the line under the picture says who drives.
 * A dialog of a page and a request for a file are shown over the picture to whoever drives. The picture
 * says when it was reduced because the link from the environment is slow.
 */
import { useMutation } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { ArrowLeft, ArrowRight, Ellipsis, Hand, MousePointerClick, Plus, RotateCw, Square, X } from 'lucide-react';
import { type RefObject, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { createStore } from 'zustand/vanilla';

import { quitBrowser } from '@/api/browsers';
import type { Browser, EnvironmentBrowsers } from '@/generated/api-types';
import { AvatarStack } from '@/ui/AvatarStack';
import { Button } from '@/ui/Button';
import { copyText } from '@/ui/clipboard';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { cx } from '@/ui/cx';
import { Dialog } from '@/ui/Dialog';
import { IconButton } from '@/ui/IconButton';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { FoldingStrip } from '@/ui/FoldingStrip';
import { Textarea } from '@/ui/Textarea';

import { Picture } from './Picture';
import { type BrowserConnection, type BrowserPicture, type BrowserSocketState, connectBrowser } from './socket';

/** The level from which the picture is smaller than the page: below it only its quality changes. */
const REDUCED_FROM_LEVEL = 2;

/** Who drives, in a line: nobody, one, or several. */
function drivers(t: TFunction, state: BrowserSocketState): string | null {
  const names = state.driving.map((person) => person.display_name);
  if (names.length === 0) return null;
  return names.length === 1 ? t('browser.people.drivesOne', { name: names[0] }) : t('browser.people.drivesMany', { names: names.join(', ') });
}

export interface FrontProps {
  browser: Browser;
  list: EnvironmentBrowsers;
  /** Whether the reader may drive browsers there (`browser.manage`). */
  mayDrive: boolean;
  /** The environment's name, for the sentences. */
  environmentName: string;
  /** Who manages the environment, for the sentence a watcher reads. */
  managers: string;
  /** Whether the reader may read the environment's files: who may give a page a file. */
  mayReadFiles: boolean;
}

export function Front({ browser, list, mayDrive, environmentName, managers, mayReadFiles }: FrontProps) {
  const [driving, setDriving] = useState(false);
  // The connection lives outside React's state: it is made and ended by the effect, and the view follows it.
  const [holder] = useState(() => createStore<{ connection: BrowserConnection | null }>(() => ({ connection: null })));
  const connection = useStore(holder, (state) => state.connection);
  const sink = useRef<((picture: BrowserPicture) => void) | null>(null);

  // A socket that watches, or one that drives: one at a time, made again when the person changes which.
  // Leaving the view ends it, and with it the driving.
  useEffect(() => {
    const made = connectBrowser({ browserId: browser.id, control: driving, onPicture: (picture) => sink.current?.(picture), onCopied: (text) => void copyText(text) });
    holder.setState({ connection: made });
    return () => {
      made.close();
      holder.setState({ connection: null });
    };
  }, [holder, browser.id, driving]);

  if (connection === null) return null;
  return <Connected key={browser.id} browser={browser} list={list} connection={connection} sink={sink} driving={driving} setDriving={setDriving} mayDrive={mayDrive} environmentName={environmentName} managers={managers} mayReadFiles={mayReadFiles} />;
}

function Connected({ browser, list, connection, sink, driving, setDriving, mayDrive, environmentName, managers, mayReadFiles }: FrontProps & { connection: BrowserConnection; sink: RefObject<((picture: BrowserPicture) => void) | null>; driving: boolean; setDriving: (on: boolean) => void }) {
  const { t } = useTranslation();
  const state = useStore(connection.state);
  const [address, setAddress] = useState<string | null>(null);
  const [panel, setPanel] = useState<{ width: number; height: number } | null>(null);
  const [pasting, setPasting] = useState(false);
  const [quitting, setQuitting] = useState(false);
  const tab = state.tabs.find((one) => one.id === state.tab) ?? null;
  const control = driving && state.control;
  const quit = useMutation({
    mutationFn: () => quitBrowser(browser.id),
    onSuccess: () => {
      setQuitting(false);
    },
  });
  const send = connection.send;
  const shownAddress = address ?? tab?.url ?? '';
  const agentLine = list.support.playwright_cli === null || browser.endpoint === null ? browser.endpoint : `playwright-cli attach --cdp=${browser.endpoint}`;

  const entries: MenuEntry[] = [
    { id: 'new-tab', label: t('browser.menu.newTab'), icon: <Plus aria-hidden />, disabled: control ? false : t('browser.menu.driveFirst'), onSelect: () => send({ op: 'tab', do: 'new' }) },
    { id: 'copy-endpoint', action: 'copy', label: t('browser.menu.copyEndpoint'), disabled: agentLine === null ? t('browser.menu.noEndpoint') : false, onSelect: () => { if (agentLine !== null) void copyText(agentLine); } },
    { id: 'settings', label: t('browser.menu.settings'), disabled: control ? false : t('browser.menu.driveFirst'), onSelect: () => send({ op: 'tab', do: 'new', url: 'chrome://settings' }) },
    { id: 'paste', label: t('browser.menu.paste'), disabled: control && tab !== null ? false : t('browser.menu.driveFirst'), onSelect: () => setPasting(true) },
    { id: 'copy-selection', label: t('browser.menu.copySelection'), disabled: control && tab !== null ? false : t('browser.menu.driveFirst'), onSelect: () => { if (tab !== null) send({ op: 'copy', tab: tab.id }); } },
    { id: 'fit', label: t('browser.menu.fit'), disabled: control && panel !== null ? false : t('browser.menu.driveFirst'), onSelect: () => { if (panel !== null) send({ op: 'viewport', width: panel.width, height: panel.height }); } },
    { type: 'separator', id: 'end' },
    { id: 'quit', label: t('browser.menu.quit'), danger: true, disabled: mayDrive ? false : t('browser.menu.manageOnly', { environment: environmentName }), onSelect: () => setQuitting(true) },
  ];

  const closedWords = state.phase === 'closed' ? t(`browser.closed.${state.reason ?? 'link_lost'}`) : null;
  const who = drivers(t, state);
  const refusedCode = state.refused?.code;
  const refusedMessage = t(`browser.refused.${refusedCode === 'view_only' || refusedCode === 'no_such_tab' || refusedCode === 'not_allowed' ? refusedCode : 'invalid'}`);

  return (
    <section className="flex min-h-0 flex-1 flex-col gap-1" aria-label={t('browser.front.label', { name: browser.profile ?? t('browser.list.ownProfile') })} data-browser-front={browser.id}>
      <form
        className="flex flex-wrap items-center gap-1"
        onSubmit={(event) => {
          event.preventDefault();
          if (!control || tab === null || shownAddress.trim() === '') return;
          send({ op: 'navigate', tab: tab.id, url: shownAddress.trim() });
          setAddress(null);
        }}
      >
        <IconButton size="sm" label={t('browser.bar.back')} icon={<ArrowLeft aria-hidden />} disabled={!control || tab === null} onClick={() => tab !== null && send({ op: 'history', tab: tab.id, to: 'back' })} />
        <IconButton size="sm" label={t('browser.bar.forward')} icon={<ArrowRight aria-hidden />} disabled={!control || tab === null} onClick={() => tab !== null && send({ op: 'history', tab: tab.id, to: 'forward' })} />
        {tab?.loading === true ? (
          <IconButton size="sm" label={t('browser.bar.stop')} icon={<Square aria-hidden />} disabled={!control} onClick={() => send({ op: 'history', tab: tab.id, to: 'stop' })} />
        ) : (
          <IconButton size="sm" label={t('browser.bar.reload')} icon={<RotateCw aria-hidden />} disabled={!control || tab === null} onClick={() => tab !== null && send({ op: 'history', tab: tab.id, to: 'reload' })} />
        )}
        <Input label={t('browser.bar.address')} hideLabel mono fieldClassName="min-w-0 flex-1" value={shownAddress} readOnly={!control} onChange={(event) => setAddress(event.target.value)} onBlur={() => setAddress(null)} />
        {state.viewport !== null && (
          <span className="text-xs text-text-muted tabular-nums" title={t('browser.bar.size')}>
            {t('browser.bar.dimensions', { width: state.viewport.width, height: state.viewport.height })}
          </span>
        )}
        <Menu label={t('browser.menu.label')} align="end" entries={entries} trigger={<IconButton size="sm" label={t('browser.menu.label')} icon={<Ellipsis aria-hidden />} />} />
      </form>
      <FoldingStrip role="group" label={t('browser.tabs.label')} active={state.tab ?? ''} items={state.tabs.map((one) => {
        const title = one.title === '' ? one.url : one.title;
        const select = () => {
          if (control) send({ op: 'tab', do: 'activate', tab: one.id });
          if (panel !== null) connection.watch(one.id, panel.width, panel.height);
        };
        return { id: one.id, label: title, onSelect: select,
          measure: <span className="inline-flex max-w-48 shrink-0 items-center border border-b-0 border-border text-xs"><span className="truncate px-2 py-1">{title}</span>{control && <span className="w-(--control-sm) shrink-0" />}</span>,
          content: <span className={cx('inline-flex min-w-0 max-w-[min(12rem,100%)] shrink-0 items-center rounded-t-md border border-b-0 border-border text-xs', one.id === state.tab ? 'bg-surface text-text' : 'bg-bg-subtle text-text-muted')}>
            <button type="button" aria-pressed={one.id === state.tab} className="min-w-0 truncate px-2 py-1" title={one.url} onClick={select}>{title}</button>
            {control && <IconButton size="sm" label={t('browser.tabs.close', { name: title })} icon={<X aria-hidden />} onClick={() => send({ op: 'tab', do: 'close', tab: one.id })} />}
          </span>,
        };
      })} actions={control ? <IconButton size="sm" label={t('browser.menu.newTab')} icon={<Plus aria-hidden />} onClick={() => send({ op: 'tab', do: 'new' })} /> : undefined} />
      <div className="relative flex min-h-0 flex-1 flex-col">
        <Picture connection={connection} sink={sink} tab={state.tab} viewport={state.viewport} driving={control} onGiveBack={() => setDriving(false)} onPanel={setPanel} label={t('browser.picture.label', { title: tab?.title ?? '' })} />
        {control && state.dialog !== null && <PageDialogOverlay state={state} onAnswer={(accept, text) => { if (state.dialog !== null) send({ op: 'dialog', tab: state.dialog.tab, accept, ...(text === undefined ? {} : { text }) }); }} />}
        {state.chooser !== null && (control ? <ChooserOverlay mayReadFiles={mayReadFiles} environmentName={environmentName} multiple={state.chooser.multiple} onAnswer={(paths) => { if (state.chooser !== null) send({ op: 'files', tab: state.chooser.tab, paths }); }} /> : null)}
        {closedWords !== null && (
          <div className="absolute inset-0 flex items-center justify-center bg-bg/80 p-4" role="status">
            <InlineAlert tone="info">{closedWords}</InlineAlert>
          </div>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-2 text-sm" data-browser-people>
        {state.watching.length > 0 && <AvatarStack users={state.watching} size={24} />}
        <span className="flex flex-wrap gap-x-3 gap-y-1 text-text-muted">
          <span>{t('browser.people.watching', { count: state.watching.length })}</span>
          {who !== null && <span>{who}</span>}
        </span>
        <span className="flex-1" />
        {state.phase === 'lost' && <span role="status" className="text-text-muted">{t('browser.phase.lost')}</span>}
        {state.phase === 'connecting' && <span role="status" className="text-text-muted">{t('browser.phase.connecting')}</span>}
        {state.quality !== null && state.quality >= REDUCED_FROM_LEVEL && (
          <span className="text-text-muted" title={t('browser.reducedHint')} data-browser-reduced>
            {t('browser.reduced')}
          </span>
        )}
        {mayDrive ? (
          driving ? (
            <Button size="sm" variant="primary" icon={<Hand aria-hidden />} onClick={() => setDriving(false)}>
              {t('browser.stopDriving')}
            </Button>
          ) : (
            <Button size="sm" icon={<MousePointerClick aria-hidden />} disabled={state.phase === 'closed'} onClick={() => setDriving(true)}>
              {t('browser.drive')}
            </Button>
          )
        ) : (
          <span className="text-text-muted">{t('browser.watchOnly', { environment: environmentName, managers })}</span>
        )}
      </div>
      {driving && <p className="text-xs text-text-muted">{t('browser.keysBack')}</p>}
      {state.refused !== null && <p role="status" className="text-xs text-danger-text">{refusedMessage}</p>}
      {pasting && tab !== null && <PasteDialog onClose={() => setPasting(false)} onPaste={(text) => send({ op: 'text', tab: tab.id, text })} />}
      <ConfirmDialog open={quitting} onOpenChange={setQuitting} title={t('browser.quit.title', { name: browser.profile ?? t('browser.list.ownProfile') })} confirmLabel={t('browser.quit.confirm')} danger pending={quit.isPending} error={quit.error} onConfirm={() => quit.mutate()}>
        {t('browser.quit.body')}
      </ConfirmDialog>
    </section>
  );
}

function PageDialogOverlay({ state, onAnswer }: { state: BrowserSocketState; onAnswer: (accept: boolean, text?: string) => void }) {
  const { t } = useTranslation();
  const dialog = state.dialog;
  const [text, setText] = useState(dialog?.default ?? '');
  if (dialog === null) return null;
  return (
    <div className="absolute inset-0 flex items-center justify-center bg-bg/70 p-4" data-page-dialog={dialog.kind}>
      <div role="alertdialog" aria-label={t('browser.dialog.label')} className="flex w-full max-w-sm flex-col gap-3 rounded-lg border border-border bg-surface p-4 shadow-lg">
        <p className="text-sm whitespace-pre-wrap wrap-anywhere">{dialog.message}</p>
        {dialog.kind === 'prompt' && <Input label={t('browser.dialog.answer')} value={text} onChange={(event) => setText(event.target.value)} />}
        <div className="flex justify-end gap-2">
          {dialog.kind !== 'alert' && <Button onClick={() => onAnswer(false)}>{dialog.kind === 'beforeunload' ? t('browser.dialog.stay') : t('common.cancel')}</Button>}
          <Button variant="primary" onClick={() => onAnswer(true, dialog.kind === 'prompt' ? text : undefined)}>
            {dialog.kind === 'beforeunload' ? t('browser.dialog.leave') : t('browser.dialog.ok')}
          </Button>
        </div>
      </div>
    </div>
  );
}

function ChooserOverlay({ mayReadFiles, environmentName, multiple, onAnswer }: { mayReadFiles: boolean; environmentName: string; multiple: boolean; onAnswer: (paths: string[] | null) => void }) {
  const { t } = useTranslation();
  const [text, setText] = useState('');
  const paths = text.split('\n').map((line) => line.trim()).filter((line) => line !== '');
  return (
    <div className="absolute inset-0 flex items-center justify-center bg-bg/70 p-4" data-page-chooser>
      <div role="dialog" aria-label={t('browser.chooser.label')} className="flex w-full max-w-md flex-col gap-3 rounded-lg border border-border bg-surface p-4 shadow-lg">
        {mayReadFiles ? (
          <>
            <Textarea label={multiple ? t('browser.chooser.paths') : t('browser.chooser.path')} hint={t('browser.chooser.hint', { environment: environmentName })} value={text} rows={multiple ? 4 : 1} onChange={(event) => setText(event.target.value)} />
            <div className="flex justify-end gap-2">
              <Button onClick={() => onAnswer(null)}>{t('browser.chooser.none')}</Button>
              <Button variant="primary" disabled={paths.length === 0 || (!multiple && paths.length > 1)} onClick={() => onAnswer(paths)}>
                {t('browser.chooser.give')}
              </Button>
            </div>
          </>
        ) : (
          <>
            <p className="text-sm">{t('browser.chooser.cannot', { environment: environmentName })}</p>
            <div className="flex justify-end">
              <Button onClick={() => onAnswer(null)}>{t('browser.chooser.none')}</Button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

function PasteDialog({ onClose, onPaste }: { onClose: () => void; onPaste: (text: string) => void }) {
  const { t } = useTranslation();
  const [text, setText] = useState('');
  return (
    <Dialog
      open
      size="md"
      title={t('browser.paste.title')}
      description={t('browser.paste.lead')}
      onOpenChange={(next) => { if (!next) onClose(); }}
      footer={
        <>
          <Button onClick={onClose}>{t('common.cancel')}</Button>
          <Button variant="primary" disabled={text === ''} onClick={() => { onPaste(text); onClose(); }}>
            {t('browser.paste.do')}
          </Button>
        </>
      }
    >
      <Textarea label={t('browser.paste.text')} value={text} rows={5} onChange={(event) => setText(event.target.value)} />
    </Dialog>
  );
}
