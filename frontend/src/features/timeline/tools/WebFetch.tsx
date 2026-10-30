/** A page the agent fetched: its address, its title and its status.
 *
 * A page the agent fetched: its title, its address and the status it answered with, and a screen of
 * the text that came back, with "Show all" for the rest of what is held.
 */
import { ExternalLink } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { Item } from '@/generated/api-types';
import { isSafeHref } from '@/lib/markdown';
import { Button } from '@/ui/Button';

type FetchItem = Extract<Item, { kind: 'tool_call' }>;

/** A screen of the page's text: about forty lines of it. */
const SCREEN_CHARS = 3_000;

export function WebFetchDisplay({ item }: { item: FetchItem }) {
  const { t } = useTranslation();
  const [all, setAll] = useState(false);
  const tool = item.payload.tool;
  if (tool.kind !== 'web_fetch') return null;
  const text = tool.content.inline;
  const cut = !all && text.length > SCREEN_CHARS;
  return (
    <div className="flex flex-col gap-2">
      <p className="flex flex-wrap items-center gap-2 text-sm">
        {tool.title !== null && tool.title !== '' && <span className="font-medium text-text">{tool.title}</span>}
        {isSafeHref(tool.url) ? (
          <a href={tool.url} target="_blank" rel="noopener noreferrer" className="inline-flex items-center gap-1 text-accent-text underline underline-offset-2">
            <span className="truncate">{tool.url}</span>
            <ExternalLink aria-hidden />
            <span className="sr-only">{t('timeline.web.newTab')}</span>
          </a>
        ) : (
          <span className="text-text-muted">{tool.url}</span>
        )}
        {tool.status !== null && <span className="text-text-subtle">{tool.status}</span>}
      </p>
      {text !== '' && (
        <pre className="max-h-(--output-height) overflow-auto rounded-lg border border-border bg-bg-inset p-3 text-sm break-words whitespace-pre-wrap">{cut ? text.slice(0, SCREEN_CHARS) : text}</pre>
      )}
      {cut && (
        <Button size="sm" variant="ghost" className="self-start" onClick={() => setAll(true)}>
          {t('timeline.web.showAll')}
        </Button>
      )}
    </div>
  );
}
