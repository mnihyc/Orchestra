/** One prompt, a separate help view, retained deep drafts, and native focus on return. */
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { HelpOverlayHost, HelpWork } from '@/features/help/HelpOverlay';
import { HelpContent } from '@/features/help/HelpView';
import { useHelpOpener } from '@/features/help/open';
import { useHelpState } from '@/features/help/state';
import { readArticle } from '@/help/guide';
import { i18n, setLocale } from '@/i18n';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { Drawer } from '@/ui/Drawer';
import { HelpHint } from '@/ui/HelpHint';
import { Input } from '@/ui/Input';
import { Popover } from '@/ui/Popover';
import { PromptScope, usePrompt } from '@/ui/PromptScope';
import { toast } from '@/ui/Toast';
import { show } from './support';

const scene = vi.hoisted(() => ({ shape: 'wide', path: '/topic/current', push: vi.fn() }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<object>(), useRouter: () => ({ state: { location: { pathname: scene.path } }, history: { push: scene.push } }), useRouterState: () => undefined, useMatches: () => '/topic/$topicId/', useLocation: () => ({ pathname: scene.path, search: {}, searchStr: '', hash: '' }), useNavigate: () => (options: { href: string }) => { scene.push(options.href); return Promise.resolve(); } }));
vi.mock('@/features/workbench/Narrow', () => ({ useShape: () => scene.shape }));
vi.mock('@/help/localized', () => ({ useLocalizedGuide: () => ({ articles: [readArticle('draft-help', '# Draft help\n\nA short answer.\n\n## Next\n\nYour draft is kept.')] }) }));
vi.mock('@/features/help/Tutorial', () => ({ TutorialList: () => null }));
vi.mock('@/features/help/SourceJump', () => ({ SettingSourceJumpHost: () => null }));

function Fields() {
  const openHelp = useHelpOpener();
  const [draft, setDraft] = useState('');
  const [confirm, setConfirm] = useState(false);
  return <>
    <Input label="Draft" value={draft} onChange={(event) => setDraft(event.target.value)} />
    <HelpHint term="Draft" what="A short answer." more={{ label: i18n.t('help.hint.more'), onOpen: () => openHelp('draft-help', 'next') }} />
    <Popover label="Earlier hint" trigger={<button>Details</button>}><button onClick={() => setConfirm(true)}>Review action</button></Popover>
    <ConfirmDialog open={confirm} onOpenChange={setConfirm} title="Latest prompt" confirmLabel="Do it" onConfirm={() => setConfirm(false)}>Check this action.</ConfirmDialog>
  </>;
}

function Work() {
  const [open, setOpen] = useState(false);
  return <><Button onClick={() => setOpen(true)}>Open setup</Button><Dialog open={open} onOpenChange={setOpen} title="Setup"><Fields /></Dialog></>;
}

beforeEach(async () => {
  await setLocale('en');
  scene.push.mockClear();
  scene.shape = 'wide';
  scene.path = '/topic/current';
  Element.prototype.scrollIntoView = () => undefined;
  useHelpState.setState({ overlay: false, readingRequest: 0, article: null, heading: null, query: '', settingContext: null, guideContext: null });
});
afterEach(async () => { act(() => useHelpState.setState({ overlay: false })); document.documentElement.removeAttribute('data-theme'); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const shape of ['wide', 'small'] as const) {
  it(`reads help without another dialog or navigation, then restores the deep draft (${locale}, ${theme}, ${shape})`, async () => {
    await setLocale(locale); scene.shape = shape; document.documentElement.dataset.theme = theme;
    const user = userEvent.setup();
    show(<><HelpOverlayHost /><HelpWork><Work /></HelpWork></>);
    const opener = screen.getByRole('button', { name: 'Open setup' });
    await user.click(opener);
    await user.type(await screen.findByRole('textbox', { name: 'Draft' }), 'Unsent changes');
    await user.click(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: 'Draft' }) }));
    await user.click(await screen.findByRole('button', { name: i18n.t('help.hint.more') }));
    const help = await screen.findByRole('main', { name: i18n.t('views.help.title') });
    await waitFor(() => expect(within(help).getByRole('heading', { name: 'Draft help' })).toBeVisible());
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect(scene.push).not.toHaveBeenCalled();
    await user.click(within(help).getByRole('button', { name: i18n.t('helpFlow.back') }));
    const returned = await screen.findByRole('dialog', { name: 'Setup' });
    expect(within(returned).getByRole('textbox', { name: 'Draft' })).toHaveValue('Unsent changes');
    await user.keyboard('{Escape}');
    await waitFor(() => expect(opener).toHaveFocus());
  });
}

