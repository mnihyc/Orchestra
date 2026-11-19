/** What the fake API says of an environment's workspace: its files, what changed in them, Git, the processes and the load of the environment, the shells of a person, and the feeds that keep a view current.
 *
 * The files live in memory, per environment, with a hash that names each content. The requests keep to
 * what the real API promises of them as far as a screen can tell: a write names the version it read and
 * is refused with the file as it stands when that is not the one there; a listing says whether it was read
 * now or from the last list; an environment that is away answers from what was opened before. A test steers
 * the fake through `/__double/workspace/*`: put files in, change them as an agent would, take the
 * environment away, give the processes and the changes, and fill a directory with twenty thousand files.
 */
import { createHash } from 'node:crypto';
import type { IncomingMessage } from 'node:http';

import type {
  DirEntry,
  DirData,
  DirListing,
  Environment,
  EnvironmentChange,
  FileStat,
  GitStatus,
  OutlineEntry,
  ThreadOutlinePage,
  Item,
  Turn,
  ProcessInfo,
  ResourceSample,
  UserRef,
} from '../../src/generated/api-types.ts';
import { ACME, id as fixtureId, RUNS_ON, userRef } from './fixtures.ts';
import { Terminals } from './terminal-socket.ts';
import { type Answer, type Part, type Person, Refusal, type Subscriber, type World } from './world.ts';

/** One file or directory of an environment. */
interface Node {
  type: 'file' | 'dir';
  content: Buffer;
  /** Milliseconds since the epoch. */
  mtime: number;
  mode: number;
}

/** What a workspace takes of its environment: the state and the switches are the record's (the resources double keeps them), and the measured limits are the workspace's. */
interface Facts {
  status: Environment['status'];
  index: 'full' | 'live';
  remote_write: boolean;
  terminal: boolean;
  /** The measured load limits the charts draw as lines. */
  limits: { cpu_millis: number | null; memory_bytes: number | null; disk_bytes: number | null };
}

const LIMITS: Facts['limits'] = { cpu_millis: 4_000, memory_bytes: 8 * 1024 ** 3, disk_bytes: 100 * 1024 ** 3 };

/** How many bytes of a file are read for a view before it says it is cut: the API's own limit. */
const PREVIEW_BYTES = 2 * 1024 * 1024;

/** The environment the main thread of the team works in. */
const MAIN = RUNS_ON.workspace;

const TINY_PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg==', 'base64');

const sha = (content: Buffer): string => createHash('sha256').update(content).digest('hex');
const isText = (content: Buffer): boolean => !content.subarray(0, 4_096).includes(0);

/** A repository that could not be read, as the server says it: why, and nothing else. */
const unreadGit = (reason: NonNullable<GitStatus['reason']>): GitStatus => ({
  available: false,
  reason,
  repository_dir: null,
  status_hash: null,
  remotes: [],
  upstream: null,
  branch: null,
  head: null,
  ahead: null,
  behind: null,
  changes: [],
});

const APP_TS = `import { greet } from './util';

/** The entry point of the page. */
export function main(name: string): string {
  const message = greet(name);
  console.log(message);
  return message;
}

main('world');
`;

const UTIL_TS = `export function greet(name: string): string {
  return \`Hello, \${name}!\`;
}

export const VERSION = 3;
`;

export function mimeOf(path: string, content: Buffer): string {
  const extension = path.slice(path.lastIndexOf('.') + 1).toLowerCase();
  const known: Record<string, string> = {
    png: 'image/png',
    svg: 'image/svg+xml',
    json: 'application/json',
    md: 'text/markdown',
    ts: 'text/typescript',
    css: 'text/css',
    html: 'text/html',
    zip: 'application/zip',
    pdf: 'application/pdf',
    mp3: 'audio/mpeg',
  };
  return known[extension] ?? (isText(content) ? 'text/plain' : 'application/octet-stream');
}

/** Everything the fake keeps of the workspaces. */
export class WorkspaceWorld implements Part {
  readonly world: World;
  private files = new Map<string, Map<string, Node>>();
  private cached = new Map<string, Set<string>>();
  /** The measured limits a test set for an environment. */
  private limits = new Map<string, Facts['limits']>();
  /** The changes the threads' tool calls recorded, per environment, newest last. */
  changes = new Map<string, EnvironmentChange[]>();
  git = new Map<string, GitStatus>();
  /** What `git diff` writes for each path that has changed in the working tree, and (staged) for each that is in the index. */
  gitDiffs = new Map<string, string>();
  gitStagedDiffs = new Map<string, string>();
  processes: ProcessInfo[] = [];
  /** Whether the live feeds are served; the real server serves none of the ephemeral ones yet. */
  feeds = true;
  /** Archives by path: what `…/files/archive` lists. */
  archives = new Map<string, { format: 'zip' | 'tar'; compression: 'gzip' | null; encrypted: boolean; entries: Map<string, Buffer | null> }>();
  /** Files whose first fetch answers `fetching` once, to look at the wait. */
  slow = new Set<string>();
  /** Every request a test may want to count, by kind. */
  seen: string[] = [];
  readonly terminals: Terminals;
  private watchers: { feed: string; id: number; subscriber: Subscriber; user: string }[] = [];
  private nextPid = 400;
  private readonly directoryWindows = new Map<string, { environment: string; listing: DirListing }>();

  constructor(world: World) {
    this.world = world;
    this.terminals = new Terminals(this);
    this.seed();
  }

  // ---------------------------------------------------------------- state

