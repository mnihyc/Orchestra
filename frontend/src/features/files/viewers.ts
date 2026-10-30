/** The list of viewers, most particular first: what each takes, what it shows and whether it edits; a list to which another viewer is added.
 *
 * Which viewer opens a file is decided here and nowhere else. A viewer is one module that says what
 * it takes, what it shows and whether it edits, and is fetched when a file of its kind is first
 * opened; a further kind of file is a module in `viewers/` and a line in this list. A viewer never
 * runs what it shows: HTML is code, an SVG is an image, an archive is listed and never unpacked.
 * Any file can be opened as text, so the code viewer takes everything and comes last among those
 * that show content; the viewer for what has none is the very last.
 */
import type { ComponentType } from 'react';
import { create } from 'zustand';

import type { Door } from '@/api/workspace';
import type { FileStat, Thread } from '@/generated/api-types';
import type { TranslationKey } from '@/i18n';

import type { FileHead } from './content';
import { extensionOf } from './languages';

/** How much of a text is read and shown: beyond it the first part is shown, read-only, with how much there is. */
export const PREVIEW_BYTES = 2 * 1024 * 1024;

/** The longest an image, a film or a sound is read into the page to be shown. */
export const MEDIA_MOST_BYTES = 64 * 1024 * 1024;

export type ViewerId = 'difference' | 'markdown' | 'image' | 'pdf' | 'archive' | 'media' | 'code' | 'none';

/** What is known of a file when the viewer is chosen. */
export interface FileFacts {
  name: string;
  extension: string;
  mediaType: string;
  isText: boolean;
  size: number;
}

/** What a viewer is given to draw. */
export interface ViewerProps {
  door: Door;
  path: string;
  /** What the file is and the version of it that is ready to be read. */
  head: FileHead;
  /** The line to go to, for a viewer of text. */
  line: number | null;
  /** Identity of an explicit line-opening request; repeated requests preserve the mounted viewer. */
  lineRequest?: number | undefined;
  /** The tab that shows it: a viewer that holds unsaved text marks that tab. */
  tabId: string;
  /** Why the file cannot be edited by this person now, or `null` when it can be. */
  whyReadOnly: ReadOnlyReason | null;
  /** The thread the file is read through: the agent that works there is the one who may have changed it. */
  thread: Thread | null;
  /** Opens the same file in another viewer: what a page that shows nothing offers. */
  openAs: (viewer: ViewerId) => void;
  /** Told the text as it stands after each pause in typing: what a viewer that shows it another way draws from. */
  onText?: (text: string) => void;
}

/** Why a file cannot be edited where it is shown. */
export type ReadOnlyReason =
  /** Phone sheets are for reading; editing remains in the wide code view. */
  | { kind: 'small_screen' }
  /** The reader holds no `files.manage` on the thread. */
  | { kind: 'permission' }
  /** The environment is offline, and this is its copy from a time. */
  | { kind: 'offline'; environment: string; asOf: string | null }
  /** The environment (or the machine's owner) takes no files from Orch. */
  | { kind: 'no_writes'; machine: string | null }
  /** The viewer does not edit this kind of file. */
  | { kind: 'viewer' }
  /** The text is longer than is read, and is shown to a point. */
  | { kind: 'too_large'; shown: number; total: number };

export interface Viewer {
  id: ViewerId;
  /** Its name in words, in the menu that opens a file with it. */
  title: TranslationKey;
  /** Whether it takes this file. */
  takes: (file: FileFacts) => boolean;
  /** Whether a file it shows can be edited in it. */
  edits: boolean;
  /** Brings the module that draws it. */
  load: () => Promise<{ default: ComponentType<ViewerProps> }>;
}

const MARKDOWN = new Set(['md', 'markdown', 'mdx']);
const IMAGES = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'avif', 'bmp', 'ico', 'svg', 'heic', 'heif', 'tif', 'tiff']);
const MEDIA = new Set(['mp3', 'wav', 'ogg', 'oga', 'opus', 'flac', 'm4a', 'aac', 'mp4', 'webm', 'ogv', 'mov', 'm4v']);
const ARCHIVES = ['.zip', '.tar', '.tar.gz', '.tgz', '.tar.zst', '.tar.xz', '.tar.bz2', '.tbz2', '.txz'] as const;

const isArchive = (name: string): boolean => ARCHIVES.some((ending) => name.toLowerCase().endsWith(ending));

/** The page that says a file has no preview: the viewer that is there when no list names one. */
const fallback: Viewer = {
  id: 'none',
  title: 'files.viewers.none',
  takes: () => true,
  edits: false,
  load: () => import('./viewers/NoPreview'),
};

