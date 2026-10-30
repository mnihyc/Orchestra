// @vitest-environment node
/** The rules of the views beside a thread that need no page: which viewer takes a file, which language a file is in, where a path of a tool call is in the workspace, how the tree and the table of processes are laid out, which lines changed, what a reference to the message says, the shell's connection through a line that comes and goes, the live feed's names, the part of a repository's difference that is one file's, and the words for a name the API gives. */
import { describe, expect, it } from 'vitest';

import { partOfDiff } from '@/api/workspace';
import { changedLines } from '@/features/files/AgentEdits';
import { referenceText } from '@/features/files/AddToMessage';
import { workspacePath } from '@/features/files/FileLink';
import { cleanDirectory, copyName, nameFault } from '@/features/files/FileMenu';
import { layoutRows } from '@/features/files/FileTree';
import { LANGUAGES, extensionOf, languageById, languageOf } from '@/features/files/languages';
import { chosenViewer, defaultViewer, factsOf, viewersFor } from '@/features/files/viewers';
import { layOut } from '@/features/inspector/Processes';
import { applyWord, environmentStatusWord, outlineKindWord, refusalWord } from '@/features/inspector/words';
import type { DirEntry, ProcessInfo } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { type ShellSocketLike, connectShell } from '@/realtime/terminal';
import { directoryKey, feedPath, sortEntries, type DirectoryState, type DirectoryWindow } from '@/realtime/stores/environment';

const file = (name: string, more: Partial<Parameters<typeof factsOf>[0]> = {}) => factsOf({ name, media_type: 'text/plain', is_text: true, size: 100, ...more });

describe('the language of a file', () => {
  it('is found by its whole name, its extension and its first line, and a name that no list knows is text', () => {
    expect(languageOf('Dockerfile')?.id).toBe('dockerfile');
    expect(languageOf('web/Makefile')?.id).toBe('shell');
    expect(languageOf('app.ts')?.id).toBe('typescript');
    expect(languageOf('widget.tsx')?.id).toBe('tsx');
    expect(languageOf('types.d.ts')?.id).toBe('typescript');
    expect(languageOf('run', '#!/usr/bin/env python3')?.id).toBe('python');
    expect(languageOf('install', '#!/bin/bash')?.id).toBe('shell');
    expect(languageOf('notes.zzz')).toBeNull();
    expect(languageOf('.gitignore')).toBeNull();
    expect(extensionOf('archive.tar.gz')).toBe('gz');
    expect(languageById('rust')?.name).toBe('Rust');
  });

  it('has every language of the list loadable: a name that is wrong in a module fails here and not when a file is opened', async () => {
    const loaded = await Promise.all(LANGUAGES.map(async (language) => [language.id, await language.load()] as const));
    for (const [id, extension] of loaded) expect(extension, id).toBeDefined();
  });
});

describe('the viewer that opens a file', () => {
  it('is the first of the list that takes it, most particular first', () => {
    expect(defaultViewer(file('README.md')).id).toBe('markdown');
    expect(defaultViewer(file('logo.png', { media_type: 'image/png', is_text: false })).id).toBe('image');
    expect(defaultViewer(file('icon.svg', { media_type: 'image/svg+xml' })).id).toBe('image');
    expect(defaultViewer(file('bundle.zip', { is_text: false })).id).toBe('archive');
    expect(defaultViewer(file('backup.tar.gz', { is_text: false })).id).toBe('archive');
    expect(defaultViewer(file('song.mp3', { media_type: 'audio/mpeg', is_text: false })).id).toBe('media');
    expect(defaultViewer(file('fix.patch')).id).toBe('difference');
    expect(defaultViewer(file('main.rs')).id).toBe('code');
    expect(defaultViewer(file('data.bin', { media_type: 'application/octet-stream', is_text: false })).id).toBe('none');
  });

  it('offers any file as text, and the person\'s own choice comes first where it still takes the file', () => {
    expect(viewersFor(file('data.bin', { is_text: false })).map((viewer) => viewer.id)).toEqual(['code']);
    expect(viewersFor(file('README.md')).map((viewer) => viewer.id)).toEqual(['markdown', 'code']);
    expect(chosenViewer(file('README.md'), 'code').id).toBe('code');
    // A choice that does not take the file any more is not followed.
    expect(chosenViewer(file('main.rs'), 'image').id).toBe('code');
    expect(chosenViewer(file('main.rs'), null).id).toBe('code');
  });
});