  /** The fake world as it begins: a small project in Main, a few things running, a change and a repository. */
  seed(): void {
    this.files.clear();
    this.cached.clear();
    this.limits.clear();
    this.changes.clear();
    this.git.clear();
    this.gitDiffs.clear();
    this.gitStagedDiffs.clear();
    this.archives.clear();
    this.slow.clear();
    this.seen = [];
    this.watchers = [];
    this.directoryWindows.clear();
    this.feeds = true;
    this.terminals.reset();
    const now = Date.parse('2026-10-01T09:20:00.000Z');
    this.put(MAIN, 'README.md', '# Main\n\nThe workspace of the fake API.\n', now);
    this.put(MAIN, 'package.json', '{\n  "name": "web",\n  "version": "1.0.0"\n}\n', now);
    this.put(MAIN, 'web/README.md', '# Web\n\nThe page.\n\n- one\n- two\n', now);
    this.put(MAIN, 'web/index.html', '<!doctype html>\n<title>Web</title>\n<h1>Hello</h1>\n', now);
    this.put(MAIN, 'web/src/app.ts', APP_TS, now);
    this.put(MAIN, 'web/src/util.ts', UTIL_TS, now);
    this.put(MAIN, 'web/src/style.css', 'body {\n  margin: 0;\n}\n', now);
    this.put(MAIN, 'web/logo.png', TINY_PNG, now);
    this.put(MAIN, 'web/notes.txt', 'first\nsecond\nthird\n', now);
    this.put(MAIN, 'web/changes.patch', '@@ -1,2 +1,2 @@\n-old line\n+new line\n context\n', now);
    this.put(MAIN, 'docs/guide.md', '# Guide\n\nRead the code.\n', now);
    this.put(MAIN, 'data/blob.bin', Buffer.from([0, 1, 2, 3, 255, 0, 7]), now);
    this.put(MAIN, 'data/report.pdf', Buffer.from('%PDF-1.4\n%fake\n'), now);
    this.put(MAIN, 'data/bundle.zip', Buffer.from('PK fake archive'), now);
    this.archives.set(`${MAIN}\u0000data/bundle.zip`, {
      format: 'zip',
      compression: null,
      encrypted: false,
      entries: new Map<string, Buffer | null>([
        ['bundle/', null],
        ['bundle/readme.txt', Buffer.from('inside the archive\n')],
        ['bundle/pixel.png', TINY_PNG],
        ['bundle/big.bin', Buffer.alloc(PREVIEW_BYTES + 10)],
      ]),
    });
    this.cached.set(MAIN, new Set(['README.md', 'web/src/app.ts']));

    const ada = this.world.people.get('ada');
    const thread = fixtureId(3, 4);
    this.changes.set(MAIN, [
      {
        added: 2,
        at: '2026-10-01T09:10:00.000Z',
        binary: false,
        change: 'modify',
        diff: '@@ -1,4 +1,5 @@\n import { greet } from \'./util\';\n \n-export function main() {\n+export function main(name: string): string {\n+  const message = greet(name);\n   return 1;\n }\n',
        item_id: fixtureId(5, 1),
        old_path: null,
        path: 'web/src/app.ts',
        removed: 1,
        thread_id: thread,
        truncated: false,
      },
      {
        added: 1,
        at: '2026-10-01T09:15:00.000Z',
        binary: false,
        change: 'add',
        diff: '@@ -0,0 +1 @@\n+export const VERSION = 3;\n',
        item_id: fixtureId(5, 2),
        old_path: null,
        path: 'web/src/util.ts',
        removed: 0,
        thread_id: thread,
        truncated: false,
      },
    ]);
    this.git.set(MAIN, {
      available: true,
      reason: null,
      // The repository is the workspace's root, with one remote whose address carries no credentials.
      repository_dir: '',
      status_hash: '7c1e4f20a9b3d5c6e8f0a1b2c3d4e5f60718293a4b5c6d7e8f9a0b1c2d3e4f5a',
      remotes: [{ name: 'origin', url: 'https://git.example/acme/web.git' }],
      upstream: 'origin/main',
      branch: 'main',
      head: 'a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0',
      ahead: 1,
      behind: 0,
      changes: [
        { path: 'web/src/app.ts', index: ' ', worktree: 'M' },
        { path: 'web/src/util.ts', index: 'A', worktree: ' ' },
        { path: 'notes.md', index: '?', worktree: '?' },
      ],
    });
    this.gitDiffs.set(
      'web/src/app.ts',
      "diff --git a/web/src/app.ts b/web/src/app.ts\nindex 3b18e51..a4c2f09 100644\n--- a/web/src/app.ts\n+++ b/web/src/app.ts\n@@ -1,3 +1,4 @@\n import { greet } from './util';\n-main();\n+main('world');\n+console.log(1);\n",
    );
    this.gitStagedDiffs.set(
      'web/src/util.ts',
      'diff --git a/web/src/util.ts b/web/src/util.ts\nnew file mode 100644\nindex 0000000..9c1d2e3\n--- /dev/null\n+++ b/web/src/util.ts\n@@ -0,0 +1,3 @@\n+export function greet(name: string): string {\n+  return `hello ${name}`;\n+}\n',
    );
    this.processes = [
      this.process(100, 1, 'bash', '/bin/bash', 0.1, 6 * 1024 ** 2, null, null, ada === undefined ? null : ada.me.username),
      this.process(101, 100, 'node', 'node server.js --port 3000', 12.4, 120 * 1024 ** 2, thread, fixtureId(5, 3), 'agent'),
      this.process(102, 101, 'esbuild', 'esbuild --watch', 3.2, 40 * 1024 ** 2, thread, fixtureId(5, 3), 'agent'),
      this.process(200, 1, 'sleep', 'sleep 600', 0, 1 * 1024 ** 2, null, null, 'ada'),
    ];
  }

  private process(pid: number, ppid: number, name: string, command: string, cpu: number, memory: number, thread: string | null, item: string | null, user: string | null): ProcessInfo {
    return {
      command,
      cpu_percent: cpu,
      item_id: item,
      memory_bytes: memory,
      name,
      pid,
      ppid,
      started_at: new Date(Date.parse('2026-10-01T09:00:00.000Z') + pid * 1_000).toISOString(),
      state: 'S',
      thread_id: thread,
      user: user ?? 'agent',
    };
  }

  private tree(environment: string): Map<string, Node> {
    let tree = this.files.get(environment);
    if (tree === undefined) {
      tree = new Map();
      this.files.set(environment, tree);
    }
    return tree;
  }

  /** A snapshot copies current bytes, so later edits or removal cannot alter its download. */
  copyFiles(environment: string): [string, Buffer][] {
    return [...this.tree(environment)].filter(([, node]) => node.type === 'file').map(([path, node]): [string, Buffer] => [path, Buffer.from(node.content)]);
  }

  /** Writes a file, making the directories above it. */
  put(environment: string, path: string, content: string | Buffer, at: number = Date.now()): void {
    const tree = this.tree(environment);
    const bytes = typeof content === 'string' ? Buffer.from(content, 'utf8') : content;
    const parts = path.split('/');
    for (let index = 1; index < parts.length; index += 1) {
      const directory = parts.slice(0, index).join('/');
      if (!tree.has(directory)) tree.set(directory, { type: 'dir', content: Buffer.alloc(0), mtime: at, mode: 0o755 });
    }
    tree.set(path, { type: 'file', content: bytes, mtime: at, mode: 0o644 });
  }

  /** What the environment is now, as the record the resources double keeps says it, with the limits a test set. */
  override(environment: string): Facts {
    const facts = this.world.resources.workspaceFacts(environment);
    return {
      status: facts?.status ?? 'running',
      index: facts?.index ?? 'full',
      remote_write: facts?.remoteWrite ?? true,
      terminal: facts?.terminal ?? true,
      limits: this.limits.get(environment) ?? LIMITS,
    };
  }

  private away(environment: string): boolean {
    return this.override(environment).status !== 'running';
  }

  // ---------------------------------------------------------------- what is described

