/** Act wording and signs are shared, and the title bar's visible and folded places are links. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { KeyRound } from 'lucide-react';
import { describe, expect, it, vi } from 'vitest';

import { client } from '@/api/client';
import { HelpContent } from '@/features/help/HelpView';
import { useHelpState } from '@/features/help/state';
import { readViewChoice, writeViewChoice } from '@/lib/viewChoices';
import { ExportStatus } from '@/features/export/ExportStatus';
import { useWatched } from '@/features/export/jobs';
import { i18n, setLocale, type TranslationKey } from '@/i18n';
import { localizedGuide } from '@/help/localized';
import type * as GuideNames from '@/help/localized';
import { readArticle } from '@/help/guide';
import { ACTIONS, actionForLabel, actionName, type ActionId } from '@/ui/actions';
import { Breadcrumbs } from '@/ui/Breadcrumbs';
import { Button } from '@/ui/Button';
import { IconButton } from '@/ui/IconButton';
import { show as render } from './support';

vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => vi.fn(),
  useLocation: () => ({ pathname: '/search', search: {}, searchStr: '', hash: '' }),
  useMatches: () => '/search/',
  useRouterState: () => undefined,
  useRouter: () => ({ state: { location: { pathname: '/search' } }, history: { push: vi.fn() } }),
}));
vi.mock('@/help/localized', async (importOriginal) => {
  const original = await importOriginal<typeof GuideNames>();
  return { ...original, useLocalizedGuide: () => ({ articles: [readArticle('finding-things', '# Finding things\n\nA real article about finding things.')] }) };
});
vi.mock('@/features/help/Dismissed', () => ({ useDismissed: () => ({ isClosed: () => false, isFinished: () => false, close: vi.fn(), finish: vi.fn(), reopenAll: vi.fn(), stepOf: () => 0, keepStep: vi.fn() }) }));

/** This audit renders 180 translated commands; the shared builder may spend seconds on accessibility names. */
const COMMAND_REVIEW_BUDGET_MS = 20_000;

describe('a button says what pressing it does', () => {
  it('gives model selection, turn selection, tags and the two chat searches their own signs', () => {
    expect(ACTIONS.models.Icon).not.toBe(ACTIONS.selectTurn.Icon);
    expect(ACTIONS.searchHistory.Icon).not.toBe(ACTIONS.search.Icon);
    render(<><Button>{i18n.t('places.topic.addTags')}</Button><Button>{i18n.t('places.topic.editTags')}</Button><IconButton action="searchHistory" label={i18n.t('screenReview.searchChatHistory')} /></>);
    expect(screen.getByRole('button', { name: i18n.t('places.topic.addTags') }).querySelector('[data-action-icon="tags"]')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: i18n.t('places.topic.editTags') }).querySelector('[data-action-icon="tags"]')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Search chat history' }).querySelector('[data-action-icon="searchHistory"]')).toBeInTheDocument();
  });

  it('keeps transient export progress and failures plain beside a named signed navigation act', () => {
    useWatched.setState({ items: [
      { job: 'running', place: 'Web', state: 'running', percent: 42, phase: 'writing', exportId: null, error: null },
      { job: 'failed', place: 'Shop', state: 'failed', percent: 0, phase: '', exportId: null, error: null },
    ] });
    try {
      render(<ExportStatus />);
      expect(screen.getByText('Exporting Web · 42%').closest('button')).toBeNull();
      expect(screen.getByText('Export failed').closest('button')).toBeNull();
      expect(screen.getByRole('button', { name: 'Your exports' }).querySelector('[data-action-icon="yourExports"]')).toBeInTheDocument();
    } finally { useWatched.setState({ items: [] }); }
  });
  it('never treats a state name as an act and gives privacy directions explicit verbs in both languages', async () => {
    for (const locale of ['en', 'zh-CN'] as const) {
      await setLocale(locale);
      expect(actionForLabel(i18n.t('state.blocked'))).toBeUndefined();
      expect(actionForLabel(i18n.t('craft.private'))).toBeUndefined();
      expect(actionName('makePrivate')).not.toBe(i18n.t('craft.private'));
      expect(actionName('makePrivate')).not.toBe(actionName('inheritAccess'));
    }
    await setLocale('en');
  });
});

