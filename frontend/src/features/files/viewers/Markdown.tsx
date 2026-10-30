/** Markdown: the source beside the rendered text, or either alone; raw HTML never rendered.
 *
 * The source is the code viewer, editable by whoever may write the file; the rendered text is drawn by
 * the page's own Markdown, which never emits raw HTML, from the text as it stands after each pause in
 * typing. The reader chooses the source, the rendering, or both side by side.
 */
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { Markdown } from '@/ui/Markdown';
import { SegmentedControl } from '@/ui/SegmentedControl';

import { useFileText } from '../content';
import type { ViewerProps } from '../viewers';
import CodeViewer from './Code';

type Mode = 'both' | 'source' | 'text';

export default function MarkdownViewer(props: ViewerProps) {
  const { t } = useTranslation();
  const [mode, setMode] = useState<Mode>('both');
  const loaded = useFileText(props.door, props.path, props.head.brought);
  const [edited, setEdited] = useState<string | null>(null);
  const text = edited ?? loaded.text?.text ?? '';
  return (
    <div className="@container flex h-full min-h-0 flex-col" data-viewer="markdown">
      <div className="flex items-center gap-2 border-b border-border px-2 py-1">
        <SegmentedControl<Mode>
          size="sm"
          label={t('files.markdown.mode')}
          value={mode}
          onValueChange={setMode}
          segments={[
            { value: 'source', ...choiceOption(t, 'markdownView.source') },
            { value: 'both', ...choiceOption(t, 'markdownView.both') },
            { value: 'text', ...choiceOption(t, 'markdownView.text') },
          ]}
        />
      </div>
      <div className={mode === 'both' ? 'grid min-h-0 flex-1 grid-cols-1 grid-rows-2 gap-px bg-border @3xl:grid-cols-2 @3xl:grid-rows-1' : 'min-h-0 flex-1'}>
        {mode !== 'text' && (
          <div className="min-h-0 min-w-0 bg-surface">
            <CodeViewer {...props} onText={setEdited} />
          </div>
        )}
        {mode !== 'source' && (
          <div className="min-h-0 min-w-0 overflow-auto bg-surface p-4" data-testid="markdown-text">
            <Markdown text={text} />
          </div>
        )}
      </div>
    </div>
  );
}