it('puts a nested confirmation above its form and stale popover, then restores the form without losing its draft', async () => {
  const user = userEvent.setup();
  show(<Work />);
  await user.click(screen.getByRole('button', { name: 'Open setup' }));
  await user.type(await screen.findByRole('textbox', { name: 'Draft' }), 'Keep me');
  await user.click(screen.getByRole('button', { name: 'Details' }));
  await user.click(await screen.findByRole('button', { name: 'Review action' }));
  const newest = await screen.findByRole('alertdialog', { name: 'Latest prompt' });
  expect(newest).toBeVisible();
  expect(screen.queryByRole('dialog', { name: 'Setup' })).toBeNull();
  expect(screen.queryByRole('dialog', { name: 'Earlier hint' })).toBeNull();
  await user.click(within(newest).getByRole('button', { name: 'Cancel' }));
  expect(await screen.findByRole('textbox', { name: 'Draft' })).toHaveValue('Keep me');
  expect(screen.getAllByRole('dialog')).toHaveLength(1);
});

it('can ask from a prompt on the Help page itself without a duplicate Help heading or route change', async () => {
  scene.path = '/help/draft-help';
  const user = userEvent.setup();
  show(<><HelpOverlayHost /><HelpWork><HelpContent page article="draft-help" /><Work /></HelpWork></>);
  await user.click(screen.getByRole('button', { name: 'Open setup' }));
  await user.type(await screen.findByRole('textbox', { name: 'Draft' }), 'On the Help page');
  await user.click(screen.getByRole('button', { name: 'Help: Draft' }));
  await user.click(await screen.findByRole('button', { name: i18n.t('help.hint.more') }));
  const reading = await screen.findByRole('main', { name: i18n.t('views.help.title') });
  expect(within(reading).getAllByRole('heading', { level: 1 })).toHaveLength(1);
  expect(within(reading).getByRole('heading', { level: 1 })).toHaveTextContent('Draft help');
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(scene.push).not.toHaveBeenCalled();
  await user.keyboard('{Escape}');
  expect(await screen.findByRole('textbox', { name: 'Draft' })).toHaveValue('On the Help page');
});

it('keeps a confirmation mounted inside its popover until it is answered', async () => {
  function Card() {
    const [open, setOpen] = useState(false);
    return <><button onClick={() => setOpen(true)}>Review nested action</button><ConfirmDialog open={open} onOpenChange={setOpen} title="Nested action" confirmLabel="Apply" onConfirm={() => setOpen(false)}>Answer before returning.</ConfirmDialog></>;
  }
  show(<Dialog open onOpenChange={() => undefined} title="Original form"><Popover label="Action card" trigger={<button>Open card</button>}><Card /></Popover></Dialog>);
  await userEvent.click(await screen.findByRole('button', { name: 'Open card' }));
  await userEvent.click(await screen.findByRole('button', { name: 'Review nested action' }));
  const confirmation = await screen.findByRole('alertdialog', { name: 'Nested action' });
  expect(confirmation).toBeVisible();
  expect(screen.queryByRole('dialog', { name: 'Action card' })).toBeNull();
  await userEvent.click(within(confirmation).getByRole('button', { name: 'Cancel' }));
  await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Action card' })).toBeNull());
  expect(await screen.findByRole('button', { name: 'Open card' })).toBeVisible();
});