describe('the same act carries the same sign', () => {
  it('takes the Move sign from the catalogue even when an older caller supplies a different icon', () => {
    render(<><Button action="move" icon={<KeyRound />}>Move</Button><IconButton action="move" label={actionName('move', 'Fee rules')} /></>);
    for (const button of screen.getAllByRole('button')) {
      expect(button.querySelector('[data-action-icon="move"]')).toBeInTheDocument();
      expect(button.querySelector('.lucide-key-round')).toBeNull();
    }
    expect(screen.getByRole('button', { name: 'Move Fee rules' })).toHaveAccessibleName();
    expect(ACTIONS.rename.Icon).toBe(ACTIONS.edit.Icon);
    expect(ACTIONS.cancel.Icon).toBe(ACTIONS.dismiss.Icon);
    expect(ACTIONS.newShell.Icon).toBe(ACTIONS.create.Icon);
  });
  it('adds the dismissal sign to an existing named Cancel caller automatically', () => {
    render(<Button>{i18n.t('common.cancel')}</Button>);
    expect(screen.getByRole('button', { name: 'Cancel' }).querySelector('[data-action-icon="cancel"]')).toBeInTheDocument();
  });
  it('keeps a named link a real link and gives its known sign through Button', () => {
    render(<Button asChild><a href="/login">Sign in</a></Button>);
    expect(screen.getByRole('link', { name: 'Sign in' })).toHaveAttribute('href', '/login');
    expect(screen.getByRole('link', { name: 'Sign in' }).querySelector('[data-action-icon="signIn"]')).toBeInTheDocument();
  });
  it('keeps deleted-item restoration, initial input and a new human conversation in their actual families', async () => {
    for (const locale of ['en', 'zh-CN'] as const) {
      await setLocale(locale);
      expect(actionForLabel(i18n.t('documents.list.restore'))).toBe('restore');
      expect(actionForLabel(i18n.t('start.quick.send'))).toBe('send');
      expect(actionForLabel(i18n.t('chat.new.direct', { name: 'Vera' }))).toBe('newMessage');
      expect(actionForLabel(i18n.t('security.password.change'))).toBe('setPassword');
      expect(actionForLabel(i18n.t('profile.address.changeButton'))).toBe('edit');
      expect(actionForLabel(i18n.t('settings.service.tokensButton'))).toBe('tokens');
      expect(actionForLabel(i18n.t('places.org.newProject'))).toBe('create');
      expect(actionForLabel(i18n.t('places.menu.newDepartment'))).toBe('create');
    }
    await setLocale('en');
  });
  it('gives the reviewed creation, navigation and editing commands their real signs in both languages', async () => {
    const commands: readonly [TranslationKey, ActionId][] = [
      ['search.find.command', 'search'],
      ['ports.title', 'ports'],
      ['craftJoined.showHelper', 'portHelper'],
      ['ports.preview', 'show'],
      ['ports.confirmPlace', 'allow'],
      ['ports.refresh', 'refresh'],
      ['ports.saveProgram', 'save'],
      ['ports.cancel', 'cancel'],
      ['export.document.openToPrint', 'openDocument'],
      ['trace.restore', 'restore'],
      ['trace.inspect', 'show'],
      ['trace.entry', 'history'],
      ['navigation.view.collapseAll', 'collapseAll'],
      ['approvals.review', 'show'],
      ['access.agents.share', 'shareAgents'],
      ['access.agents.dialog.button.none', 'shareAgents'],
      ['providers.models.switchOn', 'enable'],
      ['access.grid.back', 'backPermissions'],
      ['help.articles.back', 'back'],
      ['settings.secrets.write', 'save'],
      ['settings.secrets.replace', 'save'],
      ['access.outsiders.inviteSend', 'invite'],
      ['places.invite.submit', 'invite'],
      ['places.invite.another', 'invite'],
      ['access.reaches.open', 'open'],
      ['finder.card.openPage', 'open'],
      ['hosts.add.open', 'open'],
      ['machines.add.open', 'open'],
      ['machines.provider.open', 'open'],
      ['providers.add.open', 'open'],
      ['notifications.connect.open', 'open'],
      ['access.roles.change', 'defaults'],
      ['settings.roles.defaults', 'defaults'],
      ['access.stale.again', 'retry'],
      ['approvals.sendAnswers', 'send'],
      ['export.request.send', 'send'],
      ['settings.channels.sendTest', 'send'],
      ['settings.channels.sendTestAgain', 'send'],
      ['notifications.test.send', 'send'],
      ['start.request.send', 'send'],
      ['ui.limit.sendAndWait', 'send'],
      ['auth.verify.resend', 'sendLink'],
      ['profile.address.resend', 'sendLink'],
      ['notifications.need.resend', 'sendLink'],
      ['chat.group.addTitle', 'addPeople'],
      ['chat.group.renameButton', 'save'],
      ['agents.profile.save', 'save'],
      ['hosts.settings.save', 'save'],
      ['places.members.saveRole', 'save'],
      ['dev.sample.save', 'save'],
      ['documents.comments.reopen', 'reopen'],
      ['documents.comments.resolve', 'resolve'],
      ['documents.list.newPrivate', 'createPrivateDocument'],
      ['documents.move.confirm', 'move'],
      ['places.move.confirm', 'move'],
      ['settings.members.move.submit', 'move'],
      ['export.list.downloadOf', 'download'],
      ['files.none.openAsText', 'openText'],
      ['finder.card.back', 'back'],
      ['providers.add.back', 'back'],
      ['settings.org.back', 'back'],
      ['inspector.environment.idle.change', 'edit'],
      ['settings.channels.editNamed', 'edit'],
      ['thread.description.edit', 'edit'],
      ['places.topic.addTags', 'tags'],
      ['places.topic.editTags', 'tags'],
      ['terminal.closed.openNew', 'newShell'],
      ['terminal.strip.new', 'newShell'],
      ['agents.add.submit', 'create'],
      ['agents.list.add', 'create'],
      ['environments.make.submit', 'create'],
      ['machines.list.add', 'create'],
      ['resources.endpoint.add', 'create'],
      ['templates.list.add', 'create'],
      ['settings.secrets.add', 'create'],
      ['members.groups.make', 'create'],
      ['settings.members.closedAction', 'create'],
      ['settings.service.make', 'create'],
      ['tags.make', 'create'],
      ['notifications.connect.again', 'create'],
      ['profile.address.add', 'create'],
      ['notifications.webhook.add', 'create'],
      ['notifications.webhook.addConfirm', 'create'],
      ['start.where.makeOne', 'create'],
      ['resources.fieldForm.add', 'create'],
      ['environments.remove.confirm', 'delete'],
      ['hosts.remove.confirm', 'delete'],
      ['hosts.add.make', 'createToken'],
      ['hosts.add.again', 'createToken'],
      ['machines.add.again', 'createToken'],
      ['settings.service.makeToken', 'createToken'],
      ['providers.add.next', 'next'],
      ['settings.editor.resetNamed', 'inheritSetting'],
      ['settings.editor.reset', 'inheritSetting'],
      ['settings.editor.earlier', 'earlier'],
      ['settings.editor.later', 'later'],
      ['departments.openSettings', 'settings'],
      ['start.create', 'newThread'],
      ['dev.sample.archive', 'archive'],
      ['dev.sample.confirmLabel', 'archive'],
      ['dev.sample.dialog', 'rename'],
    ];
    for (const locale of ['en', 'zh-CN'] as const) {
      await setLocale(locale);
      const labels = commands.map(([key]) => i18n.t(key, { name: 'Review target', to: 'Web', from: 'Acme', place: 'Web', target: 'Web' }));
      const view = render(<>{labels.map((label, index) => <Button key={index}>{label}</Button>)}</>);
      try {
        const buttons = screen.getAllByRole('button');
        for (const [index, [, action]] of commands.entries()) {
          expect(actionForLabel(labels[index])).toBe(action);
          expect(buttons[index]).toHaveAccessibleName(labels[index]);
          expect(buttons[index]?.querySelector(`[data-action-icon="${action}"]`)).toBeInTheDocument();
        }
      } finally { view.unmount(); }
    }
    await setLocale('en');
  }, COMMAND_REVIEW_BUDGET_MS);
  it('keeps specific target-qualified canonical signs ahead of generic translated verbs', async () => {
    for (const locale of ['en', 'zh-CN'] as const) {
      await setLocale(locale);
      for (const [action, definition] of Object.entries(ACTIONS)) {
        const named = actionName(action as ActionId, 'Review target');
        const resolved = actionForLabel(named);
        expect(resolved, named).toBeDefined();
        expect(ACTIONS[resolved!].Icon, named).toBe(definition.Icon);
      }
    }
    await setLocale('en');
  });
  it('names every typed icon action and draws its sign from the same map', () => {
    const ids = Object.keys(ACTIONS) as (keyof typeof ACTIONS)[];
    render(<>{ids.map((action) => <IconButton key={action} action={action} label={actionName(action, action)} />)}</>);
    const buttons = screen.getAllByRole('button');
    expect(buttons).toHaveLength(ids.length);
    for (const [index, action] of ids.entries()) {
      expect(buttons[index]).toHaveAccessibleName(actionName(action, action));
      expect(buttons[index]?.querySelector(`[data-action-icon="${action}"]`)).toBeInTheDocument();
    }
  });
});

