/** References in what an agent wrote, drawn as chips: a link to a reference's address as the chip of what it names, and a shared-tree path in a link or in backquotes as the agent's thread resolves it.
 *
 * What an agent writes in its answers is its harness's text, kept as it was written: the paths in it are the agent's
 * own, relative to its topic. They are resolved here as its thread resolves them, and drawn with the name the
 * reader may know; one that resolves to nothing the reader may read stays as it was written. A reference written as
 * agents write them, `id:<id>` with an optional part `#n`, is drawn the same way, as the reader may read what it names.
 * An address of the application or of the web stays a link.
 */
import { Children, isValidElement, type ReactNode, useCallback } from 'react';

import { IdChip, SharedPathChip, TypedChip } from '@/features/people/NamedChip';
import { idReferenceOf, referenceOfAddress } from '@/features/people/references';
import { ReferenceDrawer } from '@/lib/markdown';

/** The first parts of the application's own addresses: a link that begins so is a link of the application. */
const APPLICATION = /^\/(access|admin|approvals|chat|dev|doc|document-folder|documents|everything|help|inbox|inspect|invite|login|nodes|chats|org|place-chat|project|recover|register|reset|resources|search|settings|setup|shared|thread|topic|trace|u|usage|verify-email|workbench)(\/|\?|#|$)/;

/** Whether a text in backquotes is meant as a path of the tree: it ends in `.md` (a document, a conversation, a chat)
 * or in `/` (a place), or names a part. Other words in backquotes, such as a file of a workspace, are asked about
 * nowhere. */
function pathlike(text: string): boolean {
  return text.includes('/') || text.includes('\\') || /^[^\s]+\.[^\s.]+(?::[1-9]\d*)?$/.test(text) || /[#:][1-9]\d*$/.test(text);
}

/** Whether a link's destination is a path of the tree, rather than an address of the web, of mail or of the application. */
function treePath(href: string): boolean {
  if (/^[a-z]:[\\/]/i.test(href)) return true;
  return href.trim() !== '' && !/^[a-z][a-z0-9+.-]*:/i.test(href) && !href.startsWith('#') && !href.startsWith('//') && !APPLICATION.test(href);
}

/** The words already shared in Markdown, without asking the named thing for any metadata. */
function sharedLabel(node: ReactNode): string {
  return Children.toArray(node).map((child) => typeof child === 'string' || typeof child === 'number' ? String(child) : isValidElement<{children?:ReactNode}>(child) ? sharedLabel(child.props.children) : '').join('');
}

export function AgentReferences({ threadId, children }: { threadId: string; children: ReactNode }) {
  const draw = useCallback((target: string, code: boolean, fallback: ReactNode): ReactNode | null => {
    const label = sharedLabel(fallback);
    // `id:<id>#n`, as agents write references, in a link or in backquotes: resolved by what the reader may read.
    const written = idReferenceOf(target);
    if (written?.path !== undefined) return <TypedChip reference={{ kind: 'environment_file', id: written.id, path: written.path, label, ...(written.number === undefined ? {} : { line: written.number }) }} />;
    if (written !== null) return <IdChip id={written.id} {...(written.number === undefined ? {} : { number: written.number })} fallback={label} />;
    if (!code) {
      const typed = referenceOfAddress(target);
      if (typed !== null) return <TypedChip reference={{...typed,label}} />;
    }
    if (code ? pathlike(target) : treePath(target)) return <SharedPathChip threadId={threadId} path={target} code={code} fallback={fallback} />;
    return null;
  }, [threadId]);
  // Without a thread there is nothing to resolve from: the text stays as it was written.
  if (threadId === '') return children;
  return <ReferenceDrawer.Provider value={draw}>{children}</ReferenceDrawer.Provider>;
}
