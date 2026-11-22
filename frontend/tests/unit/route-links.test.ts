// @vitest-environment node
/// <reference types="node" />
/** No link leads to a page that is not built: a check over the routes. Every address the application writes, as a
 * link, a menu entry or a place it goes to, is a route of the route tree, and none is a route whose screen is a
 * stand-in that says it is not built yet. */
import { readFileSync, readdirSync } from 'node:fs';
import { relative, resolve, sep } from 'node:path';
import { describe, expect, it } from 'vitest';
import ts from 'typescript';

const sourceRoot = resolve(import.meta.dirname, '../../src');
const routesRoot = resolve(sourceRoot, 'routes');

function files(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) return entry.name === 'generated' ? [] : files(path);
    return /\.tsx?$/.test(entry.name) && entry.name !== 'routeTree.gen.ts' ? [path] : [];
  });
}

interface Route {
  segments: string[];
  file: string;
  /** Its screen says only that it is not built yet. */
  standIn: boolean;
}

/** The routes as each route file declares its own address to the router, the way the release walk reads them. */
function routesIn(root: string): Route[] {
  return files(root).flatMap((file) => {
    const source = readFileSync(file, 'utf8');
    const standIn = source.includes('@/shell/NotBuilt') || source.includes('shell.notBuilt');
    return [...source.matchAll(/createFileRoute\(['"]([^'"]+)['"]\)/g)].map((declared) => ({
      segments: (declared[1] ?? '/').split('/').filter((part) => part !== ''),
      file: relative(sourceRoot, file),
      standIn,
    }));
  });
}

/** What stands for a part of an address that is filled in when it is used. */
const FILLED = '\u0000';

