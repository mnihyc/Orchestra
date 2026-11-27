/** Neutral tool-kind dispatch: a display made for each kind, and the raw call under it.
 *
 * One step of the agent's work as a row: an icon for its kind, a title in words, what it came to, how
 * long it took and how it stands (running with a timer, done, failed, refused). The row opens to the
 * display made for its kind, and every display can show the raw call in its place. How far a step is
 * open begins closed; what the reader opens or closes by hand holds while the screen is open.
 */
import {
  Bot,
  Braces,
  ChevronDown,
  ChevronRight,
  FilePen,
  FileText,
  Globe,
  ImageIcon,
  type LucideIcon,
  Plug,
  Search,
  Sparkles,
  SquareTerminal,
  Wrench,
} from 'lucide-react';
import { memo, type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { MenuEntry } from '@/ui/Menu';
import { readData } from '@/api/reads';
import { ReadFeedback } from '@/ui/ReadFeedback';
import { useTimelineContext } from '@/features/timeline/context';
import { useRecordedItem } from '@/features/timeline/historical';
import { ItemFrame } from '@/features/timeline/ItemActions';
import { RawCall, useRawCall } from '@/features/timeline/RawCall';
import { timelineUi, useThreadUi } from '@/features/timeline/state';
import { describeStep, stepFailed, type ToolItem } from '@/features/timeline/titles';
import { StepState } from '@/features/timeline/items/StepState';
import { SubagentRow } from '@/features/timeline/items/SubagentRow';
import { CommandDisplay } from '@/features/timeline/tools/Command';
import { FileChangeDisplay } from '@/features/timeline/tools/FileChange';
import { FileReadDisplay } from '@/features/timeline/tools/FileRead';
import { FileSearchDisplay } from '@/features/timeline/tools/FileSearch';
import { ImageDisplay } from '@/features/timeline/tools/Image';
import { McpDisplay } from '@/features/timeline/tools/Mcp';
import { OrchDisplay } from '@/features/timeline/tools/Orch';
import { OtherDisplay } from '@/features/timeline/tools/Other';
import { SubagentDisplay } from '@/features/timeline/tools/Subagent';
import { WebFetchDisplay } from '@/features/timeline/tools/WebFetch';
import { WebSearchDisplay } from '@/features/timeline/tools/WebSearch';
import { useFormatContext } from '@/lib/appearance';
import { workspacePath } from '@/features/files/FileLink';
import { openFile } from '@/features/files/FileTabs';
import { formatSpan, useRunningClock } from '@/lib/time';
import { useThreadPart } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { IconButton } from '@/ui/IconButton';
import { copyText } from '@/ui/clipboard';
import { cx } from '@/ui/cx';
import { nativeFieldMasked } from '../masking';

const ICONS: Record<ToolItem['payload']['tool']['kind'], LucideIcon> = {
  command: SquareTerminal,
  file_change: FilePen,
  file_read: FileText,
  file_search: Search,
  web_search: Globe,
  web_fetch: Globe,
  image: ImageIcon,
  mcp: Plug,
  subagent: Bot,
  orch: Sparkles,
  other: Wrench,
};

export function Display({ item }: { item: ToolItem }) {
  switch (item.payload.tool.kind) {
    case 'command':
      return <CommandDisplay item={item} />;
    case 'file_change':
      return <FileChangeDisplay item={item} />;
    case 'file_read':
      return <FileReadDisplay item={item} />;
    case 'file_search':
      return <FileSearchDisplay item={item} />;
    case 'web_search':
      return <WebSearchDisplay item={item} />;
    case 'web_fetch':
      return <WebFetchDisplay item={item} />;
    case 'image':
      return <ImageDisplay item={item} />;
    case 'mcp':
      return <McpDisplay item={item} />;
    case 'subagent':
      return <SubagentDisplay item={item} />;
    case 'orch':
      return <OrchDisplay item={item} />;
    case 'other':
      return <OtherDisplay item={item} />;
  }
}

/** The menu entries every step has for its call as the harness made it (Show raw, Copy raw input and output), and the line that says a read of that call failed. */
export function useRawCopies(item: ToolItem, open: boolean): { entries: MenuEntry[]; feedback: ReactNode } {
  const { t } = useTranslation();
  const context = useTimelineContext();
  const read = useThreadReadRef(context.threadId);
  const ui = useThreadUi(context.threadId);
  const rawCall = useRawCall(item, false);
  const [requestedCopy, setRequestedCopy] = useState<'input' | 'output' | null>(null);
  const raw = ui.raw.has(item.id);
  // The call as its harness made it is not on the item: it is read when one of its parts is asked to be copied.
  const copyRaw = async (part: 'input' | 'output') => {
    setRequestedCopy(part);
    const body = readData(rawCall) ?? readData(await rawCall.refetch({ cancelRefetch: false }));
    if (body !== undefined) {
      void copyText(part === 'input' ? body.input.inline : body.output?.inline ?? '');
      setRequestedCopy(null);
    }
  };
  const entries: MenuEntry[] = [];
  if (!open) entries.push({ id: 'raw', label: t('timeline.step.showRaw'), onSelect: () => { timelineUi.setStep(read, item.id, true); if (!raw) timelineUi.toggleRaw(read, item.id); } });
  entries.push({ id: 'copy-raw-input', label: t('timeline.step.copyRawInput'), onSelect: () => { void copyRaw('input'); } });
  if (item.status !== 'in_progress') entries.push({ id: 'copy-raw-output', label: t('timeline.step.copyRawOutput'), onSelect: () => { void copyRaw('output'); } });
  const feedback = !(open && raw) && <ReadFeedback read={{ ...rawCall, refetch: () => requestedCopy === null ? rawCall.refetch({ cancelRefetch: false }) : copyRaw(requestedCopy) }} pending={rawCall.isFetching} title={t('timeline.raw.readFailure')} />;
  return { entries, feedback };
}

function ToolCallRowView({ item }: { item: ToolItem }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const context = useTimelineContext();
  const read = useThreadReadRef(context.threadId);
  const recorded = useRecordedItem(context.threadId, item);
  const thread = useThreadPart(context.threadId, (view) => view.thread);
  const ui = useThreadUi(context.threadId);
  const title = describeStep(item, t);
  const failed = stepFailed(item);
  const running = item.status === 'in_progress' && !recorded;
  const now = useRunningClock(running);
  const tool = item.payload.tool;
  // Every tool starts as one row, including the most recent edit and failed command; reading Verbose, a running one is open as it prints.
  const open = ui.steps.get(item.id) ?? (context.detail !== 'normal' && context.readOnly !== true && running);
  const raw = ui.raw.has(item.id);
  const copies = useRawCopies(item, open);
  const tookMs = running ? Math.max(0, now - Date.parse(item.payload.started_at)) : item.payload.duration_ms;
  const took = tookMs === null || tookMs < 1000 ? null : formatSpan(tookMs, format);
  const Icon = ICONS[tool.kind];
  const singleFile = tool.kind === 'file_change' && tool.changes.length === 1 ? tool.changes[0] : undefined;
  const path = singleFile === undefined || thread === null ? null : workspacePath(singleFile.path, thread.cwd, thread.directory);
  const environmentId = context.inspection === undefined && thread?.environment_permissions.includes('files.view') === true ? thread.environment?.id : undefined;
  const result = recorded && tool.kind === 'command' && tool.background && tool.exit_code === null ? t('timeline.step.recordedRunning') : title.result;

  const extra: MenuEntry[] = [];
  const copy = (text: string) => void copyText(text);
  if (tool.kind === 'command') {
    extra.push({ id: 'copy-command', label: t('timeline.step.copyCommand'), onSelect: () => copy(tool.command) });
    extra.push({ id: 'copy-output', label: t('timeline.step.copyOutput'), onSelect: () => copy(tool.output.inline) });
  }
  if (tool.kind === 'file_change') {
    extra.push({ id: 'copy-patch', label: t('timeline.step.copyPatch'), onSelect: () => copy(tool.changes.map((change) => change.diff ?? '').join('\n')) });
  }
  // Show raw first, then the step's own copies, then the raw copies: the order the menu has always had.
  const entries = [...copies.entries.filter((entry) => entry.id === 'raw'), ...extra, ...copies.entries.filter((entry) => entry.id !== 'raw')];

  return (
    <ItemFrame item={item} label={t('timeline.step.menu')} extra={entries}>
      <div className="flex flex-col gap-1" data-step={item.id}>
        <div className="flex min-w-0 items-center gap-1 pr-[calc(var(--control-sm)+var(--spacing-3))]" data-tool-row>
        <button
          type="button"
          aria-expanded={open}
          onClick={() => timelineUi.setStep(read, item.id, !open)}
          className={cx('flex min-h-8 min-w-0 flex-1 items-center gap-2 rounded-md px-2 py-1 text-left text-sm hover:bg-bg-muted', failed && 'text-danger-text')}
        >
          <span className="inline-flex h-5 shrink-0 items-center gap-2">
            {open ? <ChevronDown className="text-text-subtle" aria-hidden /> : <ChevronRight className="text-text-subtle" aria-hidden />}
            <Icon className={failed ? 'text-danger' : 'text-text-muted'} aria-hidden />
          </span>
            <span data-user-content={['command', 'mcp', 'file_read', 'file_search', 'file_change', 'web_search', 'web_fetch'].includes(tool.kind) || undefined}
              data-overflow-text={tool.kind === 'command' ? title.command ? `$${tool.command}` : tool.command : tool.kind === 'mcp' ? title.hint ?? title.text : title.text} className={cx('min-w-0 flex-1 truncate', title.command && 'font-mono', !failed && 'text-text')}>
              {title.command && <span aria-hidden className="mr-1 text-text-subtle">$</span>}
              {title.text}
            </span>
          {result !== null && (
            <span
              className={cx(
                'shrink-0 rounded-full border px-2 text-xs tabular-nums',
                title.tone === 'danger' ? 'border-danger/40 bg-danger-subtle text-danger-text' : 'border-border text-text-muted',
              )}
            >
              {result}
            </span>
          )}
          <span className="inline-flex h-5 shrink-0 items-center"><StepState item={item} failed={failed} took={took} recorded={recorded} /></span>
        </button>
        {open && <IconButton size="sm" icon={<Braces aria-hidden />} label={raw ? t('timeline.step.showDisplay') : t('timeline.step.showRaw')} pressed={raw}
          onClick={() => timelineUi.toggleRaw(read, item.id)} />}
        {open && singleFile !== undefined && singleFile.change !== 'delete' && path !== null && environmentId !== undefined && <IconButton size="sm" icon={<FileText aria-hidden />} label={t('files.link.open', { path })}
          blocked={nativeFieldMasked(item.payload, '/tool/changes/0/path')} reason={nativeFieldMasked(item.payload, '/tool/changes/0/path') ? t('timeline.masking.path') : undefined}
          onClick={() => openFile({ environmentId, threadId: context.threadId, path, line: null, permanent: false })} />}
        </div>
        {copies.feedback}
        {open && (
          <div className="ml-4 flex flex-col gap-2 border-l border-border pl-3">
            {raw ? <RawCall item={item} /> : <Display item={item} />}
          </div>
        )}
      </div>
    </ItemFrame>
  );
}

const MemoRow = memo(ToolCallRowView);

/** A step of the agent's work, read from the store by its id so that only this row is drawn again when it changes. */
export function ToolCallRow({ threadId, id }: { threadId: string; id: string }) {
  const item = useThreadPart(threadId, (view) => view.items.get(id));
  if (item?.kind !== 'tool_call') return null;
  // A sub-agent is told as its program tells it: its kind, its task and how it is getting on, opening into its own conversation.
  return item.payload.tool.kind === 'subagent' ? <SubagentRow item={item} /> : <MemoRow item={item} />;
}