describe('a path that a tool call names', () => {
  it('is found in the workspace from the directory the thread works in, or from its root when it is absolute', () => {
    expect(workspacePath('src/app.ts', '/work/web', 'web')).toBe('web/src/app.ts');
    expect(workspacePath('./src/../lib/x.ts', '/work/web', 'web')).toBe('web/lib/x.ts');
    expect(workspacePath('/work/web/src/app.ts', '/work/web', 'web')).toBe('web/src/app.ts');
    expect(workspacePath('/work/README.md', '/work', '')).toBe('README.md');
    expect(workspacePath('/etc/passwd', '/work', '')).toBeNull();
    expect(workspacePath('../../outside', '/work', '')).toBeNull();
    expect(workspacePath('src', '/work', '')).toBe('src');
  });
});

const entry = (path: string, type: DirEntry['type'] = 'file'): DirEntry => ({ cached: false, link_target: null, mode: 420, mtime: '2026-10-01T09:00:00.000Z', name: path.slice(path.lastIndexOf('/') + 1), path, size: 1, type });
const held = (entries: DirEntry[], more: Partial<DirectoryState> = {}): DirectoryState => ({ status: 'ready', entries, source: 'live', asOf: null, truncated: false, offline: false, error: null, ...more });

describe('the rows of the tree', () => {
  it('presents a refused page as capacity and keeps a legacy partial result visibly incomplete', () => {
    const window: DirectoryWindow = { ...held([]), status: 'loading', generation: 1, page: null, navigation: 'idle',
      canNext: false, canPrevious: false, restartRequired: false, expanded: [],
      capacity: { ok: false, reason: 'rows', usage: { rows: 20_000, bytes: 1000, metadata: 1000, records: 10 } } };
    expect(layoutRows('web', new Set(), new Map([[directoryKey('e', 'web'), window]]), 'e').map((row) => row.kind)).toEqual(['capacity']);
    const partial: DirectoryWindow = { ...window, status: 'ready', capacity: null, entries: [entry('web/partial.ts')], truncated: true };
    expect(layoutRows('web', new Set(), new Map([[directoryKey('e', 'web'), partial]]), 'e').map((row) => row.kind)).toEqual(['entry', 'more']);
  });
  it('preserves a failed watch cause above retained rows even when its cursors were invalidated', () => {
    const state = held([entry('web/kept.ts')], { status: 'failed', invalidated: true, watch: 'failed',
      watchError: { code: 'environment.unavailable', reason: 'busy', detail: 'The watch could not start.', retryable: true } });
    const rows = layoutRows('web', new Set(), new Map([[directoryKey('e', 'web'), state]]), 'e');
    expect(rows[0]).toMatchObject({ kind: 'failed', reason: 'The watch could not start.', retryable: true });
    expect(rows[1]?.entry?.path).toBe('web/kept.ts');
  });
  it('are laid out from what is loaded: an open directory brings its children, one that is not read says it is being read, one that is cut says so', () => {
    const dirs = new Map<string, DirectoryState>([
      [directoryKey('e', 'web'), held([entry('web/src', 'dir'), entry('web/a.ts'), entry('web/b.ts')], { truncated: true })],
      [directoryKey('e', 'web/src'), held([entry('web/src/app.ts')])],
    ]);
    const closed = layoutRows('web', new Set(), dirs, 'e');
    expect(closed.map((row) => `${row.depth}:${row.kind}:${row.entry?.name ?? ''}`)).toEqual(['0:entry:src', '0:entry:a.ts', '0:entry:b.ts', '0:more:']);
    const open = layoutRows('web', new Set(['web/src']), dirs, 'e');
    expect(open.map((row) => row.entry?.name ?? row.kind)).toEqual(['src', 'app.ts', 'a.ts', 'b.ts', 'more']);
    expect(open[1]).toMatchObject({ depth: 1, position: 1, setSize: 1, parent: 'web/src' });
    const unread = layoutRows('web', new Set(['web/src']), new Map([[directoryKey('e', 'web'), dirs.get(directoryKey('e', 'web'))!]]), 'e');
    expect(unread.map((row) => row.kind)).toContain('loading');
    expect(layoutRows('', new Set(), new Map(), 'e').map((row) => row.kind)).toEqual(['loading']);
    expect(layoutRows('', new Set(), new Map([[directoryKey('e', ''), held([])]]), 'e').map((row) => row.kind)).toEqual(['empty']);
  });

  it('lay out twenty thousand entries without a cost that grows with how many are in view', () => {
    const entries = Array.from({ length: 20_000 }, (_, index) => entry(`many/file-${String(index).padStart(5, '0')}.txt`));
    const dirs = new Map([[directoryKey('e', 'many'), held(entries)]]);
    const began = performance.now();
    const rows = layoutRows('many', new Set(), dirs, 'e');
    expect(rows).toHaveLength(20_000);
    expect(performance.now() - began).toBeLessThan(500);
  });

  it('keep directories first and then order by name without regard to case, as the API does', () => {
    const sorted = sortEntries([entry('b.ts'), entry('Z', 'dir'), entry('a.ts'), entry('c', 'dir')]);
    expect(sorted.map((one) => one.name)).toEqual(['c', 'Z', 'a.ts', 'b.ts']);
  });

  it('name a directory on the live feed by the bytes of its path in base64 that is safe in an address', () => {
    expect(feedPath('')).toBe('');
    expect(feedPath('web/src')).toBe('d2ViL3NyYw');
    expect(feedPath('日本')).toBe('5pel5pys');
  });
});

