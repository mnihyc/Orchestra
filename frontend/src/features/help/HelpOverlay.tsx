/** Help is a view, never a second prompt. A paused work subtree retains its forms and drafts. */
import { Activity, type ReactNode, useLayoutEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { HelpContent } from '@/features/help/HelpView';
import { useHelpState } from '@/features/help/state';
import { Button } from '@/ui/Button';
import { SettingSourceJumpHost } from '@/features/help/SourceJump';
import { PromptScope, usePrompt } from '@/ui/PromptScope';

export function HelpWork({ children }: { children: ReactNode }) {
  const reading = useHelpState((state) => state.overlay);
  return <Activity mode={reading ? 'hidden' : 'visible'}><div className="contents" data-help-work>{children}</div></Activity>;
}

export function HelpOverlayHost() {
  const reading = useHelpState((state) => state.overlay);
  return <><HelpWork><SettingSourceJumpHost /></HelpWork>{reading && <HelpReadingView />}</>;
}

function HelpReadingView() {
  const { t } = useTranslation();
  const request = useHelpState((state) => state.readingRequest);
  const layer = usePrompt(true, 'reading', request);
  const article = useHelpState((state) => state.article);
  const Title = article === null ? 'h1' : 'span';
  const back = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLElement>(null);
  const focused = useRef<HTMLElement | null>(null);
  useLayoutEffect(() => {
    if (!layer.visible || layer.covered) return;
    const previous = focused.current;
    (previous !== null && previous.isConnected && panel.current?.contains(previous) ? previous : back.current)?.focus();
  }, [layer.visible, layer.covered]);
  return <PromptScope layer={layer}><main ref={panel} aria-label={t('views.help.title')} className="flex h-dvh min-h-0 flex-col bg-bg text-text" style={layer.style}
    onFocusCapture={(event) => { if (event.target instanceof HTMLElement) focused.current = event.target; }}
    data-help-reading data-main-area data-area-stop tabIndex={-1} data-prompt-current={!layer.covered} data-prompt-id={layer.id} onKeyDown={(event) => {
    if (event.key === 'F1') event.preventDefault();
    if (event.key === 'Escape' && !event.defaultPrevented && !event.nativeEvent.isComposing) { event.preventDefault(); event.stopPropagation(); useHelpState.setState({ overlay: false }); }
  }}>
    <header className="flex shrink-0 items-center gap-3 border-b border-border p-3">
      <Button ref={back} onClick={() => useHelpState.setState({ overlay: false })}>{t('helpFlow.back')}</Button>
      <Title className="text-lg font-semibold">{t('views.help.title')}</Title>
    </header>
    <div className="mx-auto min-h-0 w-full max-w-(--container-prose) flex-1"><HelpContent /></div>
  </main></PromptScope>;
}
