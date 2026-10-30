/** What the person chooses for an archive besides which rows are ticked, and the request all of it comes to.
 *
 * The kinds of thing it holds, the form they are written in, how much of each conversation, whether the attachments
 * (the files that messages and documents carry) go with it, the earlier versions and the comments of documents, and a span of
 * days. A request says only what differs from what the server does when it is not told: the same choices, made
 * the same way, always come to the same request.
 */
import type { ExportDetail, ExportFormat, ExportRequest } from '@/generated/api-types';

import type { Chosen, Wanted } from './selection';

/** A place can also be exported in the package form that can be imported again. */
export type PlaceFormat = ExportFormat | 'package';

export interface Choices {
  conversations: boolean;
  chats: boolean;
  documents: boolean;
  format: PlaceFormat;
  detail: ExportDetail;
  files: boolean;
  versions: boolean;
  comments: boolean;
  /** A day, `YYYY-MM-DD`, or nothing. */
  from: string;
  to: string;
}

/** What the dialog opens with: all three kinds, written to be read, as the people said it, and nothing besides. */
export const OPENING_CHOICES: Choices = {
  conversations: true,
  chats: true,
  documents: true,
  format: 'markdown',
  detail: 'normal',
  files: false,
  versions: false,
  comments: false,
  from: '',
  to: '',
};

const DAY = /^\d{4}-\d{2}-\d{2}$/;

/** Whether a field of days holds a day that exists, or nothing. */
export function isDay(text: string): boolean {
  if (text === '') return true;
  if (!DAY.test(text)) return false;
  const moment = new Date(`${text}T00:00:00.000Z`);
  return !Number.isNaN(moment.getTime()) && moment.toISOString().startsWith(text);
}

/** What is wrong with the span of days, if anything: a day that is none, or an end before the beginning. */
export function spanFault(choices: Pick<Choices, 'from' | 'to'>): 'day' | 'order' | null {
  if (!isDay(choices.from) || !isDay(choices.to)) return 'day';
  if (choices.from !== '' && choices.to !== '' && choices.to < choices.from) return 'order';
  return null;
}

/** The list a request names, left out where it names nothing. */
function listed(ids: readonly string[]): string[] | undefined {
  return ids.length === 0 ? undefined : [...ids];
}

/** The request that a choice of rows and a set of choices come to. */
export function requestOf(wanted: Extract<Wanted, { type: 'all' | 'chosen' }>, choices: Choices): ExportRequest {
  const what = (chosen: Chosen): ExportRequest['what'] => {
    const nodes = listed(chosen.nodes);
    const documents = listed(chosen.documents);
    const chats = listed(chosen.chats);
    return { ...(nodes === undefined ? {} : { nodes }), ...(documents === undefined ? {} : { documents }), ...(chats === undefined ? {} : { chats }) };
  };
  const unwanted = {
    ...(choices.conversations ? {} : { conversations: false }),
    ...(choices.chats ? {} : { chats: false }),
    ...(choices.documents ? {} : { documents: false }),
  };
  return {
    what: wanted.type === 'all' ? 'all' : what(wanted.chosen),
    ...(Object.keys(unwanted).length === 0 ? {} : { parts: unwanted }),
    ...(choices.format === 'package' ? { package: true } : choices.format === 'markdown' ? {} : { format: choices.format }),
    ...(choices.detail === 'normal' ? {} : { detail: choices.detail }),
    ...(choices.format === 'package' ? { files: choices.files } : choices.files ? { files: true } : {}),
    // Versions and comments belong to documents: asked for with documents left out, they would be asked for nothing.
    ...(choices.versions && choices.documents ? { versions: true } : {}),
    ...(choices.format === 'package' ? { comments: choices.comments && choices.documents } : choices.comments && choices.documents ? { comments: true } : {}),
    ...(choices.from === '' ? {} : { from: choices.from }),
    ...(choices.to === '' ? {} : { to: choices.to }),
  };
}

/** Whether no kind of thing is wanted at all. */
export function wantsNothing(choices: Pick<Choices, 'conversations' | 'chats' | 'documents'>): boolean {
  return !choices.conversations && !choices.chats && !choices.documents;
}
