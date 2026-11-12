/** Development-only controller for the real summary component; no model, route or API is simulated as ready. */
import { useRef, useState } from 'react';
import { flushSync } from 'react-dom';
import { createRoot, type Root } from 'react-dom/client';
import englishArticle from '../../../docs/user/getting-started.md?raw';
import chineseArticle from '../../../docs/user/zh-CN/getting-started.md?raw';
import { SummaryBox } from '@/features/summaries/SummaryBox';
import type { SummaryAskContext, SummarySnapshot, SummaryTarget } from '@/features/summaries/model';
import { readArticle } from '@/help/guide';
import { i18n } from '@/i18n';
import { Button } from '@/ui/Button';
import { TooltipProvider } from '@/ui/Tooltip';

let root: Root | undefined;
let host: HTMLDivElement | undefined;

function Preview({ chinese }: { chinese: boolean }) {
  const words = chinese ? {
    previous: '审阅已就绪', updated: '审阅已通过，接下来发布', source: '测试摘要器',
    complete: '完成刷新', fail: '刷新失败', noSummarizer: '显示未设置摘要器', written: '人工帮助摘要',
  } : {
    previous: 'The review is ready', updated: 'The review was accepted; publish next', source: 'Scripted summarizer',
    complete: 'Complete refresh', fail: 'Fail refresh', noSummarizer: 'Show no summarizer', written: 'Written help',
  };
  const [target, setTarget] = useState<SummaryTarget>({ kind: 'thread', reference: '/thread/summary-preview' });
  const [summary, setSummary] = useState<SummarySnapshot>({ revision: '1', state: 'fresh', text: words.previous,
    producer: { kind: 'extension', name: words.source }, updated_at: null });
  const [requests, setRequests] = useState(0);
  const [configured, setConfigured] = useState(0);
  const [asked, setAsked] = useState<SummaryAskContext | null>(null);
  const waiting = useRef<{ resolve: () => void; reject: (error: unknown) => void } | null>(null);
  const refresh = () => new Promise<void>((resolve, reject) => {
    waiting.current = { resolve, reject };
    setRequests((count) => count + 1);
  });
  return <TooltipProvider><div className="flex min-w-0 flex-col gap-6 bg-bg p-4 text-text">
    <SummaryBox readerKey="preview-reader" target={target} summary={summary} onRefresh={refresh} onConfigure={() => setConfigured((count) => count + 1)} onAsk={setAsked} />
    <div className="flex flex-wrap gap-2" data-summary-controller>
      <Button size="sm" onClick={() => {
        setSummary((previous) => ({ ...previous, revision: `${Number(previous.revision) + 1}`, state: 'fresh', text: words.updated }));
        waiting.current?.resolve(); waiting.current = null;
      }}>{words.complete}</Button>
      <Button size="sm" onClick={() => { waiting.current?.reject(new Error('A synthetic refresh failed')); waiting.current = null; }}>{words.fail}</Button>
      <Button size="sm" onClick={() => setSummary((previous) => ({ ...previous, revision: `${Number(previous.revision) + 1}`, state: 'no_summarizer', text: null, producer: null }))}>{words.noSummarizer}</Button>
      <Button size="sm" onClick={() => {
        const article = readArticle('getting-started', chinese ? chineseArticle : englishArticle);
        setTarget({ kind: 'help', reference: '/help/getting-started' });
        setSummary({ revision: 'help', state: 'fresh', text: article.summary ?? null, producer: { kind: 'written' }, updated_at: null });
      }}>{words.written}</Button>
    </div>
    <output aria-label="refresh requests">{requests}</output>
    <output aria-label="configuration requests">{configured}</output>
    <output aria-label="Ask context">{JSON.stringify(asked)}</output>
  </div></TooltipProvider>;
}

export async function mountSummaryBox(locale: 'en' | 'zh-CN', theme: 'light' | 'dark'): Promise<void> {
  flushSync(() => root?.unmount()); host?.remove();
  await i18n.changeLanguage(locale);
  document.documentElement.lang = locale; document.documentElement.dataset.theme = theme;
  host = document.createElement('div'); host.dataset.summaryPreview = 'true';
  Object.assign(host.style, { position: 'fixed', inset: '0', zIndex: '100', overflow: 'auto', background: 'var(--color-bg)' });
  document.body.append(host); root = createRoot(host);
  flushSync(() => root?.render(<Preview chinese={locale === 'zh-CN'} />));
}
