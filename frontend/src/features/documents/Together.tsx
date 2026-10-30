/** Several people in one document, and an agent that was let write there: who is here, who writes in which block, and the change that came second kept beside the first.
 *
 * Two people who write in different blocks never meet. Beside a block somebody is writing in stands their
 * avatar, so that people keep out of each other's sentence without being told to. When two write in the
 * same block at the same moment, the one whose change came second finds the other's text standing and
 * their own kept as a block directly under it, with three small actions. An agent that was let write
 * shows the same way, with its name.
 */
import { Extension } from '@tiptap/core';
import type { Node as PmNode } from '@tiptap/pm/model';
import { Plugin, PluginKey } from '@tiptap/pm/state';
import { Decoration, DecorationSet } from '@tiptap/pm/view';

import { client } from '@/api/client';
import { containersOf, findContainer } from '@/features/documents/remote';
import { useMarks, type Marks } from '@/features/documents/marks';
import type { StoreApi } from 'zustand';
import { syncOf, type Kept, type Sync } from '@/features/documents/sync';
import type { Author } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { documentStateNow, useDocumentStore, WRITING_SHOWN_MS } from '@/realtime/stores/document';
import { identityGround } from '@/ui/Avatar';

export const togetherKey = new PluginKey('orchTogether');

/** The name of who wrote something: a person's, or an agent's. */
export function authorName(author: Author | null | undefined): string {
  if (author === null || author === undefined) return i18n.t('documents.together.someone');
  return 'display_name' in author ? author.display_name : author.agent;
}

const element = (tag: string, className: string, text?: string): HTMLElement => {
  const made = document.createElement(tag);
  made.className = className;
  if (text !== undefined) made.textContent = text;
  return made;
};

/** The avatars of the people who are writing in a block. */
function writerWidget(people: readonly { name: string; text: string; color: number }[]): HTMLElement {
  const box = element('span', 'oc-writer');
  box.contentEditable = 'false';
  for (const person of people) {
    const dot = element('span', `oc-writer-avatar ${identityGround(person.color)}`, person.text);
    dot.title = i18n.t('documents.together.writing', { name: person.name });
    dot.setAttribute('role', 'img');
    dot.setAttribute('aria-label', i18n.t('documents.together.writing', { name: person.name }));
    box.append(dot);
  }
  return box;
}

/** What is said under a block the person wrote that was kept beside somebody else's, and what can be done about it. */
function keptNotice(kept: Kept, sync: Sync): HTMLElement {
  const name = authorName(kept.by);
  const box = element('div', 'oc-kept');
  box.contentEditable = 'false';
  box.setAttribute('role', 'group');
  box.setAttribute('aria-label', i18n.t('documents.together.keptGroup'));
  box.append(element('span', '', i18n.t(kept.note === 'removed' ? 'documents.together.keptRemoved' : 'documents.together.keptChanged', { name })));
  const act = (label: string, run: () => void) => {
    const button = element('button', '', label) as HTMLButtonElement;
    button.type = 'button';
    // The caret stays where it is: the press is not a move into the button.
    button.addEventListener('mousedown', (event) => event.preventDefault());
    button.addEventListener('click', run);
    box.append(button);
  };
  if (kept.note === 'changed') act(i18n.t('documents.together.replace', { name }), () => sync.replaceTheirs(kept.id));
  act(i18n.t('documents.together.keepBoth'), () => sync.keepBoth(kept.id));
  act(i18n.t('documents.together.discard'), () => sync.discardMine(kept.id));
  return box;
}