  private entryOf(environment: string, path: string, node: Node): DirEntry {
    return {
      cached: node.type === 'file' && (this.cached.get(environment)?.has(path) ?? false),
      link_target: null,
      mode: node.mode,
      mtime: new Date(node.mtime).toISOString(),
      name: path.slice(path.lastIndexOf('/') + 1),
      path,
      size: node.type === 'dir' ? 0 : node.content.length,
      type: node.type,
    };
  }

  private statOf(environment: string, path: string, node: Node): FileStat {
    return {
      ...this.entryOf(environment, path, node),
      hash: node.type === 'file' ? sha(node.content) : null,
      is_text: node.type === 'file' && isText(node.content),
      media_type: node.type === 'dir' ? 'inode/directory' : mimeOf(path, node.content),
    };
  }

  private children(environment: string, directory: string): DirEntry[] {
    const prefix = directory === '' ? '' : `${directory}/`;
    const found: DirEntry[] = [];
    for (const [path, node] of this.tree(environment)) {
      if (!path.startsWith(prefix) || path === directory) continue;
      const rest = path.slice(prefix.length);
      if (rest.includes('/')) continue;
      found.push(this.entryOf(environment, path, node));
    }
    return found.sort((one, other) => {
      if ((one.type === 'dir') !== (other.type === 'dir')) return one.type === 'dir' ? -1 : 1;
      return one.name.toLowerCase().localeCompare(other.name.toLowerCase());
    });
  }

  // ---------------------------------------------------------------- feeds

  /** Tells whoever watches a directory that something in it changed. */
  private touched(environment: string, directory: string, removed: string[] = []): void {
    if (!this.feeds) return;
    const listing = this.directoryPage(environment, directory, 'live');
    for (const watcher of this.watchers) {
      if (!watcher.feed.endsWith(`:dir:${this.feedPath(directory)}`)) continue;
      if (!watcher.feed.startsWith(`env:${environment}:dir:`)) continue;
      watcher.subscriber.send({ op: 'event', id: watcher.id, feed: watcher.feed, data: { ...listing, reset: true, changed: listing.entries, entries: undefined, removed } });
    }
  }

  private feedPath(path: string): string {
    return Buffer.from(path, 'utf8').toString('base64url');
  }

  /** The frames of the live connection about the feeds of a directory, of the processes and of a measurement: whether the frame was about one of these. */
  frame(subscriber: Subscriber, person: Person | undefined, frame: Record<string, unknown>): boolean {
    const id = Number(frame.id);
    if (frame.op === 'unsub') {
      const watcher = this.watchers.find((candidate) => candidate.subscriber === subscriber && candidate.id === id);
      if (watcher === undefined) return false;
      this.watchers = this.watchers.filter((candidate) => candidate !== watcher);
      subscriber.send({ op: 'closed', id, feed: watcher.feed, reason: 'unsubscribed' });
      return true;
    }
    if (frame.op !== 'sub' || person === undefined) return false;
    const feed = typeof frame.feed === 'string' ? frame.feed : '';
    if (/^thread:[^:]+:(dir:|procs$)/.test(feed)) {
      subscriber.send({ op: 'error', id, code: 'bad_feed', detail: 'This workspace feed needs an environment.' });
      return true;
    }
    if (!/^env:[^:]+:(stats$|dir:|procs$)/.test(feed)) return false;
    if (!this.feeds) {
      subscriber.send({ op: 'error', id, code: 'bad_feed', detail: feed });
      return true;
    }
    this.watchers.push({ feed, id, subscriber, user: person.me.username });
    subscriber.send({ op: 'subscribed', id, feed, seq: null, mode: 'ephemeral' });
    const directory = /^env:([^:]+):dir:(.*)$/.exec(feed);
    if (directory !== null) {
      const environment = directory[1] ?? '';
      const path = Buffer.from(directory[2] ?? '', 'base64url').toString('utf8');
      const listing = this.away(environment) ? null : this.directoryPage(environment, path, 'live');
      subscriber.send({ op: 'event', id, feed, data: listing === null ? { offline: true } : { ...listing, reset: true, changed: listing.entries, entries: undefined, removed: [] } });
    }
    if (feed.endsWith(':procs')) subscriber.send({ op: 'event', id, feed, data: { as_of: new Date().toISOString(), processes: this.processes } });
    return true;
  }

  /** A live connection ended: nothing it watched is told any more. */
  gone(subscriber: Subscriber): void {
    this.watchers = this.watchers.filter((watcher) => watcher.subscriber !== subscriber);
  }

  // ---------------------------------------------------------------- the requests

  /** The environment is the resources double's; a request that names one it does not hold is refused as the real API refuses it. */
  private environmentOf(id: string): void {
    if (!this.world.resources.hasEnvironment(id)) throw new Refusal(404, 'not_found', 'there is no such environment');
  }

  /** The real server reads the one repository at the root of the workspace: a path that is not the root is refused. */
  private repositoryAtTheRoot(query: URLSearchParams): void {
    const path = (query.get('path') ?? '').replace(/^\/+|\/+$/g, '');
    if (path !== '') throw new Refusal(400, 'request.invalid', 'path: only the repository at the root of the workspace is read', { errors: [{ path: 'path', message: 'only the repository at the root of the workspace is read' }] });
  }

  /** Lets a person in through the thread they name, or as a system administrator, as the real API does. */
  private door(person: Person, environment: string, query: URLSearchParams, need: 'files.view' | 'files.manage' | 'terminal.view' | 'terminal.manage'): string | null {
    this.environmentOf(environment);
    const thread = query.get('thread');
    // The doors are the environment's: what the caller holds on it, whichever thread they work beside.
    if (!this.world.access.permissionsOnEnvironment(person, environment).includes(need)) {
      throw new Refusal(403, 'authz.denied', `${need} is required`, { required: need, requestable: true, override: false, missing: [] });
    }
    return thread;
  }

  private cleanPath(raw: string | null): string {
    const path = (raw ?? '').replace(/^\/+|\/+$/g, '');
    if (path.split('/').includes('..')) throw new Refusal(404, 'not_found', 'there is no such thing, or it was deleted');
    return path;
  }

  private writable(environment: string): void {
    const over = this.override(environment);
    if (!over.remote_write) throw new Refusal(403, 'workspace.remote_write_disabled', 'files are not written into this environment from here');
    if (this.away(environment)) throw new Refusal(503, 'source.offline', 'the environment is away');
  }