const process = (pid: number, ppid: number, thread: string | null): ProcessInfo => ({ command: `p${pid}`, cpu_percent: 0, item_id: null, memory_bytes: 1, name: `p${pid}`, pid, ppid, started_at: '2026-10-01T09:00:00.000Z', state: 'S', thread_id: thread, user: 'agent' });

describe('the table of processes', () => {
  it('is a forest with a process under the one that started it, and this thread\'s own trees first', () => {
    const rows = layOut([process(1, 0, null), process(10, 1, null), process(200, 1, 'mine'), process(201, 200, 'mine'), process(7, 0, null)], 'mine');
    // 1 and 7 are roots, and 1 owns a process of this thread through 200.
    expect(rows.map((row) => `${row.depth}:${row.process.pid}`)).toEqual(['0:1', '1:10', '1:200', '2:201', '0:7']);
    const none = layOut([process(5, 0, null), process(3, 0, null)], 'mine');
    expect(none.map((row) => row.process.pid)).toEqual([3, 5]);
  });

  it('puts a process that is its own parent, or whose parent is gone, at the top', () => {
    const rows = layOut([process(4, 4, null), process(9, 99, null)], 'x');
    expect(rows.map((row) => row.depth)).toEqual([0, 0]);
  });
});

describe('what changed in a file', () => {
  it('is the lines between what the two texts share at their ends, and a place where lines were only taken away', () => {
    expect(changedLines('a\nb\nc\n', 'a\nb\nc\n')).toEqual([]);
    expect(changedLines('a\nb\nc', 'a\nX\nc')).toEqual([2]);
    expect(changedLines('a\nb\nc', 'a\nb\nc\nd\ne')).toEqual([4, 5]);
    expect(changedLines('a\nb\nc', 'a\nc')).toEqual([2]);
    expect(changedLines('', 'new\ntext')).toEqual([1, 2]);
    expect(changedLines('x', 'y')).toEqual([1]);
  });

  it('marks no more than a few hundred lines: a file written afresh is not lit from end to end', () => {
    const many = Array.from({ length: 5_000 }, (_, index) => `line ${index}`).join('\n');
    expect(changedLines('', many).length).toBeLessThanOrEqual(400);
  });
});

describe('what goes into the message', () => {
  it('is the path alone, or the path with its lines and a fenced quotation that nothing inside can end', () => {
    expect(referenceText({ kind: 'file', path: 'web/a.ts' })).toBe('`web/a.ts`');
    expect(referenceText({ kind: 'lines', path: 'web/a.ts', fromLine: 3, toLine: 3, text: 'x' })).toBe('`web/a.ts:3`\n```\nx\n```');
    expect(referenceText({ kind: 'lines', path: 'a', fromLine: 2, toLine: 5, text: 'a ``` b' })).toBe('`a:2-5`\n````\na ``` b\n````');
    expect(referenceText({ kind: 'output', command: 'ls', text: 'one' })).toBe('`$ ls`\n```\none\n```');
    expect(referenceText({ kind: 'difference', path: 'a', text: '+x' })).toContain('```diff');
  });
});