it('keeps a newer prompt on top when an older loading presenter becomes its loaded form', async () => {
  function Loading() {
    return <Dialog open onOpenChange={() => undefined} title="Loading setup">Reading choices.</Dialog>;
  }
  function Loaded() {
    const [draft, setDraft] = useState('');
    return <Dialog open onOpenChange={() => undefined} title="Loaded setup"><Input label="Loaded draft" value={draft} onChange={(event) => setDraft(event.target.value)} /></Dialog>;
  }
  function Presenter({ ready }: { ready: boolean }) {
    const layer = usePrompt(true, 'scope');
    return <PromptScope layer={layer}>{ready ? <Loaded /> : <Loading />}</PromptScope>;
  }
  function Scene({ ready, next }: { ready: boolean; next: boolean }) {
    return <><Presenter ready={ready} /><Dialog open={next} onOpenChange={() => undefined} title="Newer question"><Input label="Newer draft" /></Dialog></>;
  }
  const rendered = show(<Scene ready={false} next={false} />);
  expect(await screen.findByRole('dialog', { name: 'Loading setup' })).toBeVisible();
  rendered.rerender(<Scene ready={false} next />);
  await userEvent.type(await screen.findByRole('textbox', { name: 'Newer draft' }), 'Keep the newer draft');
  rendered.rerender(<Scene ready next />);
  expect(screen.getByRole('dialog', { name: 'Newer question' })).toBeVisible();
  expect(screen.getByRole('textbox', { name: 'Newer draft' })).toHaveValue('Keep the newer draft');
  expect(screen.queryByRole('dialog', { name: 'Loaded setup' })).toBeNull();
  rendered.rerender(<Scene ready next={false} />);
  expect(await screen.findByRole('textbox', { name: 'Loaded draft' })).toBeVisible();
  expect(screen.getAllByRole('dialog')).toHaveLength(1);
});

it('passes a form’s ownership through a layout sheet without hiding its newer child prompt', async () => {
  function Layout() {
    const [viewOpen, setViewOpen] = useState(false);
    const [confirm, setConfirm] = useState(false);
    return <Dialog open onOpenChange={() => undefined} title="Outer form"><button onClick={() => setViewOpen(true)}>Open layout view</button><Drawer prompt={false} modal={false} open={viewOpen} onOpenChange={setViewOpen} title="Layout view"><button onClick={() => setConfirm(true)}>Review in view</button><ConfirmDialog open={confirm} onOpenChange={setConfirm} title="View action" confirmLabel="Apply" onConfirm={() => setConfirm(false)}>Keep the form alive.</ConfirmDialog></Drawer></Dialog>;
  }
  show(<Layout />);
  await userEvent.click(await screen.findByRole('button', { name: 'Open layout view' }));
  await userEvent.click(await screen.findByRole('button', { name: 'Review in view' }));
  const newest = await screen.findByRole('alertdialog', { name: 'View action' });
  expect(newest).toBeVisible();
  expect(screen.queryByRole('dialog', { name: 'Outer form' })).toBeNull();
  expect(screen.queryByRole('dialog', { name: 'Layout view' })).toBeNull();
  await userEvent.click(within(newest).getByRole('button', { name: 'Cancel' }));
  expect(await screen.findByRole('button', { name: 'Review in view' })).toBeVisible();
});

it('keeps an already open Help page and a background toast out of a newer prompt, then restores both', async () => {
  const user = userEvent.setup();
  useHelpState.setState({ article: 'draft-help' });
  const dismiss = toast({ tone: 'danger', title: 'Background failure' });
  try {
    show(<><HelpContent page article="draft-help" /><Work /></>);
    expect(await screen.findByRole('heading', { name: 'Draft help' })).toBeVisible();
    const notice = await screen.findByText('Background failure');
    await user.click(screen.getByRole('button', { name: 'Open setup' }));
    expect(await screen.findByRole('dialog', { name: 'Setup' })).toBeVisible();
    expect(screen.getByText('Draft help', { selector: 'h1' })).not.toBeVisible();
    expect(notice).not.toBeVisible();
    await user.keyboard('{Escape}');
    await waitFor(() => expect(screen.getByRole('heading', { name: 'Draft help' })).toBeVisible());
    expect(await screen.findByText('Background failure')).toBeVisible();
  } finally { act(() => dismiss()); }
});