  /** Every request of the workspace reads its own body (a file's bytes, a range), so they are all asked before the others. */
  async raw(person: Person, request: IncomingMessage, method: string, path: string, query: URLSearchParams): Promise<Answer | null> {
    const route = `${method} ${path}`;
    let found: RegExpExecArray | null;

    if ((found = /^GET \/environments\/([^/]+)\/files$/.exec(route))) return this.list(person, found[1] ?? '', query);
    if ((found = /^GET \/environments\/([^/]+)\/files\/stat$/.exec(route))) return this.stat(person, found[1] ?? '', query);
    if ((found = /^POST \/environments\/([^/]+)\/files\/fetch$/.exec(route))) return this.fetch(person, found[1] ?? '', query, await jsonOf(request));
    if ((found = /^GET \/environments\/([^/]+)\/files\/content$/.exec(route))) return this.content(person, found[1] ?? '', query, request);
    if ((found = /^GET \/environments\/([^/]+)\/files\/preview$/.exec(route))) return this.preview(person, found[1] ?? '', query);
    if ((found = /^PUT \/environments\/([^/]+)\/files\/content$/.exec(route))) return this.write(person, found[1] ?? '', query, await rawOf(request));
    if ((found = /^POST \/environments\/([^/]+)\/files\/mkdir$/.exec(route))) return this.mkdir(person, found[1] ?? '', query, await jsonOf(request));
    if ((found = /^POST \/environments\/([^/]+)\/files\/(rename|copy)$/.exec(route))) {
      return this.moveOrCopy(person, found[1] ?? '', query, await jsonOf(request), found[2] === 'copy');
    }
    if ((found = /^DELETE \/environments\/([^/]+)\/files$/.exec(route))) return this.remove(person, found[1] ?? '', query);
    if ((found = /^GET \/environments\/([^/]+)\/files\/search$/.exec(route))) return this.search(person, found[1] ?? '', query);
    if ((found = /^GET \/environments\/([^/]+)\/files\/archive$/.exec(route))) return this.archive(person, found[1] ?? '', query);
    if ((found = /^GET \/environments\/([^/]+)\/files\/archive\/entry$/.exec(route))) return this.archiveEntry(person, found[1] ?? '', query);
    if ((found = /^GET \/environments\/([^/]+)\/changes$/.exec(route))) {
      const environment = found[1] ?? '';
      const thread = this.door(person, environment, query, 'files.view');
      const all = query.get('all') === 'true' || thread === null;
      const list = (this.changes.get(environment) ?? []).filter((change) => all || change.thread_id === thread);
      return { status: 200, body: { changes: list } };
    }
    if ((found = /^GET \/environments\/([^/]+)\/git\/status$/.exec(route))) {
      const environment = found[1] ?? '';
      this.door(person, environment, query, 'files.view');
      this.repositoryAtTheRoot(query);
      if (this.away(environment)) return { status: 200, body: unreadGit('offline') };
      return { status: 200, body: this.git.get(environment) ?? unreadGit('not_a_repository') };
    }
    if ((found = /^GET \/environments\/([^/]+)\/git\/diff$/.exec(route))) {
      this.door(person, found[1] ?? '', query, 'files.view');
      this.repositoryAtTheRoot(query);
      // The difference of the whole repository, as the real server hands it over: the one at the root, written by Git.
      const text = [...(query.get('staged') === 'true' ? this.gitStagedDiffs : this.gitDiffs).values()].join('');
      return { status: 200, text, headers: { 'X-Truncated': '0' } };
    }
    if ((found = /^GET \/environments\/([^/]+)\/processes$/.exec(route))) {
      this.door(person, found[1] ?? '', query, 'terminal.view');
      return { status: 200, body: { as_of: new Date().toISOString(), processes: this.processes } };
    }
    if ((found = /^POST \/environments\/([^/]+)\/processes\/([^/]+)\/signal$/.exec(route))) {
      this.door(person, found[1] ?? '', query, 'terminal.manage');
      const body = await jsonOf(request);
      const pid = Number(found[2]);
      const target = this.processes.find((candidate) => candidate.pid === pid);
      if (target === undefined || target.started_at !== body.started_at) throw new Refusal(404, 'not_found', 'there is no such process');
      this.seen.push(`signal ${pid} ${String(body.signal)}`);
      if (body.signal !== 'INT') this.processes = this.processes.filter((candidate) => candidate.pid !== pid);
      this.tellProcesses();
      return { status: 204 };
    }
    if ((found = /^GET \/environments\/([^/]+)\/resources$/.exec(route))) {
      this.door(person, found[1] ?? '', query, 'terminal.view');
      return { status: 200, body: this.samples(found[1] ?? '', Number(query.get('minutes') ?? 60)) };
    }

    if ((found = /^GET \/environments\/([^/]+)\/terminals$/.exec(route))) return this.terminals.list(person, found[1] ?? '', query);
    if ((found = /^POST \/environments\/([^/]+)\/terminals$/.exec(route))) {
      const environment = found[1] ?? '';
      const thread = this.door(person, environment, query, 'terminal.manage');
      return this.terminals.open(person, environment, thread, await jsonOf(request));
    }
    if ((found = /^POST \/terminals\/([^/]+)\/ticket$/.exec(route))) return this.terminals.ticket(person, found[1] ?? '');
    if ((found = /^DELETE \/terminals\/([^/]+)$/.exec(route))) return this.terminals.close(person, found[1] ?? '', 'closed_by_user');
    if ((found = /^GET \/terminals\/([^/]+)\/recording$/.exec(route))) return this.terminals.recording(person, found[1] ?? '');

    if ((found = /^GET \/threads\/([^/]+)\/outline$/.exec(route))) return { status: 200, body: this.outline(person, found[1] ?? '', query) };
    return null;
  }

  private tellProcesses(): void {
    for (const watcher of this.watchers) {
      if (watcher.feed.endsWith(':procs')) {
        watcher.subscriber.send({ op: 'event', id: watcher.id, feed: watcher.feed, data: { as_of: new Date().toISOString(), processes: this.processes } });
      }
    }
  }

  // ---------------------------------------------------------------- files

