/** Feature-owned registrations provide working views; the frame cannot invent replacement bodies. */
import { expect, it } from 'vitest';

import '@/features/chat/register';
import '@/features/documents/register';
import { registerHelpView } from '@/features/help/register';
import { registerInspectorViews } from '@/features/inspector/Inspector';
import { shippedLayout } from '@/features/workbench/Arrangements';
import { allTabs, keepOffered, serialize } from '@/features/workbench/layout';
import { registerShippedViews } from '@/features/workbench/shippedViews';
import { offers, registerView, type ViewDefinition, viewOf } from '@/features/workbench/views';

const OWNED = ['outline', 'chat', 'files', 'documents', 'changes', 'git', 'processes', 'resources', 'terminal', 'environment', 'parameters', 'comments', 'help'] as const;
const context = { title: 'Thread', views: { ready: true, workspace: true, chat: true, terminal: true, processes: true } };

function definition(name: string): ViewDefinition {
  const view = viewOf(name);
  if (view === undefined) throw new Error(`The feature did not register ${name}`);
  return view;
}

it('restores all thirteen native views from shell registrations before any document page is imported', () => {
  registerInspectorViews(); registerHelpView();
  const native = OWNED.map(definition);
  for (const view of native) {
    expect(view.implemented, view.name).not.toBe(false);
    expect(offers(view, view.name === 'comments' ? 'document' : 'thread', context), view.name).toBe(true);
  }
  const saved = shippedLayout('thread', 'conversation', { left: 280, right: 400, bottom: 240 });
  const before = serialize(saved);
  try {
    // A feature unloading removes its registration; no stand-in body may take its name.
    for (const view of native) registerView({ ...view })();
    registerShippedViews(() => null);
    for (const name of OWNED) expect(viewOf(name), name).toBeUndefined();
    expect(definition('screen').closable).not.toBe(false);
    expect(definition('page').role).toBe('content');
    expect(definition('navigation').area).toBe('left');
    const visible = keepOffered(saved, (name) => viewOf(name) !== undefined);
    expect(allTabs(visible).some((tab) => OWNED.some((name) => name === tab.view))).toBe(false);
    expect(serialize(saved)).toBe(before);
  } finally { for (const view of native) registerView(view); }
  for (const original of native) {
    expect(definition(original.name).body).toBe(original.body);
    expect(offers(definition(original.name), original.name === 'comments' ? 'document' : 'thread', context)).toBe(true);
  }
  expect(serialize(saved)).toBe(before);
});
