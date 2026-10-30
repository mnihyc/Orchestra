/** Shared rules for authorized paths, dormant views and view choices on a person's device. */
import { describe, expect, it } from 'vitest';

import { offers, type ViewDefinition, viewOf } from '@/features/workbench/views';
import { registerInspectorViews } from '@/features/inspector/Inspector';
import { readViewChoice, viewChoiceKeyDetails, writeViewChoice } from '@/lib/viewChoices';
import { screenPath } from '@/shell/screenPath';
import { ACTIONS } from '@/ui/actions';

describe('the title bar says where one is', () => {
  it('keeps the authorized folder parents without repeating the document', () => {
    const authorized = [{ id: 'org', label: 'Acme', href: '/org/acme' }, { id: 'folder', label: 'Payments', href: '/project/folder' }];
    expect(screenPath({ title: 'Fee rules', leaf: true }, authorized, [])).toEqual(authorized);
  });
  it('keeps Messages as the parent of a selected conversation', () => {
    expect(screenPath({ title: 'Release review', leaf: true }, [{ id: 'org', label: 'Acme', href: '/org/acme' }], [{ id: 'messages', label: 'Messages', href: '/org/acme/messages' }])).toEqual([{ id: 'org', label: 'Acme', href: '/org/acme' }, { id: 'messages', label: 'Messages', href: '/org/acme/messages' }]);
  });
  it('keeps redacted parents unlinked and drops the current node', () => {
    expect(screenPath({ title: 'Thread' }, [{ id: 'redacted', label: 'Private project' }, { id: 'thread', label: 'Thread', href: '/thread/id' }], [])).toEqual([{ id: 'redacted', label: 'Private project' }]);
  });
  it('has no breadcrumb on a root page', () => {
    expect(screenPath({ title: 'Acme' }, [{ id: 'org', label: 'Acme', href: '/org/acme' }], [])).toEqual([]);
  });
  it('retains a settings section as parent without adding its own page name', () => {
    expect(screenPath({ title: 'Profile', leaf: true }, [], [{ id: 'settings', label: 'Settings', href: '/settings/profile' }])).toEqual([{ id: 'settings', label: 'Settings', href: '/settings/profile' }]);
  });
});

describe('a view once chosen stays chosen', () => {
  it('keeps the Help page as the only displayed help when it is already the addressed screen', () => {
    const help: ViewDefinition = { name: 'help', title: 'views.help.title', about: 'views.help.about', icon: ACTIONS.help.Icon, screens: 'all', area: 'right', body: () => null };
    expect(offers(help, 'thread', { title: 'Thread' })).toBe(true);
    expect(offers(help, 'page', { title: 'Help', views: { ready: true, help: false } })).toBe(false);
  });
  it('shares a kind choice between things while isolating people and deliberate thing overrides', () => {
    writeViewChoice('reader-a', 'topic', 'tab', 'documents');
    expect(readViewChoice('reader-a', 'topic', 'tab', 'overview', 'next')).toBe('documents');
    expect(readViewChoice('reader-b', 'topic', 'tab', 'overview')).toBe('overview');
    writeViewChoice('reader-a', 'topic', 'tab', 'threads', 'one');
    expect(readViewChoice('reader-a', 'topic', 'tab', 'overview', 'one')).toBe('threads');
    expect(readViewChoice('reader-a', 'topic', 'tab', 'overview', 'next')).toBe('documents');
  });
  it('keeps the bar closed across kinds and keeps its width independently', () => {
    writeViewChoice('reader-a', 'global', 'rightOpen', false);
    writeViewChoice('reader-a', 'global', 'rightSize', 501);
    expect(readViewChoice('reader-a', 'global', 'rightOpen', true)).toBe(false);
    expect(readViewChoice('reader-a', 'global', 'rightSize', 400)).toBe(501);
  });
  it('names a kind or thing choice for device controls without exposing another person or a thing identifier', () => {
    expect(viewChoiceKeyDetails('a', 'orch.a.choices.topic.tab')).toEqual({ kind: 'topic', key: 'tab' });
    expect(viewChoiceKeyDetails('a', 'orch.a.choices.topic.private-id.tab')).toEqual({ kind: 'topic', key: 'tab' });
    expect(viewChoiceKeyDetails('b', 'orch.a.choices.topic.tab')).toBeNull();
  });
  it('offers neither contextless files nor unreadable contextual content, retaining read-only content', () => {
    const definition: ViewDefinition = { name: 'files', title: 'views.files.title', about: 'views.files.about', icon: ACTIONS.files.Icon, screens: ['topic', 'thread'], area: 'right', body: () => null };
    expect(offers(definition, 'topic', { title: 'Topic' })).toBe(false);
    expect(offers(definition, 'topic', null)).toBe(false);
    expect(offers(definition, 'thread', { title: 'No workspace', views: { ready: true, workspace: false } })).toBe(false);
    expect(offers(definition, 'thread', { title: 'Unavailable', views: { ready: false } })).toBe(false);
    expect(offers(definition, 'thread', { title: 'Reader', views: { ready: true, workspace: true } })).toBe(true);
    expect(offers({ ...definition, implemented: false }, 'thread', null)).toBe(false);
  });
  it('keeps independently targeted file tabs available on another screen', () => {
    const targeted: ViewDefinition = { name: 'file', title: 'files.fileView.title', about: 'files.fileView.about', icon: ACTIONS.files.Icon, screens: ['thread', 'page'], area: 'main', available: () => true, body: () => null };
    expect(offers(targeted, 'page', { title: 'Settings' })).toBe(true);
    expect(offers({ ...targeted, available: () => false }, 'page', { title: 'Settings' })).toBe(false);
    expect(offers(targeted, 'thread', { title: 'Revoked', views: { ready: true, workspace: false } })).toBe(false);
  });
});

describe('every offer of the same view uses its canonical sign', () => {
  it('registers inspector signs from the action map', () => {
    registerInspectorViews();
    for (const name of ['files', 'outline', 'changes', 'git', 'processes', 'resources', 'terminal', 'environment', 'parameters'] as const) {
      expect(viewOf(name)?.icon).toBe(ACTIONS[name].Icon);
    }
  });
});