  /** Screen double for the native whole-page contract. Cursors retain one unchanged observation. */
  private directoryPage(environment: string, path: string, source: 'live' | 'cached', cursor: string | null = null, limit = 5000): DirListing {
    let start = 0, listing: DirListing;
    if (cursor !== null) {
      const decoded: unknown = JSON.parse(Buffer.from(cursor, 'base64url').toString());
      if (typeof decoded !== 'object' || decoded === null || !('revision' in decoded) || typeof decoded.revision !== 'string'
        || !('start' in decoded) || typeof decoded.start !== 'number' || !Number.isSafeInteger(decoded.start) || decoded.start < 0) throw new Refusal(400, 'request.invalid', 'invalid cursor');
      const held = this.directoryWindows.get(decoded.revision);
      if (held?.environment !== environment || held.listing.path !== path || held.listing.source !== source) throw new Refusal(409, 'conflict.stale_revision', 'restart the directory listing');
      listing = held.listing; start = decoded.start;
    } else {
      const all = this.children(environment, path);
      const entries = source === 'cached' && this.override(environment).index === 'live' ? all.filter((entry) => entry.type === 'dir' || entry.cached) : all;
      const revision = sha(Buffer.from(JSON.stringify([environment, path, source, entries])));
      const existing = this.directoryWindows.get(revision);
      if (existing !== undefined) listing = existing.listing;
      else {
        const asOf = new Date().toISOString();
        listing = { path, mount: null, entries, source, revision, as_of: asOf, observed_from: asOf,
          expires_at: new Date(Date.now() + 5 * 60_000).toISOString(), truncated: false, next_cursor: null };
        if (this.directoryWindows.size >= 256) { const oldest = this.directoryWindows.keys().next().value; if (oldest !== undefined) this.directoryWindows.delete(oldest); }
        this.directoryWindows.set(revision, { environment, listing });
      }
    }
    const token = (offset: number) => Buffer.from(JSON.stringify({ revision: listing.revision, start: offset })).toString('base64url');
    const entries: DirEntry[] = [];
    // Exact encoded rows plus a wrapper carrying the largest possible offset; no sliced reset can skip rows.
    let bytes = Buffer.byteLength(JSON.stringify({ ...listing, entries: [], next_cursor: token(Number.MAX_SAFE_INTEGER), truncated: false }));
    for (let at = start; at < listing.entries.length && entries.length < Math.min(limit, 5000); at += 1) {
      const entry = listing.entries[at]; if (entry === undefined) break;
      const cost = Buffer.byteLength(JSON.stringify(entry)) + Number(entries.length > 0);
      if (bytes + cost > 2 * 1024 * 1024) break;
      entries.push(entry); bytes += cost;
    }
    const next = start + entries.length;
    return { ...listing, entries, truncated: next < listing.entries.length, next_cursor: next < listing.entries.length ? token(next) : null };
  }

  private list(person: Person, environment: string, query: URLSearchParams): Answer {
    this.door(person, environment, query, 'files.view');
    this.seen.push('list');
    const path = this.cleanPath(query.get('path'));
    const tree = this.tree(environment);
    const node = tree.get(path);
    if (path !== '' && node?.type !== 'dir') throw new Refusal(404, 'not_found', 'there is no such thing, or it was deleted');
    const away = this.away(environment);
    const source = query.get('source');
    if (source === 'live' && away) throw new Refusal(503, 'source.offline', 'the environment is away');
    return { status: 200, body: this.directoryPage(environment, path, away || source === 'cached' ? 'cached' : 'live', query.get('cursor'), Number(query.get('limit') ?? 5000)) };
  }

  private stat(person: Person, environment: string, query: URLSearchParams): Answer {
    this.door(person, environment, query, 'files.view');
    const path = this.cleanPath(query.get('path'));
    const node = this.tree(environment).get(path);
    if (node === undefined) throw new Refusal(404, 'not_found', 'there is no such thing, or it was deleted');
    return { status: 200, body: this.statOf(environment, path, node) };
  }

  private fetch(person: Person, environment: string, query: URLSearchParams, body: Record<string, unknown>): Answer {
    this.door(person, environment, query, 'files.view');
    const path = this.cleanPath(typeof body.path === 'string' ? body.path : null);
    const node = this.tree(environment).get(path);
    if (node?.type !== 'file') return { status: 200, body: { hash: null, reason: 'not_found', received: null, size: null, state: 'unavailable' } };
    if (this.away(environment) && !(this.cached.get(environment)?.has(path) ?? false)) {
      return { status: 200, body: { hash: null, reason: 'offline_not_cached', received: null, size: null, state: 'unavailable' } };
    }
    const key = `${environment}\u0000${path}`;
    if (this.slow.has(key)) {
      this.slow.delete(key);
      return { status: 200, body: { hash: null, reason: null, received: 10, size: node.content.length, state: 'fetching' } };
    }
    this.cached.get(environment)?.add(path);
    if (!this.cached.has(environment)) this.cached.set(environment, new Set([path]));
    return { status: 200, body: { hash: sha(node.content), reason: null, received: null, size: node.content.length, state: 'ready' } };
  }

  private content(person: Person, environment: string, query: URLSearchParams, request: IncomingMessage): Answer {
    this.door(person, environment, query, 'files.view');
    const path = this.cleanPath(query.get('path'));
    const node = this.tree(environment).get(path);
    if (node?.type !== 'file' || sha(node.content) !== query.get('hash')) throw new Refusal(404, 'not_found', 'there is no such thing, or it was deleted');
    if (query.get('preview') === 'true') return this.preview(person, environment, query);
    this.seen.push('content');
    const range = /^bytes=(\d+)-(\d+)?$/.exec(request.headers.range ?? '');
    const hash = sha(node.content);
    if (range === null) return { status: 200, bytes: node.content, headers: { 'Content-Type': mimeOf(path, node.content), ETag: `"${hash}"` } };
    // No range of a file with nothing in it can be had: the real server answers 416 and `bytes */0`, with no body.
    if (node.content.length === 0) return { status: 416, bytes: Buffer.alloc(0), headers: { 'Content-Range': 'bytes */0' } };
    const from = Number(range[1]);
    const to = Math.min(range[2] === undefined ? node.content.length - 1 : Number(range[2]), node.content.length - 1);
    return {
      status: 206,
      bytes: node.content.subarray(from, to + 1),
      headers: { 'Content-Type': mimeOf(path, node.content), ETag: `"${hash}"`, 'Content-Range': `bytes ${from}-${to}/${node.content.length}` },
    };
  }

  /** The real server derives bounded previews; this fixture's only raster picture is already a tiny PNG. */
  private preview(person: Person, environment: string, query: URLSearchParams): Answer {
    this.door(person, environment, query, 'files.view');
    const path = this.cleanPath(query.get('path'));
    const node = this.tree(environment).get(path);
    if (node?.type !== 'file') throw new Refusal(404, 'not_found', 'the file no longer exists');
    this.seen.push('preview');
    const media = mimeOf(path, node.content);
    const image = media === 'image/png' && node.content.equals(TINY_PNG) ? `data:image/png;base64,${node.content.toString('base64')}` : null;
    const text = isText(node.content) && !media.startsWith('image/') && media !== 'application/pdf' ? node.content.subarray(0, 262144).toString('utf8').split('\n').slice(0, 5).join('\n') : null;
    return { status: 200, body: { state: image === null && text === null ? 'unavailable' : 'ready', hash: sha(node.content), size: node.content.length,
      media_type: media, is_text: text !== null, image, text, width: image === null ? null : 1, height: image === null ? null : 1,
      duration_seconds: null, entry_count: null, viewer_bytes: 64 * 1024 * 1024 } };
  }