describe('guide names are interface wording in both languages', () => {
  it('translates shipped names while preserving authored bodies and stable heading anchors', async () => {
    const guide = { articles: [readArticle('resources', '# Resources and movement\n\n## Getting access\nThe authored guide body.'), readArticle('other', '# Authored name\nUnknown guide.')] };
    for (const [locale, title] of [['en', 'Resources and movement'], ['zh-CN', '资源与移动']] as const) {
      await setLocale(locale);
      const localized = localizedGuide(guide, (key) => i18n.t(key));
      expect(localized.articles[0]?.title).toBe(title);
      expect(localized.articles[0]?.body).toBe(guide.articles[0]?.body);
      expect(localized.articles[0]?.headings).toEqual(guide.articles[0]?.headings);
      expect(localized.articles[1]?.title).toBe('Authored name');
      expect(guide.articles[0]?.title).toBe('Resources and movement');
    }
    await setLocale('en');
  });
});

describe('every permitted step in the title bar is a link', () => {
  it('keeps every ancestor as a full native link without an overflow menu or current self-link', () => {
    const crumbs = ['Acme', 'Shop', 'Checkout', 'Payments'].map((label) => ({ id: label, label, href: `/${label}` }));
    render(<Breadcrumbs crumbs={crumbs} max={3} />);
    for (const crumb of crumbs) expect(screen.getByRole('link', { name: crumb.label })).toHaveAttribute('href', crumb.href);
    expect(screen.queryByRole('link', { current: 'page' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: i18n.t('ui.breadcrumbs.folded') })).not.toBeInTheDocument();
  });
});