/** The viewers there are, most particular first. */
export const VIEWERS: readonly Viewer[] = [
  {
    id: 'difference',
    title: 'files.viewers.difference',
    takes: (file) => file.isText && (file.extension === 'diff' || file.extension === 'patch'),
    edits: false,
    load: () => import('./viewers/Difference'),
  },
  {
    id: 'markdown',
    title: 'files.viewers.markdown',
    takes: (file) => file.isText && MARKDOWN.has(file.extension),
    edits: true,
    load: () => import('./viewers/Markdown'),
  },
  {
    id: 'image',
    title: 'files.viewers.image',
    takes: (file) => IMAGES.has(file.extension) || (file.mediaType.startsWith('image/') && !file.isText),
    edits: false,
    load: () => import('./viewers/Image'),
  },
  {
    id: 'archive',
    title: 'files.viewers.archive',
    takes: (file) => isArchive(file.name),
    edits: false,
    load: () => import('./viewers/Archive'),
  },
  {
    id: 'pdf',
    title: 'files.viewers.pdf',
    takes: (file) => file.extension === 'pdf' || file.mediaType === 'application/pdf',
    edits: false,
    load: () => import('./viewers/Pdf'),
  },
  {
    id: 'media',
    title: 'files.viewers.media',
    takes: (file) => MEDIA.has(file.extension) || file.mediaType.startsWith('audio/') || file.mediaType.startsWith('video/'),
    edits: false,
    load: () => import('./viewers/Media'),
  },
  {
    id: 'code',
    title: 'files.viewers.code',
    // Any file can be opened as text; a file that is no text is offered it too, by the menu.
    takes: () => true,
    edits: true,
    load: () => import('./viewers/Code'),
  },
  {
    id: 'none',
    title: 'files.viewers.none',
    takes: () => true,
    edits: false,
    load: () => import('./viewers/NoPreview'),
  },
];

/** What is known of a file from its description. */
export function factsOf(stat: Pick<FileStat, 'name' | 'media_type' | 'is_text' | 'size'>): FileFacts {
  return { name: stat.name, extension: extensionOf(stat.name), mediaType: stat.media_type, isText: stat.is_text, size: stat.size };
}

/** The viewer that opens a file when nothing was chosen: the first that takes it, and for a file that is no text and that no viewer shows, the page that says so. */
export function defaultViewer(file: FileFacts, list: readonly Viewer[] = VIEWERS): Viewer {
  const found = list.find((viewer) => viewer.id !== 'code' && viewer.id !== 'none' && viewer.takes(file));
  if (found !== undefined) return found;
  const wanted = file.isText ? 'code' : 'none';
  return list.find((viewer) => viewer.id === wanted) ?? fallback;
}

/** The viewers that take a file, for the menu "Open with…": its own, then the text viewer, which takes anything. */
export function viewersFor(file: FileFacts, list: readonly Viewer[] = VIEWERS): Viewer[] {
  return list.filter((viewer) => viewer.id !== 'none' && viewer.takes(file));
}

/** The viewer a person chose for a kind of file if it still takes the file, else the default. */
export function chosenViewer(file: FileFacts, chosen: ViewerId | null, list: readonly Viewer[] = VIEWERS): Viewer {
  const picked = chosen === null ? undefined : list.find((viewer) => viewer.id === chosen && viewer.id !== 'none' && viewer.takes(file));
  return picked ?? defaultViewer(file, list);
}

// ---------------------------------------------------------------- what a person chose for a kind of file

const CHOICE_KEY = (userId: string) => `orch.${userId}.viewers`;

/** The kind of file a choice is kept for: its extension, else its whole name. */
export const kindOf = (name: string): string => {
  const extension = extensionOf(name);
  return extension === '' ? name.toLowerCase() : `.${extension}`;
};

/** What the person chose, by kind of file, on this device. */
export function readChoices(userId: string | null): Record<string, ViewerId> {
  if (userId === null) return {};
  try {
    const raw = localStorage.getItem(CHOICE_KEY(userId));
    const parsed: unknown = raw === null ? {} : JSON.parse(raw);
    return typeof parsed === 'object' && parsed !== null ? (parsed as Record<string, ViewerId>) : {};
  } catch {
    return {};
  }
}

/** The choices of the person signed in, held so that a tab follows a choice made in a menu. */
const useChoices = create<{ userId: string | null; choices: Record<string, ViewerId> }>(() => ({ userId: null, choices: {} }));

/** The viewer the person chose for this kind of file, if they did. */
export function useViewerChoice(userId: string | null, name: string): ViewerId | null {
  const held = useChoices((state) => state);
  if (userId === null) return null;
  const choices = held.userId === userId ? held.choices : readChoices(userId);
  return choices[kindOf(name)] ?? null;
}

/** Remembers the viewer chosen for a kind of file, for this person on this device. */
export function rememberChoice(userId: string | null, name: string, viewer: ViewerId): void {
  if (userId === null) return;
  const choices = { ...readChoices(userId), [kindOf(name)]: viewer };
  useChoices.setState({ userId, choices });
  try {
    localStorage.setItem(CHOICE_KEY(userId), JSON.stringify(choices));
  } catch {
    // A device that keeps nothing keeps no choice; the next file opens in its default viewer.
  }
}