  private write(person: Person, environment: string, query: URLSearchParams, content: Buffer): Answer {
    this.door(person, environment, query, 'files.manage');
    this.writable(environment);
    this.seen.push('write');
    const path = this.cleanPath(query.get('path'));
    if (path === '') throw new Refusal(400, 'request.invalid', 'a file needs a name', { errors: [] });
    const tree = this.tree(environment);
    const before = tree.get(path);
    const expected = query.get('expected_hash');
    if (before?.type === 'dir') throw new Refusal(409, 'conflict.exists', 'that is a directory');
    if (expected !== null) {
      const stale = expected === 'none' ? before !== undefined : before === undefined || sha(before.content) !== expected;
      if (stale) {
        throw new Refusal(409, 'conflict.stale_revision', 'the file is not the version that was loaded; nothing was written', before === undefined ? {} : { current: this.statOf(environment, path, before) });
      }
    }
    this.put(environment, path, content);
    const written = tree.get(path);
    if (written === undefined) throw new Refusal(500, 'internal', 'not written');
    this.cached.get(environment)?.add(path);
    this.touched(environment, path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '');
    return { status: 200, body: this.statOf(environment, path, written) };
  }

  private mkdir(person: Person, environment: string, query: URLSearchParams, body: Record<string, unknown>): Answer {
    this.door(person, environment, query, 'files.manage');
    this.writable(environment);
    const path = this.cleanPath(typeof body.path === 'string' ? body.path : null);
    const tree = this.tree(environment);
    if (tree.has(path)) throw new Refusal(409, 'conflict.exists', 'that is there already');
    const parts = path.split('/');
    for (let index = 1; index <= parts.length; index += 1) {
      const directory = parts.slice(0, index).join('/');
      if (!tree.has(directory)) tree.set(directory, { type: 'dir', content: Buffer.alloc(0), mtime: Date.now(), mode: 0o755 });
    }
    this.touched(environment, parts.slice(0, -1).join('/'));
    const made = tree.get(path);
    if (made === undefined) throw new Refusal(500, 'internal', 'not made');
    return { status: 200, body: this.statOf(environment, path, made) };
  }

  private moveOrCopy(person: Person, environment: string, query: URLSearchParams, body: Record<string, unknown>, copy: boolean): Answer {
    this.door(person, environment, query, 'files.manage');
    this.writable(environment);
    const from = this.cleanPath(typeof body.from === 'string' ? body.from : null);
    const to = this.cleanPath(typeof body.to === 'string' ? body.to : null);
    const tree = this.tree(environment);
    if (!tree.has(from)) throw new Refusal(404, 'not_found', 'there is no such thing, or it was deleted');
    if (tree.has(to)) throw new Refusal(409, 'conflict.exists', 'that is there already');
    const moved = [...tree.entries()].filter(([path]) => path === from || path.startsWith(`${from}/`));
    const at = Date.now();
    for (const [path, node] of moved) {
      const target = to + path.slice(from.length);
      if (!copy) tree.delete(path);
      // The directories above a destination are made, so that a file lands in a place that exists.
      const parts = target.split('/');
      for (let index = 1; index < parts.length; index += 1) {
        const directory = parts.slice(0, index).join('/');
        if (!tree.has(directory)) tree.set(directory, { type: 'dir', content: Buffer.alloc(0), mtime: at, mode: 0o755 });
      }
      tree.set(target, { ...node, mtime: at });
    }
    const parentOf = (path: string) => (path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '');
    this.touched(environment, parentOf(to));
    if (!copy) this.touched(environment, parentOf(from), [from.slice(from.lastIndexOf('/') + 1)]);
    const made = tree.get(to);
    if (made === undefined) throw new Refusal(500, 'internal', 'not moved');
    return { status: 200, body: this.statOf(environment, to, made) };
  }

  private remove(person: Person, environment: string, query: URLSearchParams): Answer {
    this.door(person, environment, query, 'files.manage');
    this.writable(environment);
    const path = this.cleanPath(query.get('path'));
    const tree = this.tree(environment);
    const node = tree.get(path);
    if (node === undefined) throw new Refusal(404, 'not_found', 'there is no such thing, or it was deleted');
    if (node.type === 'dir' && query.get('recursive') !== 'true' && [...tree.keys()].some((key) => key.startsWith(`${path}/`))) {
      throw new Refusal(409, 'conflict.exists', 'the directory is not empty');
    }
    for (const key of [...tree.keys()]) if (key === path || key.startsWith(`${path}/`)) tree.delete(key);
    this.touched(environment, path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '', [path.slice(path.lastIndexOf('/') + 1)]);
    return { status: 204 };
  }

