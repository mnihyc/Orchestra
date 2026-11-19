/** The words and the figure of a step's row, made from what the tool call was and not from the server's own line: so they come in the reader's language.
 *
 * A row says what the step did in a few words ("Read src/login.ts · lines 40–120", "Edited 2 files",
 * a command as it was typed), what it came to ("+12 −3", "exit 1", "14 matches") and how it stands. A
 * kind the page has no words for (a tool of Orch's own, one the adapter did not recognise) is told by
 * the title the server gave it.
 */
import type { TFunction } from 'i18next';

import type { Item } from '@/generated/api-types';

export type ToolItem = Extract<Item, { kind: 'tool_call' }>;

export type Tone = 'muted' | 'danger' | 'neutral' | 'success';

export interface StepTitle {
  /** The words of the row; for a command the command itself, which is drawn as typed. */
  text: string;
  /** The text is a command line: it is drawn in mono type behind a prompt. */
  command: boolean;
  /** What it came to, if it came to anything worth a chip. */
  result: string | null;
  tone: Tone;
  /** What the command was for, or the command itself where the words are about what it was for: on hover. */
  hint: string | null;
}

const COMMAND_SHOWN_CHARS = 160;
const ARGUMENT_SHOWN_CHARS = 60;

const oneLine = (text: string, most: number): string => {
  const line = text.split('\n', 1)[0] ?? '';
  return line.length > most ? `${line.slice(0, most - 1)}…` : line;
};

/** The first argument of an MCP call that says something: a word or a sentence, not an object. */
function firstArgument(args: unknown): string | null {
  if (typeof args !== 'object' || args === null) return null;
  for (const value of Object.values(args)) {
    if (typeof value === 'string' && value.trim() !== '') return value.trim();
  }
  return null;
}

/** The host and path of an address, without its scheme. */
function shortAddress(url: string): string {
  try {
    const parsed = new URL(url);
    return `${parsed.host}${parsed.pathname === '/' ? '' : parsed.pathname}`;
  } catch {
    return url;
  }
}

/** Whether the step ended in a way that is a failure: it failed, or a command ended with something other than 0. */
export function stepFailed(item: ToolItem): boolean {
  if (item.status === 'failed') return true;
  const tool = item.payload.tool;
  return tool.kind === 'command' && tool.exit_code !== null && tool.exit_code !== 0;
}

export function describeStep(item: ToolItem, t: TFunction): StepTitle {
  const tool = item.payload.tool;
  const none = { command: false, result: null, tone: 'muted', hint: null } as const;
  switch (tool.kind) {
    case 'command': {
      const exit = tool.exit_code;
      const result =
        exit !== null ? t('timeline.step.exit', { code: exit }) : tool.background ? t('timeline.step.stillRunning') : item.status === 'in_progress' ? null : t('timeline.step.noExit');
      const tone: Tone = exit === null ? 'neutral' : exit === 0 ? 'muted' : 'danger';
      const action = tool.actions[0];
      // Where the harness says what the command was for, the row says that, and the command is under the pointer.
      if (action !== undefined && action.type !== 'other') {
        const text =
          action.type === 'read'
            ? t('timeline.step.read', { path: action.path })
            : action.type === 'list'
              ? action.path === null
                ? t('timeline.step.listHere')
                : t('timeline.step.list', { path: action.path })
              : action.path === null
                ? t('timeline.step.searchAnywhere', { query: action.query })
                : t('timeline.step.search', { query: action.query, path: action.path });
        return { text, command: false, result, tone, hint: tool.command };
      }
      return { text: oneLine(tool.command, COMMAND_SHOWN_CHARS), command: true, result, tone, hint: tool.cwd === '' ? tool.command : `${tool.command}\n${tool.cwd}` };
    }
    case 'file_change': {
      const first = tool.changes[0];
      const added = tool.changes.reduce((sum, change) => sum + change.added, 0);
      const removed = tool.changes.reduce((sum, change) => sum + change.removed, 0);
      const result = t('timeline.step.changed', { added, removed });
      if (tool.changes.length === 1 && first !== undefined) {
        const text =
          first.change === 'add'
            ? t('timeline.step.created', { path: first.path })
            : first.change === 'delete'
              ? t('timeline.step.deleted', { path: first.path })
              : first.change === 'rename' && first.old_path !== null
                ? t('timeline.step.renamed', { from: first.old_path, to: first.path })
                : t('timeline.step.edited', { path: first.path });
        return { text, command: false, result: first.binary ? null : result, tone: 'muted', hint: null };
      }
      return { text: t('timeline.step.editedMany', { count: tool.changes.length }), command: false, result, tone: 'muted', hint: null };
    }
    case 'file_read': {
      const lines =
        tool.start_line !== null && tool.end_line !== null
          ? t('timeline.step.readLines', { path: tool.path, from: tool.start_line, to: tool.end_line })
          : t('timeline.step.read', { path: tool.path });
      return { ...none, text: lines };
    }
    case 'file_search': {
      const where = tool.path === null || tool.path === '' ? null : tool.path;
      const text =
        tool.mode === 'name'
          ? t('timeline.step.listed', { query: tool.query })
          : where === null
            ? t('timeline.step.searchAnywhere', { query: tool.query })
            : t('timeline.step.search', { query: tool.query, path: where });
      const count = tool.total ?? tool.matches.length;
      const result = tool.mode === 'name' ? t('timeline.step.files', { count }) : t('timeline.step.matches', { count });
      return { ...none, text, result: tool.truncated && tool.total === null ? `${count}+` : result };
    }
    case 'web_search':
      return { ...none, text: t('timeline.step.webSearch', { query: tool.query }) };
    case 'web_fetch':
      return { ...none, text: t('timeline.step.webFetch', { address: shortAddress(tool.url) }), result: tool.status === null ? null : String(tool.status), tone: tool.status !== null && tool.status >= 400 ? 'danger' : 'muted' };
    case 'image':
      return { ...none, text: tool.name === null ? t('timeline.step.imageUnnamed') : t('timeline.step.image', { name: tool.name }) };
    case 'mcp': {
      const argument = firstArgument(tool.arguments);
      return {
        ...none,
        text: argument === null ? t('timeline.step.mcp', { server: tool.server, tool: tool.tool }) : t('timeline.step.mcpWith', { server: tool.server, tool: tool.tool, argument: oneLine(argument, ARGUMENT_SHOWN_CHARS) }),
        hint: argument === null ? null : t('timeline.step.mcpWith', { server: tool.server, tool: tool.tool, argument }),
        result: tool.error === null ? null : t('timeline.step.error'),
        tone: 'danger',
      };
    }
    case 'subagent':
      return { ...none, text: tool.name === null ? t('timeline.step.subagentUnnamed') : t('timeline.step.subagent', { name: tool.name }), result: t(`timeline.step.subagentState.${tool.state}`), tone: tool.state === 'failed' ? 'danger' : 'muted' };
    case 'orch':
      return { ...none, text: tool.tool === 'send_message' || tool.tool === 'send_chat_message' || tool.tool === 'send_thread_message'
        ? t('choiceCatalog.sharedTool.send_message.name')
        : item.payload.title === '' ? t('timeline.step.orch', { tool: tool.tool }) : item.payload.title };
    case 'other':
      return { ...none, text: item.payload.title === '' ? tool.name : item.payload.title };
  }
}