/** Everything the page shows of other people's work and of what it is led to, as decorations of the editor's state. */
export function togetherExtension(options: { docId: string; marks?: StoreApi<Marks> }) {
  const markStore = options.marks ?? useMarks;
  return Extension.create({
    name: 'orchTogether',
    addProseMirrorPlugins() {
      return [
        new Plugin({
          key: togetherKey,
          props: {
            decorations: (state) => {
              const decorations: Decoration[] = [];
              const me = client.session.getState().userId;
              const now = Date.now();
              const writing = [...documentStateNow(options.docId).writing.values()].filter((one) => one.user.id !== me && now - one.at < WRITING_SHOWN_MS);
              const sync = syncOf(this.editor);
              const kept = sync === null ? [] : [...sync.kept.getState().byId.values()];
              const marks = markStore.getState();
              if (writing.length === 0 && kept.length === 0 && marks.changed.size === 0 && marks.highlight === null && marks.anchors.length === 0) return DecorationSet.empty;

              const place = (id: string): { pos: number; node: PmNode } | null => findContainer(state.doc, id);
              const byBlock = new Map<string, { name: string; text: string; color: number }[]>();
              for (const one of writing) {
                const list = byBlock.get(one.block) ?? [];
                list.push({ name: one.user.display_name, text: one.user.avatar.text, color: one.user.avatar.color });
                byBlock.set(one.block, list);
              }
              for (const [block, people] of byBlock) {
                const at = place(block);
                if (at !== null) decorations.push(Decoration.widget(at.pos + 1, () => writerWidget(people), { side: -1, key: `writer-${block}-${people.map((one) => one.name).join()}`, ignoreSelection: true }));
              }
              for (const one of kept) {
                const at = place(one.id);
                if (at === null || sync === null) continue;
                decorations.push(Decoration.node(at.pos, at.pos + at.node.nodeSize, { class: 'is-kept' }));
                decorations.push(Decoration.widget(at.pos + 1 + at.node.child(0).nodeSize, () => keptNotice(one, sync), { side: -1, key: `kept-${one.id}-${one.note}`, stopEvent: () => true }));
              }
              if (marks.changed.size > 0) {
                const index = containersOf(state.doc);
                for (const [id, who] of marks.changed) {
                  const at = index.get(id);
                  if (at !== undefined) decorations.push(Decoration.node(at.pos, at.pos + at.node.nodeSize, { class: 'is-changed', title: who }));
                }
              }
              if (marks.highlight !== null) {
                const at = place(marks.highlight);
                if (at !== null) decorations.push(Decoration.node(at.pos, at.pos + at.node.nodeSize, { class: 'is-highlighted' }));
              }
              // The words a discussion is about, where they are still in their block.
              for (const anchor of marks.anchors) {
                if (anchor.block === null || anchor.quote === '') continue;
                const at = place(anchor.block);
                if (at === null) continue;
                const content = at.node.child(0);
                let text = '';
                content.forEach((child) => {
                  text += child.isText ? (child.text ?? '') : child.type.name === 'hardBreak' ? '\n' : '￼';
                });
                const found = text.indexOf(anchor.quote);
                if (found < 0) continue;
                const from = at.pos + 2 + found;
                decorations.push(
                  Decoration.inline(from, from + anchor.quote.length, { class: `oc-commented${marks.focused === anchor.discussion ? ' is-open' : ''}`, 'data-discussion': anchor.discussion }),
                );
              }
              return DecorationSet.create(state.doc, decorations);
            },
            handleClick: (_view, _pos, event) => {
              const target = event.target instanceof Element ? event.target.closest<HTMLElement>('[data-discussion]') : null;
              if (target === null) return false;
              markStore.setState({ focused: target.dataset.discussion ?? null });
              return false;
            },
          },
          view: (view) => {
            // Anything that changes what is marked asks the page to be looked at again.
            const refresh = () => view.dispatch(view.state.tr.setMeta(togetherKey, 'refresh').setMeta('addToHistory', false));
            const stopMarks = markStore.subscribe(refresh);
            const stopWriting = subscribeWriting(options.docId, refresh);
            // The page's sync is made after its editor is, so it is looked for at every change of the state until it is there.
            let attached: Sync | null = null;
            let stopKept: (() => void) | undefined;
            const attach = () => {
              const sync = syncOf(this.editor);
              if (sync === null || sync === attached) return;
              stopKept?.();
              attached = sync;
              stopKept = sync.kept.subscribe(refresh);
            };
            attach();
            const tick = setInterval(() => {
              if (documentStateNow(options.docId).writing.size > 0) refresh();
            }, 1_000);
            return {
              update: attach,
              destroy() {
                stopMarks();
                stopWriting();
                stopKept?.();
                clearInterval(tick);
              },
            };
          },
        }),
      ];
    },
  });
}

/** Calls `change` whenever the signs of writing in a document change. */
function subscribeWriting(docId: string, change: () => void): () => void {
  let before = documentStateNow(docId).writing;
  return useDocumentStore.subscribe((store) => {
    const next = store.byId.get(docId)?.writing;
    if (next !== undefined && next !== before) {
      before = next;
      change();
    }
  });
}