  private search(person: Person, environment: string, query: URLSearchParams): Answer {
    this.door(person, environment, query, 'files.view');
    const text = query.get('q') ?? '';
    if (text.length < 2) throw new Refusal(400, 'request.invalid', 'q needs at least two characters', { errors: [{ path: 'q', message: 'too short' }] });
    const tree = this.tree(environment);
    if (query.get('in') === 'content') {
      if (this.away(environment)) throw new Refusal(503, 'source.offline', 'the environment is away');
      const under = this.cleanPath(query.get('path'));
      const matcher = query.get('regex') === 'true' ? new RegExp(text, query.get('case') === 'true' ? 'g' : 'gi') : new RegExp(text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), query.get('case') === 'true' ? 'g' : 'gi');
      const matches: { line: number; path: string; ranges: [number, number][]; text: string }[] = [];
      let searched = 0;
      for (const [path, node] of tree) {
        if (node.type !== 'file' || !isText(node.content) || (under !== '' && !path.startsWith(`${under}/`))) continue;
        searched += 1;
        node.content
          .toString('utf8')
          .split('\n')
          .forEach((line, index) => {
            const ranges: [number, number][] = [];
            for (const hit of line.matchAll(matcher)) ranges.push([hit.index, hit.index + hit[0].length]);
            if (ranges.length > 0) matches.push({ line: index + 1, path, ranges, text: line.slice(0, 400) });
          });
      }
      return { status: 200, body: { matches: matches.slice(0, 200), truncated: matches.length > 200, searched } };
    }
    if (this.override(environment).index === 'live') throw new Refusal(409, 'workspace.not_indexed', 'this workspace is too large to index');
    const lower = text.toLowerCase();
    const found: DirEntry[] = [];
    for (const [path, node] of tree) if (path.toLowerCase().includes(lower)) found.push(this.entryOf(environment, path, node));
    return { status: 200, body: found.slice(0, 200) };
  }

  private archive(person: Person, environment: string, query: URLSearchParams): Answer {
    this.door(person, environment, query, 'files.view');
    const path = this.cleanPath(query.get('path'));
    const archive = this.archives.get(`${environment}\u0000${path}`);
    if (archive === undefined) throw new Refusal(404, 'not_found', 'there is no such thing, or it was deleted');
    const inside = this.cleanPath(query.get('inside'));
    const prefix = inside === '' ? '' : `${inside}/`;
    const entries: { path: string; kind: 'file' | 'dir' | 'link'; size: number | null; modified_at: string | null }[] = [];
    const seen = new Set<string>();
    for (const [entry, content] of archive.entries) {
      const clean = entry.replace(/\/$/, '');
      if (!clean.startsWith(prefix) || clean === inside) continue;
      const rest = clean.slice(prefix.length);
      const first = rest.split('/')[0] ?? rest;
      const isDir = rest.includes('/') || content === null;
      const full = `${prefix}${first}`;
      if (seen.has(full)) continue;
      seen.add(full);
      entries.push({ path: full, kind: isDir ? 'dir' : 'file', size: isDir ? null : content.length, modified_at: '2026-10-01T09:00:00.000Z' });
    }
    entries.sort((one, other) => (one.kind === other.kind ? one.path.localeCompare(other.path) : one.kind === 'dir' ? -1 : 1));
    return { status: 200, body: { format: archive.format, compression: archive.compression, entries, truncated: false, encrypted: archive.encrypted } };
  }

  private archiveEntry(person: Person, environment: string, query: URLSearchParams): Answer {
    this.door(person, environment, query, 'files.view');
    const archive = this.archives.get(`${environment}\u0000${this.cleanPath(query.get('path'))}`);
    const content = archive?.entries.get(query.get('entry') ?? '');
    if (archive === undefined || content === undefined || content === null) throw new Refusal(404, 'not_found', 'there is no such thing, or it was deleted');
    if (content.length > PREVIEW_BYTES) throw new Refusal(413, 'request.too_large', 'the entry is larger than can be looked at');
    return { status: 200, bytes: content, headers: { 'Content-Type': 'application/octet-stream' } };
  }

  // ---------------------------------------------------------------- load

  private samples(environment: string, minutes: number): ResourceSample[] {
    const over = this.override(environment);
    const count = Math.min(Math.max(minutes, 1), 360) * 12;
    const end = Date.now();
    return Array.from({ length: count }, (_, index) => {
      const at = end - (count - 1 - index) * 5_000;
      const wave = (Math.sin(index / 9) + 1) / 2;
      return {
        at: new Date(at).toISOString(),
        cpu_limit_millis: over.limits.cpu_millis,
        cpu_percent: 20 + wave * 60,
        disk_limit_bytes: over.limits.disk_bytes,
        disk_used_bytes: 20 * 1024 ** 3 + index * 1024 ** 2,
        memory_bytes: Math.round(2 * 1024 ** 3 + wave * 1024 ** 3),
        memory_limit_bytes: over.limits.memory_bytes,
        net_rx_bps: Math.round(wave * 400_000),
        net_tx_bps: Math.round(wave * 120_000),
        pids: 40 + Math.round(wave * 10),
      };
    });
  }

  // ---------------------------------------------------------------- the conversation's outline

  private outline(person: Person, threadId: string, query: URLSearchParams): ThreadOutlinePage {
    this.world.places.need(person, threadId, 'thread.view');
    const view = this.world.threads.snapshot(threadId) as { items: Item[]; turns: Turn[]; seq: number };
    return this.outlineSnapshot(view, query);
  }

  /** The same bounded outline from an already authorized main or side conversation. */
  outlineSnapshot(view: { items: readonly Item[]; turns: readonly Turn[]; seq: number }, query: URLSearchParams): ThreadOutlinePage {
    const limit = Number(query.get('limit') ?? 200), before = query.get('before'), around = query.get('around'), turn = query.get('turn');
    if (!Number.isInteger(limit) || limit < 1 || limit > 500 || [before, around, turn].filter((v) => v !== null).length > 1) throw new Refusal(400, 'request.invalid', 'invalid outline page');
    const entries: OutlineEntry[] = [];
    const first = new Map<string, string>();
    for (const item of view.items.toSorted((a, b) => a.seq_created - b.seq_created)) {
      if (item.stream !== 'main') continue;
      if (item.turn_id !== null && !first.has(item.turn_id)) first.set(item.turn_id, item.id);
      const numbered = view.turns.find((one) => first.get(one.id) === item.id);
      if (numbered !== undefined) entries.push({ at: item.created_at, item_id: item.id, seq: item.seq_created, kind: 'turn', title: '', turn_ordinal: numbered.ordinal });
      const kind = item.kind === 'user_message' ? 'user_message' : item.kind === 'agent_message' ? 'agent_message' : item.kind === 'approval' ? 'approval' : item.kind === 'error' ? 'error' : item.kind === 'marker' ? 'marker' : null;
      if (kind === null) continue;
      const payload = item.payload as Record<string, unknown>;
      const parts = Array.isArray(payload.parts) ? (payload.parts as { type?: string; text?: string }[]) : [];
      const spoken = parts.flatMap((part) => (part.type === 'text' && typeof part.text === 'string' ? [part.text] : [])).join(' ');
      const text = spoken !== '' ? spoken : typeof payload.message === 'string' ? payload.message : typeof payload.text === 'string' ? payload.text : typeof payload.title === 'string' ? payload.title : item.kind;
      const goalCondition = item.kind === 'user_message' && item.payload.source === 'goal' ? Array.from(item.payload.goal?.condition ?? '').slice(0, 80).join('') : undefined;
      entries.push({ at: item.created_at, item_id: item.id, kind, seq: item.seq_created, title: goalCondition ?? text.slice(0, 120), ...(goalCondition === undefined ? {} : { goal_condition: goalCondition }) });
    }
    const points = [...new Set(entries.map((entry) => entry.seq))].sort((a, b) => a - b);
    let selected: number[];
    if (around !== null || turn !== null) {
      const anchor = entries.find((entry) => around !== null ? entry.item_id === around : entry.kind === 'turn' && entry.turn_ordinal === Number(turn));
      if (anchor === undefined) throw new Refusal(404, 'not_found', 'there is no such outline point');
      const start = Math.max(0, points.indexOf(anchor.seq) - Math.floor(limit / 2));
      selected = points.slice(start, start + limit);
    } else selected = points.filter((seq) => before === null || seq < Number(before)).slice(-limit);
    const oldest = selected[0];
    return { entries: entries.filter((entry) => selected.includes(entry.seq)), next_before: oldest !== undefined && points.some((seq) => seq < oldest) ? oldest : null, seq: view.seq };
  }

  // ---------------------------------------------------------------- what a test steers

  /** The requests only the double has. */
  control(path: string, body: Record<string, unknown>): Answer {
    const environment = typeof body.environment === 'string' ? body.environment : MAIN;
    switch (path) {
      case '/directory-event': {
        const directory = this.cleanPath(typeof body.path === 'string' ? body.path : '');
        if (body.kind !== 'failure' && body.kind !== 'changed') throw new Refusal(400, 'request.invalid', 'unknown directory event');
        const data: DirData = { as_of: new Date().toISOString(), changed: [], removed: [] };
        if (body.kind === 'failure') data.error = { code: 'environment.unavailable', reason: 'busy', detail: 'The directory watch could not be installed.', retryable: true };
        else { data.invalidated = true; data.truncated = true; }
        const feed = `env:${environment}:dir:${this.feedPath(directory)}`;
        for (const watcher of this.watchers) if (watcher.feed === feed) watcher.subscriber.send({ op: 'event', id: watcher.id, feed, data });
        return { status: 204 };
      }
      case '/reset':
        this.seed();
        return { status: 204 };
      case '/put': {
        // Files appear as if somebody else wrote them: an agent, or a shell.
        const directory = (path: string) => (path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '');
        const files = (body.files ?? {}) as Record<string, string | { base64: string }>;
        for (const [name, content] of Object.entries(files)) {
          this.put(environment, name, typeof content === 'string' ? content : Buffer.from(content.base64, 'base64'));
          this.touched(environment, directory(name));
        }
        return { status: 204 };
      }
      case '/delete': {
        const name = typeof body.path === 'string' ? body.path : '';
        const tree = this.tree(environment);
        for (const key of [...tree.keys()]) if (key === name || key.startsWith(`${name}/`)) tree.delete(key);
        this.touched(environment, name.includes('/') ? name.slice(0, name.lastIndexOf('/')) : '', [name.slice(name.lastIndexOf('/') + 1)]);
        return { status: 204 };
      }
      case '/fill': {
        // A directory of thousands of files, and optionally thousands of lines in one of them.
        const directory = typeof body.directory === 'string' ? body.directory : 'many';
        const count = typeof body.files === 'number' ? body.files : 1_000;
        const folders = typeof body.directories === 'number' ? body.directories : 0;
        const at = Date.parse('2026-10-01T09:00:00.000Z');
        for (let index = 0; index < count; index += 1) this.put(environment, `${directory}/file-${String(index).padStart(5, '0')}.txt`, `file ${index}\n`, at);
        for (let index = 0; index < folders; index += 1) this.put(environment, `${directory}/folder-${String(index).padStart(4, '0')}/inside.txt`, 'inside\n', at);
        return { status: 200, body: { entries: this.children(environment, directory).length } };
      }
      case '/lines': {
        const name = typeof body.path === 'string' ? body.path : 'big/long.ts';
        const count = typeof body.lines === 'number' ? body.lines : 20_000;
        const lines = Array.from({ length: count }, (_, index) => `export const value${index} = ${index}; // line ${index + 1}`);
        this.put(environment, name, `${lines.join('\n')}\n`, Date.parse('2026-10-01T09:00:00.000Z'));
        return { status: 200, body: { bytes: this.tree(environment).get(name)?.content.length ?? 0 } };
      }
      case '/environment': {
        // The state and the switches are written on the record of the environment, where every screen reads them; the limits are the workspace's.
        const patch = (body.patch ?? {}) as { status?: Environment['status']; index?: 'full' | 'live'; remote_write?: boolean; terminal?: boolean; limits?: Facts['limits']; machine_online?: boolean };
        this.world.resources.steerWorkspace(environment, {
          ...(patch.status === undefined ? {} : { status: patch.status }),
          ...(patch.index === undefined ? {} : { index: patch.index }),
          ...(patch.remote_write === undefined ? {} : { remoteWrite: patch.remote_write }),
          ...(patch.terminal === undefined ? {} : { terminal: patch.terminal }),
          ...(patch.machine_online === undefined ? {} : { machineOnline: patch.machine_online }),
        });
        if (patch.limits !== undefined) this.limits.set(environment, patch.limits);
        if (this.away(environment) && !this.cached.has(environment)) this.cached.set(environment, new Set());
        return { status: 204 };
      }
      case '/bind-thread': {
        const thread = typeof body.thread === 'string' ? body.thread : ACME.mainThread;
        const directory = typeof body.directory === 'string' ? body.directory : '';
        this.world.resources.bind(thread, { ...this.world.resources.bindingOf(thread), environment, directory });
        this.world.threads.emit(thread, { thread: { directory } });
        return { status: 204 };
      }
      case '/feeds':
        this.feeds = body.on !== false;
        return { status: 204 };
      case '/slow':
        this.slow.add(`${environment}\u0000${String(body.path)}`);
        return { status: 204 };
      case '/processes':
        this.processes = ((body.processes ?? []) as Partial<ProcessInfo>[]).map((one, index) => this.process(one.pid ?? this.nextPid + index, one.ppid ?? 1, one.name ?? 'proc', one.command ?? one.name ?? 'proc', one.cpu_percent ?? 0, one.memory_bytes ?? 1024 ** 2, one.thread_id ?? null, one.item_id ?? null, one.user ?? 'agent'));
        this.nextPid += this.processes.length;
        this.tellProcesses();
        return { status: 204 };
      case '/changes':
        this.changes.set(environment, (body.changes ?? []) as EnvironmentChange[]);
        return { status: 204 };
      case '/git':
        this.git.set(environment, body.status as GitStatus);
        return { status: 204 };
      case '/seen': {
        const seen = this.seen.slice();
        if (body.clear === true) this.seen = [];
        return { status: 200, body: { seen } };
      }
      case '/content': {
        const name = typeof body.path === 'string' ? body.path : '';
        const node = this.tree(environment).get(name);
        return { status: 200, body: { text: node?.content.toString('utf8') ?? null } };
      }
      default:
        return this.terminals.control(path, body);
    }
  }

  /** Who the opener of a shell is, as a reference. */
  userRefOf(person: Person): UserRef {
    return userRef(person.me);
  }

  /** The thread a shell is opened through. */
  environmentIds(): string[] {
    return this.world.resources.environments.map((environment) => environment.id);
  }
}

async function chunks(request: IncomingMessage): Promise<Buffer> {
  const parts: Buffer[] = [];
  for await (const chunk of request) parts.push(chunk as Buffer);
  return Buffer.concat(parts);
}

async function jsonOf(request: IncomingMessage): Promise<Record<string, unknown>> {
  const raw = await chunks(request);
  if (raw.length === 0) return {};
  try {
    const value: unknown = JSON.parse(raw.toString('utf8'));
    return typeof value === 'object' && value !== null ? (value as Record<string, unknown>) : {};
  } catch {
    throw new Refusal(400, 'request.invalid', 'the body is not JSON', { errors: [] });
  }
}

async function rawOf(request: IncomingMessage): Promise<Buffer> {
  return chunks(request);
}