it('lets a global prompt interrupt reading, retains article scroll and focus, and defers its background notice', async () => {
  function Question() {
    const [open, setOpen] = useState(false);
    return <><button onClick={() => setOpen(true)}>Open global question</button><Dialog open={open} onOpenChange={setOpen} title="Global question">A newer prompt.</Dialog></>;
  }
  const dismiss = toast({ tone: 'danger', title: 'Queued while reading' });
  try {
    show(<><HelpOverlayHost /><Question /><HelpWork><span>Work page</span></HelpWork></>);
    const notice = await screen.findByText('Queued while reading');
    act(() => useHelpState.setState({ overlay: true, article: 'draft-help' }));
    const article = await screen.findByRole('region', { name: 'Draft help' });
    article.scrollTop = 240;
    article.focus();
    expect(notice).not.toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: 'Open global question' }));
    const question = await screen.findByRole('dialog', { name: 'Global question' });
    expect(screen.queryByRole('main', { name: i18n.t('views.help.title') })).toBeNull();
    expect(notice).not.toBeVisible();
    await userEvent.click(within(question).getByRole('button', { name: 'Close' }));
    expect(await screen.findByRole('region', { name: 'Draft help' })).toBeVisible();
    await waitFor(() => expect(article.scrollTop).toBe(240));
    expect(article).toHaveFocus();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('helpFlow.back') }));
    expect(await screen.findByText('Queued while reading')).toBeVisible();
  } finally { act(() => dismiss()); }
});

it('brings Help back above a newer global form and returns to that form’s deep draft', async () => {
  function QuestionFields() {
    const [draft, setDraft] = useState('');
    const help = useHelpOpener();
    return <><Input label="Question draft" value={draft} onChange={(event) => setDraft(event.target.value)} /><button onClick={() => help('draft-help')}>Help from question</button></>;
  }
  function Question() {
    const [open, setOpen] = useState(false);
    return <><button onClick={() => setOpen(true)}>Open another form</button><Dialog open={open} onOpenChange={setOpen} title="Another form"><QuestionFields /></Dialog></>;
  }
  show(<><HelpOverlayHost /><Question /><HelpWork><Work /></HelpWork></>);
  await userEvent.click(screen.getByRole('button', { name: 'Open setup' }));
  await userEvent.type(await screen.findByRole('textbox', { name: 'Draft' }), 'Keep the original draft');
  await userEvent.click(screen.getByRole('button', { name: 'Help: Draft' }));
  await userEvent.click(await screen.findByRole('button', { name: i18n.t('help.hint.more') }));
  expect(await screen.findByRole('region', { name: 'Draft help' })).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: 'Open another form' }));
  await userEvent.type(await screen.findByRole('textbox', { name: 'Question draft' }), 'Keep the second draft');
  await userEvent.click(screen.getByRole('button', { name: 'Help from question' }));
  expect(await screen.findByRole('main', { name: i18n.t('views.help.title') })).toBeVisible();
  expect(screen.queryByRole('dialog', { name: 'Another form' })).toBeNull();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('helpFlow.back') }));
  expect(await screen.findByRole('textbox', { name: 'Question draft' })).toHaveValue('Keep the second draft');
  expect(screen.queryByRole('dialog', { name: 'Setup' })).toBeNull();
  await userEvent.click(within(screen.getByRole('dialog', { name: 'Another form' })).getByRole('button', { name: 'Close' }));
  expect(await screen.findByRole('textbox', { name: 'Draft' })).toHaveValue('Keep the original draft');
});
