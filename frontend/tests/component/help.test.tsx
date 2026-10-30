/** The "?" beside a term with one, two and all four of its parts, what a link inside an article does, and what the finder's palette makes of a row, a key and a disabled row. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ArticleView } from '@/features/help/Article';
import { articleInLanguage, makeGuide, readArticle } from '@/help/guide';
import { HelpHint } from '@/ui/HelpHint';
import { Palette, PaletteGroup, PaletteInput, PaletteItem, PaletteList } from '@/ui/Palette';

import { show } from './support';

beforeEach(() => {
  // What the page gives that jsdom does not: the list of the palette measures itself and scrolls its chosen row into view.
  class Observer {
    observe = vi.fn();
    unobserve = vi.fn();
    disconnect = vi.fn();
  }
  vi.stubGlobal('ResizeObserver', Observer);
  Element.prototype.scrollIntoView = () => undefined;
});

describe('a hint beside a term', () => {
  it('has only the part it has: what it is', async () => {
    const user = userEvent.setup();
    show(<HelpHint term="Dates" what="Keeps the search to what was written between two days." />);
    await user.click(screen.getByRole('button', { name: 'Help: Dates' }));

    expect(await screen.findByText('What this is')).toBeVisible();
    expect(screen.getByText(/between two days/)).toBeVisible();
    expect(screen.queryByText('Which one for me')).not.toBeInTheDocument();
    expect(screen.queryByText('Where it is set')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Read more' })).not.toBeInTheDocument();
  });

  it('adds the choice by what the reader wants when there is one', async () => {
    const user = userEvent.setup();
    show(<HelpHint term="How they connect" what="Where the agent runs." which="If the code is on your own computer: Separated." />);
    await user.click(screen.getByRole('button', { name: 'Help: How they connect' }));

    expect(await screen.findByText('Which one for me')).toBeVisible();
    expect(screen.getByText(/Separated/)).toBeVisible();
    expect(screen.queryByText('Where it is set')).not.toBeInTheDocument();
  });

  it('has all four parts, and More leads to the article and closes the card', async () => {
    const user = userEvent.setup();
    const open = vi.fn();
    show(
      <HelpHint
        term="How they connect"
        what="Where the agent runs."
        which="If the code is on your own computer: Separated."
        where={<a href="/org/acme">Decided by Acme (policy)</a>}
        more={{ label: 'Read more', onOpen: open }}
      />,
    );
    await user.click(screen.getByRole('button', { name: 'Help: How they connect' }));

    for (const part of ['What this is', 'Which one for me', 'Where it is set']) expect(await screen.findByText(part)).toBeVisible();
    expect(screen.getByRole('link', { name: 'Decided by Acme (policy)' })).toHaveAttribute('href', '/org/acme');
    await user.click(screen.getByRole('button', { name: 'Read more' }));
    expect(open).toHaveBeenCalledTimes(1);
    expect(screen.queryByText('What this is')).not.toBeInTheDocument();
  });

  it('opens the shared card and returns focus when it is dismissed', async () => {
    const user = userEvent.setup();
    show(<HelpHint term="Dates" what="Keeps the search to what was written between two days." />);
    await user.click(screen.getByRole('button', { name: 'Help: Dates' }));
    expect(await screen.findByRole('dialog', { name: 'Dates' })).toBeVisible();
    await user.keyboard('{Escape}');
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Dates' })).not.toBeInTheDocument());
    await waitFor(() => expect(screen.getByRole('button', { name: 'Help: Dates' })).toHaveFocus());
  });
});

it('opens the requested translated section under its shared help address', () => {
  const article=readArticle('semantic-search','---\nheading_id.索引状态: index-states\n---\n# 语义搜索\n\n## 索引状态\n\n索引正在处理文字。');
  const scroll=vi.spyOn(Element.prototype,'scrollIntoView');
  show(<ArticleView article={article} heading="index-states" onArticle={()=>undefined} onScreen={()=>undefined}/>);
  const target=screen.getByRole('heading',{name:'索引状态'});
  expect(target).toHaveAttribute('id','index-states');
  expect(scroll).toHaveBeenCalled();
  expect(scroll.mock.instances).toContain(target);
});

describe('an article', () => {
  const article = readArticle('finding-things', ['# Finding things', '', '## Search', '', 'Words. See [the finder](finding-things.md#the-finder), [the inbox](/inbox) and [the site](https://example.org).', '', '## The finder', '', 'More.'].join('\n'));

  it('stays in the help for a link to another article, opens a screen for a link to one, and leaves a link to a site to the browser', async () => {
    const user = userEvent.setup();
    const onArticle = vi.fn();
    const onScreen = vi.fn();
    show(<ArticleView article={article} heading={null} onArticle={onArticle} onScreen={onScreen} />);

    await user.click(screen.getByRole('link', { name: 'the finder' }));
    expect(onArticle).toHaveBeenCalledWith('finding-things', 'the-finder');
    await user.click(screen.getByRole('link', { name: 'the inbox' }));
    expect(onScreen).toHaveBeenCalledWith('/inbox');
    expect(screen.getByRole('link', { name: 'the site' })).toHaveAttribute('target', '_blank');
    expect(onArticle).toHaveBeenCalledTimes(1);
    expect(onScreen).toHaveBeenCalledTimes(1);
  });

  it('gives each heading its anchor and is a region the keyboard can scroll', () => {
    show(<ArticleView article={article} heading="the-finder" onArticle={() => undefined} onScreen={() => undefined} />);
    expect(screen.getByRole('heading', { name: 'The finder' })).toHaveAttribute('id', 'the-finder');
    expect(screen.getByRole('heading', { name: 'Search' })).toHaveAttribute('id', 'search');
    expect(screen.getByRole('region', { name: 'Finding things' })).toHaveAttribute('tabindex', '0');
  });

  it('opens a screen destination at the authored Chinese heading', () => {
    const translated = readArticle('plans', '---\ntitle: 制定计划\nheading_id.回应计划: decide-the-plan\n---\n# 制定计划\n\n## 回应计划\n\n阅读后再继续执行。');
    const scroll = vi.spyOn(Element.prototype, 'scrollIntoView');
    show(<ArticleView article={translated} heading="decide-the-plan" onArticle={() => undefined} onScreen={() => undefined} />);
    const section = screen.getByRole('heading', { name: '回应计划' });
    expect(section).toHaveAttribute('id', 'decide-the-plan');
    expect(scroll).toHaveBeenCalled();
    expect(scroll.mock.instances).toContain(section);
  });

  it.each(['en', 'zh-CN'])('keeps a saved Chinese section link readable in %s', (language) => {
    const source = makeGuide({ 'plans.md': '# Plans\n\n## Decide the plan\n\nRead before proceeding.' }, {}, {
      'zh-CN': { 'plans.md': '---\ntitle: 制定计划\nheading_id.回应计划: decide-the-plan\n---\n# 制定计划\n\n## 回应计划\n\n阅读后再继续执行。' },
    }).articles[0];
    if (source === undefined) throw new Error('the bilingual plan article is required');
    const translated = articleInLanguage(source, language);
    const scroll = vi.spyOn(Element.prototype, 'scrollIntoView');
    show(<ArticleView article={translated} heading="回应计划" onArticle={() => undefined} onScreen={() => undefined} />);
    const section = screen.getByRole('heading', { name: language === 'en' ? 'Decide the plan' : '回应计划' });
    expect(section).toHaveAttribute('id', 'decide-the-plan');
    expect(scroll.mock.instances).toContain(section);
  });
});

describe('the palette of the finder', () => {
  function Rows({ onKey, inputLabel }: { onKey?: (key: string) => void; inputLabel?: string }) {
    return (
      <Palette open onOpenChange={() => undefined} label="Find" {...(inputLabel === undefined ? {} : { inputLabel })} value="b" onValueChange={() => undefined} {...(onKey === undefined ? {} : { onKeyDown: (event) => onKey(event.key) })}>
        <PaletteInput value="" onValueChange={() => undefined} placeholder="Find anything" />
        <PaletteList label="Results">
          <PaletteGroup heading="Go to">
            <PaletteItem value="a" onSelect={() => undefined}>
              First
            </PaletteItem>
            <PaletteItem value="b" onSelect={() => undefined}>
              Second
            </PaletteItem>
            <PaletteItem value="c" disabled>
              Third, that may not be used
            </PaletteItem>
          </PaletteGroup>
        </PaletteList>
      </Palette>
    );
  }

  it('is a combobox over a list of options, one chosen, and one that is switched off and says so', () => {
    show(<Rows inputLabel="Search terms" />);
    expect(screen.getByRole('dialog', { name: 'Find' })).toBeVisible();
    const field = screen.getByRole('combobox', { name: 'Search terms' });
    const list = screen.getByRole('listbox', { name: 'Results' });
    expect(field).toHaveFocus();
    expect(screen.queryByRole('combobox', { name: 'Find' })).not.toBeInTheDocument();
    expect(list).toBeVisible();
    expect(field).toHaveAttribute('aria-controls', list.id);
    expect(screen.getByRole('option', { name: 'Second' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('option', { name: /Third/ })).toHaveAttribute('aria-disabled', 'true');
  });

  it('shows a key to whoever listens before the list takes it', async () => {
    const user = userEvent.setup();
    const seen = vi.fn();
    show(<Rows onKey={seen} />);
    // Callers that omit the optional field name retain the dialog label as their field name.
    expect(screen.getByRole('combobox', { name: 'Find' })).toHaveFocus();
    await user.keyboard('{ArrowDown}');
    expect(seen).toHaveBeenCalledWith('ArrowDown');
  });
});