describe('names typed for files', () => {
  it('are refused when they are empty or a path of their own, and a destination is cleaned of what is no part of a path', () => {
    expect(nameFault('')).toBe('empty');
    expect(nameFault('  ')).toBe('empty');
    expect(nameFault('a/b')).toBe('slash');
    expect(nameFault('..')).toBe('dots');
    expect(nameFault('notes.txt')).toBeNull();
    expect(cleanDirectory(' /web//src/ ')).toBe('web/src');
    expect(cleanDirectory('./')).toBe('');
    expect(copyName('notes.md')).toBe('notes copy.md');
    expect(copyName('Makefile')).toBe('Makefile copy');
  });
});

/** A socket that says what a test tells it to, and keeps what was sent. */
function fakeSocket() {
  const sent: (string | Uint8Array)[] = [];
  const socket: ShellSocketLike = { readyState: 1, binaryType: 'blob', onopen: null, onmessage: null, onclose: null, onerror: null, send: (data) => void sent.push(data), close: () => undefined };
  return { socket, sent, text: (frame: object) => socket.onmessage?.({ data: JSON.stringify(frame) }), bytes: (data: Uint8Array) => socket.onmessage?.({ data: data.buffer }) };
}

/** Where a test's socket would be: the page's own address is not there in a test without a page. */
const where = (_id: string, ticket: string) => `ws://x/${ticket}`;

describe('the connection of a shell', () => {
  it('is attached when the shell says it is ready, draws what it prints, and says why it ended', () => {
    const made = fakeSocket();
    const drawn: string[] = [];
    const readies: number[] = [];
    const connection = connectShell({
      terminalId: 't1',
      firstTicket: 'ticket-1',
      connect: () => made.socket,
      address: where,
      listener: { onReady: (size) => readies.push(size.replayBytes), onData: (bytes) => drawn.push(new TextDecoder().decode(bytes)) },
    });
    return Promise.resolve().then(() => {
      made.socket.onopen?.();
      expect(connection.state.getState().phase).toBe('connecting');
      made.text({ op: 'ready', cols: 80, rows: 24, replay_bytes: 5 });
      expect(connection.state.getState().phase).toBe('attached');
      made.bytes(new TextEncoder().encode('hello'));
      expect(drawn).toEqual(['hello']);
      expect(readies).toEqual([5]);
      connection.resize(100, 30);
      expect(made.sent.at(-1)).toBe(JSON.stringify({ op: 'resize', cols: 100, rows: 30 }));
      made.text({ op: 'exit', code: 3 });
      made.text({ op: 'closed', reason: 'exited' });
      expect(connection.state.getState()).toMatchObject({ phase: 'closed', reason: 'exited', exitCode: 3 });
    });
  });

  it('is told that another screen took the shell, and does not try to take it back by itself', () => {
    const made = fakeSocket();
    const connection = connectShell({ terminalId: 't1', firstTicket: 'x', connect: () => made.socket, address: where, listener: { onReady: () => undefined, onData: () => undefined } });
    return Promise.resolve().then(() => {
      made.text({ op: 'ready', cols: 80, rows: 24, replay_bytes: 0 });
      made.text({ op: 'closed', reason: 'attached_elsewhere' });
      expect(connection.state.getState()).toMatchObject({ phase: 'elsewhere', reason: 'attached_elsewhere' });
    });
  });

  it('says that it lost the line, keeps the screen, and asks for a new ticket to attach again', async () => {
    const sockets = [fakeSocket(), fakeSocket()];
    const urls: string[] = [];
    let asked = 0;
    const connection = connectShell({
      terminalId: 't1',
      firstTicket: 'first',
      connect: (url) => {
        urls.push(url);
        return (sockets[urls.length - 1] ?? sockets[1]!).socket;
      },
      address: where,
      tickets: () => {
        asked += 1;
        return Promise.resolve('second');
      },
      listener: { onReady: () => undefined, onData: () => undefined },
    });
    await Promise.resolve();
    sockets[0]!.text({ op: 'ready', cols: 80, rows: 24, replay_bytes: 0 });
    sockets[0]!.socket.onclose?.({ code: 1006 });
    expect(connection.state.getState().phase).toBe('lost');
    await new Promise((resolve) => setTimeout(resolve, 700));
    expect(asked).toBe(1);
    expect(urls).toEqual(['ws://x/first', 'ws://x/second']);
    connection.detach();
  });

  it('measures the round trip as the middle of the last three, so that one late answer changes nothing', async () => {
    const made = fakeSocket();
    const connection = connectShell({ terminalId: 't1', firstTicket: 'x', connect: () => made.socket, address: where, listener: { onReady: () => undefined, onData: () => undefined } });
    await Promise.resolve();
    made.socket.onopen?.();
    // The first ping goes at once; answering it right away measures a few milliseconds at most.
    const first = JSON.parse(String(made.sent[0])) as { n: number };
    made.text({ op: 'pong', n: first.n });
    expect(connection.state.getState().roundTripMs).toBeGreaterThanOrEqual(0);
    expect(connection.state.getState().roundTripMs).toBeLessThan(1_000);
    connection.detach();
  });
});