function segmentsOf(address: string): string[] {
  const path = address.replace(/\$\{[^}]*\}/g, FILLED).split(/[?#]/)[0] ?? '';
  return path.split('/').filter((segment) => segment !== '');
}

function matches(address: readonly string[], route: readonly string[]): boolean {
  // A filled-in last part may stand for several segments: a place's own path, say.
  const open = address.at(-1) === FILLED;
  if (address.length !== route.length && !(open && route.length > address.length)) return false;
  return address.every((segment, at) => {
    const pattern = route[at] ?? '';
    return pattern.startsWith('$') || segment.includes(FILLED) || segment === pattern;
  });
}

/** A link, a menu entry or a place gone to: `to` and `href` with an address of the application. */
const TARGET = /(?:\bto|\bhref)\s*[:=]\s*\{?\s*(['"`])(\/[^'"`\s]*)\1/g;
/** Any other text that begins as an address of the application does: what a helper builds an address from. */
const LITERAL = /(['"`])(\/[a-z][^'"`\s]*)\1/g;
/** A request to the server, whose paths are its own and not the application's. */
const REQUEST = /(?:\.(?:GET|POST|PUT|PATCH|DELETE)|\btrace(?:Json(?:<[^>]*>)?|Request))\(\s*$/;
/** A pathname prefix/suffix used to recognize a section is not a destination. Explicit to/href remain checked. */
const PATH_TEST = /\.(?:startsWith|endsWith)\(\s*$/;
/** Equality against the current pathname also only recognizes a section; it does not open that path. */
const PATH_EQUALITY = /\bpathname\s*(?:===|!==)\s*$/;

interface Address {
  text: string;
  where: string;
}

function addressesInSource(source: string, file: string, firsts: ReadonlySet<string>, requestModule = false): Address[] {
    const where = (index: number) => `${file}:${String(source.slice(0, index).split('\n').length)}`;
    const found: Address[] = [];
    for (const target of source.matchAll(TARGET)) found.push({ text: target[2] ?? '', where: where(target.index) });
    if (!requestModule) {
      for (const literal of source.matchAll(LITERAL)) {
        const text = literal[2] ?? '';
        const first = text.slice(1).split(/[/?#]/)[0] ?? '';
        const before = source.slice(Math.max(0, literal.index - 40), literal.index);
        if (!firsts.has(first) || REQUEST.test(before) || PATH_TEST.test(before) || PATH_EQUALITY.test(before)) continue;
        found.push({ text, where: where(literal.index) });
      }
    }
    return found;
}

function addressesIn(root: string, firsts: ReadonlySet<string>): Address[] {
  const requests = resolve(root, 'api') + sep;
  return files(root).flatMap((file) => addressesInSource(readFileSync(file, 'utf8'), relative(root, file), firsts, file.startsWith(requests)));
}

const ROUTES = routesIn(routesRoot);
const FIRSTS = new Set(ROUTES.flatMap((route) => route.segments.slice(0, 1)));
const ADDRESSES = addressesIn(sourceRoot, FIRSTS);

describe('every address leads to a built screen', () => {
  it('distinguishes pathname recognition from a link or navigation to the same unbuilt address', () => {
    const source = [
      "const selected = pathname === '/settings/not-a-screen/deep' || pathname.startsWith('/settings/not-a-screen/deep/');",
      '<a href="/settings/not-a-screen/deep">Settings</a>',
      "navigate({ href: '/settings/not-a-screen/deep' });",
    ].join('\n');
    const found = addressesInSource(source, 'fixture.tsx', new Set(['settings']));
    expect([...new Set(found.map((address) => address.where))]).toEqual(['fixture.tsx:2', 'fixture.tsx:3']);
    expect(found.every((address) => address.text === '/settings/not-a-screen/deep')).toBe(true);
    expect(ROUTES.some((route) => matches(segmentsOf('/settings'), route.segments))).toBe(true);
    expect(ROUTES.some((route) => matches(segmentsOf('/settings/not-a-screen/deep'), route.segments))).toBe(false);
  });
  it('reads the route tree as the router does, and tells an address of it from one that is not', () => {
    const names = ROUTES.map((route) => `/${route.segments.join('/')}`);
    expect(names).toEqual(expect.arrayContaining(['/', '/org/$slug', '/org/$slug/settings/$section', '/resources/providers/$id']));
    const known = (address: string) => ROUTES.some((route) => matches(segmentsOf(address), route.segments));
    // `${}` stands for what a template fills in.
    expect(known('/org/acme')).toBe(true);
    expect(known('/project/${}?view=list')).toBe(true);
    expect(known('/org/$slug/settings/$section')).toBe(true);
    expect(known('/resources/${}/${}')).toBe(true);
    expect(known('/chats/acme')).toBe(false);
    expect(known('/org/acme/nowhere/at/all')).toBe(false);
    expect(known('/resources/$kind')).toBe(false);
    expect(ADDRESSES.length).toBeGreaterThan(50);
  });

  it('writes no address that is not a route: a link, a menu entry or a place gone to', () => {
    const nowhere = ADDRESSES.filter((address) => !ROUTES.some((route) => matches(segmentsOf(address.text), route.segments)));
    const missing = nowhere.map((address) => `${address.where} ${address.text}`);
    expect(missing, `Unbuilt destinations: ${JSON.stringify(missing)}`).toEqual([]);
  });

  it('keeps API helpers out of page recognition while still checking explicit links with the same prefix', () => {
    const source = "traceJson<InspectedDocument>(`/documents/${id}/sync?inspect_deleted=true`);\ntraceRequest(`/documents/${id}/files/${file}?inspect_deleted=true`);\n<a href=\"/documents/unbuilt/deep\" />";
    expect(addressesInSource(source, 'fixture.tsx', new Set(['documents']))).toEqual([{ text: '/documents/unbuilt/deep', where: 'fixture.tsx:3' }, { text: '/documents/unbuilt/deep', where: 'fixture.tsx:3' }]);
  });

  it('leads nowhere that shows a stand-in instead of its screen', () => {
    const standIns = ROUTES.filter((route) => route.standIn);
    const toStandIns = ADDRESSES.filter((address) => {
      const segments = segmentsOf(address.text);
      // An address that one built route answers is not one of a stand-in, whatever other route it could also be.
      const answering = ROUTES.filter((route) => matches(segments, route.segments));
      return answering.length > 0 && answering.every((route) => route.standIn);
    });
    expect(toStandIns.map((address) => `${address.where} ${address.text} (${standIns.map((route) => route.file).join(', ')})`)).toEqual([]);
  });
});


/** Native document transitions have one explicit owner; ordinary app links use the shared routing component. */
const NATIVE_ANCHORS: Readonly<Record<string, string>> = {
  'shell/TrailLink.tsx': 'The shared anchor routes ordinary application paths and retains native browser acts',
  'features/auth/IdentityProviders.tsx': 'The server owns the identity-provider redirect',
  'shell/PublicAddressNotice.tsx': 'Open explicitly changes the browser origin',
};
const NATIVE_DOCUMENT_ACTS: Readonly<Record<string, string>> = {
  'shell/Banners.tsx': 'The explicit new-version Reload act',
  'features/admin/PublicAddress.tsx': 'The successful Save and open changes origin only for its actor',
  'features/auth/IdentityProviders.tsx': 'The server owns the identity-provider redirect',
  'features/auth/freshProof.ts': 'A server-owned fresh-proof continuation',
  'features/settings/personal/Security.tsx': 'Linking an identity leaves for its provider',
  'features/ports/Ports.tsx': 'A port entrance has its own origin and ticket',
  'features/chat/open.ts': 'A standalone part with no router retains normal browser navigation',
};
function navigationDefaults(source: string, file: string): string[] {
  const parsed = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, file.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
  const failures: string[] = [];
  const where = (node: ts.Node) => `${file}:${String(parsed.getLineAndCharacterOfPosition(node.getStart(parsed)).line + 1)}`;
  const text = (attribute: ts.JsxAttribute | undefined): string | null => {
    const value = attribute?.initializer;
    if (value === undefined) return null;
    if (ts.isStringLiteral(value)) return value.text;
    if (!ts.isJsxExpression(value) || value.expression === undefined) return null;
    return ts.isStringLiteralLike(value.expression) ? value.expression.text : ts.isTemplateExpression(value.expression) ? value.expression.head.text : null;
  };
  const visit = (node: ts.Node) => {
    if ((ts.isJsxOpeningElement(node) || ts.isJsxSelfClosingElement(node)) && node.tagName.getText(parsed) === 'a' && NATIVE_ANCHORS[file] === undefined) {
      const attributes = node.attributes.properties.filter(ts.isJsxAttribute);
      const named = (name: string) => attributes.find((attribute) => attribute.name.getText(parsed) === name);
      const href = named('href'), target = text(named('target')), value = text(href);
      const download = named('download');
      const downloading = download !== undefined && !(download.initializer !== undefined && ts.isJsxExpression(download.initializer) && download.initializer.expression?.kind === ts.SyntaxKind.FalseKeyword);
      const native = downloading || (target !== null && target !== '' && target !== '_self') || value?.startsWith('#') === true || (value !== null && /^[a-z][a-z0-9+.-]*:/iu.test(value));
      if (href !== undefined && !native) failures.push(`${where(node)} ordinary anchor`);
    }
    if (ts.isJsxOpeningElement(node) || ts.isJsxSelfClosingElement(node)) {
      const tag = node.tagName.getText(parsed), attributes = node.attributes.properties.filter(ts.isJsxAttribute);
      const named = (name: string) => attributes.find((attribute) => attribute.name.getText(parsed) === name);
      if (tag === 'button' && named('type') === undefined) failures.push(`${where(node)} implicit submit button`);
      if (tag === 'form' && named('onSubmit') === undefined && text(named('method')) !== 'dialog' && NATIVE_DOCUMENT_ACTS[file] === undefined) failures.push(`${where(node)} native form submission`);
    }
    if (ts.isCallExpression(node) && ts.isPropertyAccessExpression(node.expression) && ['assign', 'replace', 'reload'].includes(node.expression.name.text)) {
      const receiver = node.expression.expression.getText(parsed);
      if ((receiver === 'location' || receiver.endsWith('.location')) && NATIVE_DOCUMENT_ACTS[file] === undefined) failures.push(`${where(node)} document navigation`);
    }
    ts.forEachChild(node, visit);
  };
  visit(parsed);
  return failures;
}

it('distinguishes application navigation from downloads external tabs hashes and explicit protocol transitions', () => {
  expect(navigationDefaults(`const links = <><a href="/inbox" /><a href={path} /><a href="#part" /><a href="/file" download />
<a href="/inbox" target="_blank" /><a href="https://other.example.test" /></>;
window.location.assign('/settings/security');`, 'fixture.tsx')).toEqual([
    'fixture.tsx:1 ordinary anchor', 'fixture.tsx:1 ordinary anchor', 'fixture.tsx:3 document navigation',
  ]);
  expect(navigationDefaults('<a href="/inbox" download={false} />', 'fixture.tsx')).toEqual(['fixture.tsx:1 ordinary anchor']);
  expect(navigationDefaults('<a href={meta.public_url} />', 'shell/PublicAddressNotice.tsx')).toEqual([]);
});

it('keeps every application action and link from reloading its document unless its native flow requires it', () => {
  const failures = files(sourceRoot).flatMap((file) => navigationDefaults(readFileSync(file, 'utf8'), relative(sourceRoot, file)));
  expect(failures, `Document navigation: ${JSON.stringify(failures)}`).toEqual([]);
}, 30_000);

it('requires a native button type and a form submission owner so an ordinary press cannot reload the page', () => {
  expect(navigationDefaults('<form><button>Go</button></form>', 'fixture.tsx')).toEqual(['fixture.tsx:1 native form submission', 'fixture.tsx:1 implicit submit button']);
  expect(navigationDefaults('<form onSubmit={submit}><button type="button">Go</button></form>', 'fixture.tsx')).toEqual([]);
});
