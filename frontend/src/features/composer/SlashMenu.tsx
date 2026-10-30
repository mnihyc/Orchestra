/** Conversation actions, a side thread and a fork among them.
 *
 * What a person types after a slash at the start of the box: the list of commands that begin with what was
 * typed, to be walked with the arrow keys and run with Enter or Tab, or a command written out with its
 * argument. The commands here set up the model a thread runs on: `/model` and `/effort` open the model
 * control, and `/speed`, `/access` and `/context` open it on that choice or, written with what to choose
 * (`/speed fast`, `/access none`, `/context 400k`), make it, each only where the thread's model has it
 * open. A command that this reader may not use is listed disabled, with the reason. A message that begins
 * with a slash and is no command goes as a message. The commands of a side thread and a fork are added to
 * the list by the screens that have them.
 */
import { type KeyboardEvent, type ReactNode, useId, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { contextFits, forceLabel, type Kind, type State } from '@/features/composer/modelState';
import { useRunsWith } from '@/features/composer/runsWith';
import { appliesWords } from '@/features/composer/SpeedAndAccess';
import { formatContext } from '@/features/models/context';
import type { Thread } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { holds } from '@/lib/permissions';
import { cx } from '@/ui/cx';
import { StateLine } from '@/ui/StateLine';
import { toast } from '@/ui/Toast';

/** One command of the list. */
export interface SlashCommand {
  /** What follows the slash. */
  name: string;
  /** What it does, in a few words. */
  about: string;
  /** Why it cannot be used by this reader; none when it can. */
  off: string | null;
  /** Runs it with what was written after its name; answers what is wrong with the argument, or nothing. */
  run: (argument: string) => string | null;
}

/** What the box holds, read as a slash command: its name and what was written after it, or nothing where the text is no command. */
export function readSlash(text: string): { name: string; argument: string; typing: boolean } | null {
  const match = /^\/([a-z]*)(\s[^\n]*)?$/i.exec(text);
  if (match === null) return null;
  const argument = (match[2] ?? '').trim();
  return { name: (match[1] ?? '').toLowerCase(), argument, typing: match[2] === undefined };
}

/** The size a person wrote: "400k", "1M", "1.5m", "400000", with the grouping people put in a long number. */
export function readSize(text: string): number | null {
  const match = /^(\d+(?:[.,]\d+)?)\s*([kKmM])?$/.exec(text.trim().replace(/[\s_]/g, ''));
  if (match === null) return null;
  const figure = Number((match[1] ?? '').replace(',', '.'));
  const unit = (match[2] ?? '').toLowerCase();
  const size = Math.round(figure * (unit === 'm' ? 1_000_000 : unit === 'k' ? 1_000 : 1));
  return Number.isSafeInteger(size) && size > 0 ? size : null;
}

/** The commands of the model control, as this reader meets them on this thread. */
export function useModelCommands(thread: Thread, openControl: (section: Kind | null) => void): SlashCommand[] {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const runs = useRunsWith(thread, false);
  const off = holds(thread.access, 'thread.manage') ? null : t('composer.model.readOnly', { who: thread.owner.display_name });

  const say = (state: State, value: string): string | null => {
    toast({ tone: 'success', title: t('composer.model.slash.done', { what: t(`composer.model.${state.kind}.title`), value }), description: appliesWords(state, t) });
    return null;
  };

  // A thread that is bound to nothing has no model to set up.
  if (thread.binding === null) return [];
  const commands: SlashCommand[] = [
    { name: 'model', about: t('composer.model.slash.model'), off, run: () => (openControl(null), null) },
    { name: 'effort', about: t('composer.model.slash.effort'), off, run: () => (openControl(null), null) },
  ];
  for (const state of runs.states) {
    const name = state.kind;
    const about = t(`composer.model.slash.${name}`);
    if (state.kind === 'context') {
      commands.push({
        name,
        about,
        off,
        run: (argument) => {
          if (argument === '') return (openControl(name), null);
          const size = readSize(argument);
          if (size === null) return t('composer.model.slash.sizeInvalid');
          if (size < state.least) return t('composer.model.context.faultLow', { least: formatContext(state.least, locale) });
          if (!contextFits(size, { context_max: state.largest }, state.least)) return t('composer.model.context.faultHigh', { largest: formatContext(state.largest, locale) });
          runs.choose(state, size);
          return say(state, formatContext(size, locale));
        },
      });
      continue;
    }
    commands.push({
      name,
      about,
      off,
      run: (argument) => {
        if (argument === '') return (openControl(name), null);
        const wanted = argument.toLowerCase();
        const none = name === 'speed' ? t('composer.model.speed.standard') : t('composer.model.access.none');
        const noneWords = [none.toLowerCase(), 'none', 'standard', 'off'];
        const found = noneWords.includes(wanted) ? ({ id: null, label: none } as const) : (state.open.find((one) => one.id.toLowerCase() === wanted || one.label.toLowerCase() === wanted) ?? null);
        if (found === null) return t('composer.model.slash.notOpen', { what: t(`composer.model.${name}.title`), given: argument, list: [none, ...state.open.map((one) => one.label)].join(', ') });
        runs.choose(state, found.id);
        return say(state, found.id === null ? none : (forceLabel({ ...state, force: found.id }) ?? found.label));
      },
    });
  }
  return commands;
}

export interface SlashMenu {
  /** The list of commands, to float above the box; nothing while the box holds no command. */
  list: ReactNode;
  /** What is wrong with the command that was written, for under the box. */
  problem: ReactNode;
  /** For the box: what it controls and which option is active while the list is open. */
  boxProps: { 'aria-controls'?: string; 'aria-activedescendant'?: string };
  /** Offers the key to the list: true when it took it. */
  onKeyDown: (event: KeyboardEvent<HTMLTextAreaElement>) => boolean;
}

/** The slash menu of the box: what the text says, the commands that fit it, and the keys that walk and run them. */
export function useSlashMenu(text: string, commands: readonly SlashCommand[], done: () => void): SlashMenu {
  const { t } = useTranslation();
  const listId = useId();
  const [active, setActive] = useState(0);
  const [dismissedFor, setDismissedFor] = useState<string | null>(null);
  const [wrong, setWrong] = useState<{ for: string; words: string } | null>(null);
  const read = readSlash(text);
  const open = read !== null && read.typing && dismissedFor !== text;
  const shown = open ? commands.filter((command) => command.name.startsWith(read.name)) : [];
  const at = Math.min(active, Math.max(shown.length - 1, 0));
  const idOf = (name: string) => `${listId}-${name}`;

  /** Runs a command; what is wrong with its argument stays under the box until the text changes. */
  const run = (command: SlashCommand, argument: string) => {
    if (command.off !== null) {
      setWrong({ for: text, words: command.off });
      return;
    }
    const problem = command.run(argument);
    if (problem === null) done();
    else setWrong({ for: text, words: problem });
  };

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>): boolean => {
    if (read === null) return false;
    const plain = !event.shiftKey && !event.ctrlKey && !event.metaKey && !event.altKey;
    if (open && shown.length > 0) {
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault();
        setActive((at + (event.key === 'ArrowDown' ? 1 : -1) + shown.length) % shown.length);
        return true;
      }
      if (event.key === 'Escape') {
        event.preventDefault();
        setDismissedFor(text);
        return true;
      }
      if ((event.key === 'Enter' && plain) || (event.key === 'Tab' && !event.shiftKey)) {
        const command = shown[at];
        if (command === undefined) return false;
        event.preventDefault();
        run(command, '');
        return true;
      }
      return false;
    }
    // A command written out with its argument: Enter runs it. A message that only begins with a slash is no command.
    if (event.key === 'Enter' && plain && !read.typing) {
      const command = commands.find((candidate) => candidate.name === read.name);
      if (command === undefined) return false;
      event.preventDefault();
      run(command, read.argument);
      return true;
    }
    if (event.key === 'Enter' && plain && read.typing && read.name !== '') {
      const command = commands.find((candidate) => candidate.name === read.name);
      if (command === undefined) return false;
      event.preventDefault();
      run(command, '');
      return true;
    }
    return false;
  };

  const list =
    open && shown.length > 0 ? (
      <ul
        id={listId}
        role="listbox"
        aria-label={t('composer.model.slash.list')}
        // It floats above the box: the conversation keeps the room it has.
        className="absolute inset-x-0 bottom-full z-(--layer-dropdown) mb-2 flex flex-col gap-1 rounded-lg border border-border bg-surface-raised p-1 shadow-md"
      >
        {shown.map((command, index) => (
          <li
            key={command.name}
            id={idOf(command.name)}
            role="option"
            aria-selected={index === at}
            aria-disabled={command.off !== null || undefined}
            className={cx('flex cursor-default flex-col rounded-md px-2 py-1 text-sm', index === at ? 'bg-accent-subtle' : 'hover:bg-bg-muted', command.off !== null && 'text-text-muted')}
            // The pointer runs a command as Enter does; the box keeps the keyboard.
            onMouseDown={(event) => event.preventDefault()}
            onClick={() => run(command, '')}
          >
            <span className="flex flex-wrap items-baseline gap-x-2">
              <code className="font-medium">/{command.name}</code>
              <span className="text-text-muted">{command.about}</span>
            </span>
            {command.off !== null && <span className="text-xs text-text-muted">{command.off}</span>}
          </li>
        ))}
      </ul>
    ) : null;
  const problem =
    wrong !== null && wrong.for === text ? (
      <StateLine role="alert" tone="danger" compact message={wrong.words} />
    ) : null;

  return {
    list,
    problem,
    boxProps: open && shown.length > 0 ? { 'aria-controls': listId, 'aria-activedescendant': idOf(shown[at]?.name ?? '') } : {},
    onKeyDown,
  };
}