describe('All articles names its actual destination', () => {
  it('clears search and selects Articles from a missing article and from an article opened on This screen', async () => {
    // jsdom has no layout observer; actual resize/visibility is checked in the browser cases.
    class Observer { observe = vi.fn(); unobserve = vi.fn(); disconnect = vi.fn(); }
    vi.stubGlobal('ResizeObserver', Observer);
    const userId = 'craft-help-return';
    const session = client.session.getState();
    const help = useHelpState.getState();
    client.session.setState({ userId });
    try {
      for (const locale of ['en', 'zh-CN'] as const) {
        await setLocale(locale);
        writeViewChoice(userId, 'help', 'part', 'screen');
        useHelpState.setState({ article: 'missing-guide', heading: null, query: 'retained search' });
        const view = render(<HelpContent />);
        const back = screen.getByRole('button', { name: i18n.t('help.articles.back') });
        expect(back.querySelector('[data-action-icon="back"]')).toBeInTheDocument();
        await userEvent.click(back);
        expect(screen.getByRole('tab', { name: /^Articles|^文章/ })).toHaveAttribute('aria-selected', 'true');
        expect(screen.getByRole('searchbox')).toHaveValue('');
        expect(readViewChoice(userId, 'help', 'part', 'screen')).toBe('articles');
        await userEvent.click(screen.getByRole('tab', { name: i18n.t('help.parts.screen') }));
        expect(readViewChoice(userId, 'help', 'part', 'articles')).toBe('screen');
        useHelpState.setState({ article: 'finding-things', heading: null });
        const articleBack = await screen.findByRole('button', { name: i18n.t('help.articles.back') });
        expect(articleBack.querySelector('[data-action-icon="back"]')).toBeInTheDocument();
        await userEvent.click(articleBack);
        expect(screen.getByRole('tab', { name: /^Articles|^文章/ })).toHaveAttribute('aria-selected', 'true');
        expect(screen.getByRole('button', { name: /Finding things/ })).toBeVisible();
        view.unmount();
      }
    } finally {
      client.session.setState(session);
      useHelpState.setState(help);
      localStorage.removeItem(`orch.${userId}.choices.help.part`);
      vi.unstubAllGlobals();
      await setLocale('en');
    }
  });
});