describe('the words for what the API names', () => {
  it.each(['en', 'zh-CN'] as const)('uses current catalogue messages and a readable stand-in for an unknown API name in %s', async (locale) => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    try {
      expect(refusalWord(t, 'excluded_by_policy')).toBe(t('start.reason.excluded_by_policy'));
      for (const mode of ['next_turn', 'new_session', 'recreate_environment'] as const) {
        const word = applyWord(t, mode);
        expect(word).toBe(t(`composer.model.applies.${mode}`));
        expect(word).not.toMatch(/^composer\./u);
      }
      for (const state of ['none', 'provisioning', 'running', 'stopped', 'recreating', 'moving', 'failed', 'offline', 'disabled', 'removed'] as const) {
        const word = environmentStatusWord(t, state);
        expect(word).toBe(t(`environments.status.${state}`));
        expect(word).not.toMatch(/^environments\./u);
      }
      expect(refusalWord(t, 'a_reason_added_later')).toBe(refusalWord(t, 'disabled'));
      expect(refusalWord(t, null)).toBe(refusalWord(t, 'disabled'));
      expect(refusalWord(t, 'excluded_by_policy')).not.toMatch(/^(?:inspector|start)\./u);
      expect(applyWord(t, 'on_a_tuesday')).toBe(applyWord(t, 'next_turn'));
      expect(outlineKindWord(t, 'a_kind_added_later')).toBe(outlineKindWord(t, 'marker'));
      expect(environmentStatusWord(t, 'sleeping')).toBe(environmentStatusWord(t, 'none'));
    } finally { await setLocale('en'); }
  });
});

describe('the difference of a repository', () => {
  const whole = [
    'diff --git a/web/a.ts b/web/a.ts',
    'index 111..222 100644',
    '--- a/web/a.ts',
    '+++ b/web/a.ts',
    '@@ -1 +1 @@',
    '-one',
    '+two',
    'diff --git a/web/old name.ts b/web/new name.ts',
    'similarity index 90%',
    'rename from web/old name.ts',
    'rename to web/new name.ts',
    '@@ -1 +1 @@',
    '-x',
    '+y',
    'diff --git a/web/b.ts b/web/b.ts',
    'new file mode 100644',
    '--- /dev/null',
    '+++ b/web/b.ts',
    '@@ -0,0 +1 @@',
    '+b',
    '',
  ].join('\n');

  it('is handed over whole by the server, and the part of one file is cut out by its path, a renamed file being found under either name', () => {
    expect(partOfDiff(whole, 'web/a.ts')).toBe(whole.split('diff --git a/web/old name.ts')[0]);
    expect(partOfDiff(whole, 'web/b.ts')).toContain('+++ b/web/b.ts');
    expect(partOfDiff(whole, 'web/b.ts')).not.toContain('web/a.ts');
    expect(partOfDiff(whole, 'web/new name.ts')).toContain('rename to web/new name.ts');
    expect(partOfDiff(whole, 'web/old name.ts')).toContain('rename from web/old name.ts');
    expect(partOfDiff(whole, 'web/missing.ts')).toBe('');
    // A path that only ends like another is not that file.
    expect(partOfDiff(whole, 'a.ts')).toBe('');
  });
});
