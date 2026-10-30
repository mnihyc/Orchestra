/** The pieces of the left bar as they are drawn: a section that folds and says in its heading only what waits inside, the icon of a place in the colour of its kind or the one it was given, and the field that narrows a tree. */
import { act, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { Section } from '@/features/navigation/Section';
import { TreeFilter } from '@/features/navigation/TreeFilter';
import { KIND_COLORS, NodeIcon } from '@/ui/NodeIcon';

import { show } from './support';

afterEach(() => {
  vi.useRealTimers();
});

describe('a section of the left bar', () => {
  const nothing = (): void => undefined;

  it('is folded and unfolded by its heading, which says which of the two it is', async () => {
    const changes: boolean[] = [];
    const view = show(
      <Section title="Starred" folded={false} onFoldedChange={(folded) => changes.push(folded)}>
        <p>Fix login</p>
      </Section>,
    );
    const heading = screen.getByRole('button', { name: 'Starred' });
    expect(heading.getAttribute('aria-expanded')).toBe('true');
    expect(screen.getByText('Fix login')).toBeTruthy();

    await userEvent.click(heading);
    expect(changes).toEqual([true]);
    view.rerender(
      <Section title="Starred" folded onFoldedChange={nothing}>
        <p>Fix login</p>
      </Section>,
    );
    expect(screen.getByRole('button', { name: 'Starred' }).getAttribute('aria-expanded')).toBe('false');
  });

  it('keeps its rows out of reach from the moment it is folded, and out of the page once they have been seen to leave', () => {
    vi.useFakeTimers();
    const view = show(
      <Section title="Recent" folded={false} onFoldedChange={nothing}>
        <a href="/topic/1">Fix login</a>
      </Section>,
    );
    view.rerender(
      <Section title="Recent" folded onFoldedChange={nothing}>
        <a href="/topic/1">Fix login</a>
      </Section>,
    );
    // Still drawn, so that it is seen to close, and nothing in it can be reached.
    expect(view.container.querySelector('a[href="/topic/1"]')?.closest('[inert]')).not.toBeNull();
    act(() => {
      vi.advanceTimersByTime(200);
    });
    expect(view.container.querySelector('a[href="/topic/1"]')).toBeNull();
  });

  it('says in its heading, while it is folded, only what waits inside: a dot for what is unread, and numbers for decisions and mentions', () => {
    const view = show(
      <Section title="Projects" folded onFoldedChange={nothing} needs={{ unread: true, waiting: 2, mentions: 0 }}>
        <p>rows</p>
      </Section>,
    );
    expect(screen.getByRole('img', { name: 'Unread inside' })).toBeTruthy();
    expect(screen.getByText('2 approvals wait inside')).toBeTruthy();

    // A number of mentions says that something is unread already: no dot beside it.
    view.rerender(
      <Section title="Projects" folded onFoldedChange={nothing} needs={{ unread: true, waiting: 0, mentions: 3 }}>
        <p>rows</p>
      </Section>,
    );
    expect(screen.queryByRole('img', { name: 'Unread inside' })).toBeNull();
    expect(screen.getByText('3 mentions of you inside')).toBeTruthy();
    expect(screen.queryByText(/approvals? waits? inside/)).toBeNull();

    // Unfolded, the rows say it themselves and the heading says nothing.
    view.rerender(
      <Section title="Projects" folded={false} onFoldedChange={nothing} needs={{ unread: true, waiting: 2, mentions: 3 }}>
        <p>rows</p>
      </Section>,
    );
    expect(screen.queryByRole('img', { name: 'Unread inside' })).toBeNull();
    expect(screen.queryByText('3 mentions of you inside')).toBeNull();
  });

  it('shows the way to where its tree begins in place of its name, and keeps the name for a screen reader', () => {
    show(
      <Section title="Projects" folded={false} onFoldedChange={nothing} path={<span>Web</span>}>
        <p>rows</p>
      </Section>,
    );
    expect(screen.getByRole('button', { name: 'Projects' })).toBeTruthy();
    expect(screen.getByText('Web')).toBeTruthy();
  });
});

describe('the icon of a place', () => {
  it('is drawn in the colour of its kind, or in the one it was given, and a thread and a document have none', () => {
    const view = show(
      <>
        <NodeIcon kind="project" />
        <NodeIcon kind="folder" color={5} />
        <NodeIcon kind="topic" color={99} />
        <NodeIcon kind="thread" color={5} />
        <NodeIcon kind="document" />
      </>,
    );
    const colour = (kind: string) => view.container.querySelector(`[data-node-icon="${kind}"]`)?.getAttribute('data-node-color');
    expect(colour('project')).toBe(String(KIND_COLORS.project));
    expect(colour('folder')).toBe('5');
    // A number that is none of the twelve is the colour of the kind.
    expect(colour('topic')).toBe(String(KIND_COLORS.topic));
    expect(colour('thread')).toBeNull();
    expect(colour('document')).toBeNull();
  });

  it('is named where it stands alone, and is no part of what a screen reader hears where a name stands beside it', () => {
    const view = show(
      <>
        <NodeIcon kind="topic" done label="topic, done" />
        <NodeIcon kind="org" />
      </>,
    );
    expect(screen.getByRole('img', { name: 'topic, done' })).toBeTruthy();
    expect(view.container.querySelector('[data-node-icon="org"]')?.getAttribute('aria-hidden')).toBe('true');
  });
});

describe('the field that narrows a tree', () => {
  /** The field with what was typed held beside it, as the bar holds it. */
  function Held({ typed, onLeave, onDown }: { typed: string[]; onLeave: () => void; onDown: () => void }) {
    const [value, setValue] = useState('');
    return (
      <TreeFilter
        value={value}
        onChange={(next) => {
          typed.push(next);
          setValue(next);
        }}
        onLeave={onLeave}
        onDown={onDown}
      />
    );
  }

  it('gives what is typed, goes on into what was found by the arrow down and by Enter, and is emptied by Escape, which leaves it, and by its button', async () => {
    const typed: string[] = [];
    let left = 0;
    let down = 0;
    show(<Held typed={typed} onLeave={() => (left += 1)} onDown={() => (down += 1)} />);
    const field = screen.getByRole('searchbox', { name: 'Narrow this tree' });
    // With nothing typed there is nothing to clear.
    expect(screen.queryByRole('button', { name: 'Clear' })).toBeNull();

    fireEvent.change(field, { target: { value: 'fee' } });
    expect(typed).toEqual(['fee']);
    fireEvent.keyDown(field, { key: 'ArrowDown' });
    fireEvent.keyDown(field, { key: 'Enter' });
    expect(down).toBe(2);
    fireEvent.keyDown(field, { key: 'Escape' });
    expect(typed).toEqual(['fee', '']);
    expect(left).toBe(1);

    fireEvent.change(field, { target: { value: 'web' } });
    await userEvent.click(screen.getByRole('button', { name: 'Clear' }));
    expect(typed).toEqual(['fee', '', 'web', '']);
    expect(left).toBe(1);
    expect(screen.queryByRole('button', { name: 'Clear' })).toBeNull();
  });
});
