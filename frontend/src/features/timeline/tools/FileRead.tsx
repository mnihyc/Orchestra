/** A file the agent read: its path as a link, the lines, and an excerpt.
 *
 * What the agent was given when it read a file: the excerpt with its line numbers, from the line it
 * began at. A file that is an image is told by its type, as the picture itself is kept by the
 * environment's files.
 */
import { useTranslation } from 'react-i18next';

import { FileLink, FilePathAttachment } from '@/features/files/FileLink';
import type { Item } from '@/generated/api-types';

type ReadItem = Extract<Item, { kind: 'tool_call' }>;

export function FileReadDisplay({ item }: { item: ReadItem }) {
  const { t } = useTranslation();
  const tool = item.payload.tool;
  if (tool.kind !== 'file_read') return null;
  if (tool.media_type?.startsWith('image/') === true) {
    return <FilePathAttachment path={tool.path} mediaType={tool.media_type} />;
  }
  const first = tool.start_line ?? 1;
  const lines = tool.content.inline.split('\n');
  return (
    <div className="flex flex-col gap-1">
      <p className="font-mono text-sm text-text wrap-anywhere">
        <FileLink path={tool.path} line={tool.start_line} />
      </p>
      <div className="max-h-(--output-height) overflow-auto rounded-lg border border-border bg-bg-inset">
        <table aria-label={t('timeline.read.excerpt', { path: tool.path })} className="w-full border-collapse font-mono text-sm">
          <tbody>
            {lines.map((line, index) => (
              <tr key={index}>
                <td className="w-px min-w-10 px-2 text-right text-xs text-text-muted tabular-nums select-none">{first + index}</td>
                <td className="pr-3 whitespace-pre-wrap break-words">{line === '' ? ' ' : line}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {tool.content.truncated && <p className="text-xs text-text-subtle">{t('timeline.read.cut')}</p>}
      {tool.total_lines !== null && <p className="text-xs text-text-subtle">{t('timeline.read.total', { count: tool.total_lines })}</p>}
    </div>
  );
}
