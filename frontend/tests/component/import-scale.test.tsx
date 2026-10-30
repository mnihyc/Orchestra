/** Large local selections and source-person lists draw one page and keep its neighbours reachable. */
import { fireEvent, screen, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';

import { ImportPeople } from '@/features/people/ImportPeople';
import { ImportReasons } from '@/features/people/ImportReasons';
import { ImportSourceStep } from '@/features/people/ImportSourceStep';
import { show } from './support';

it('draws 50 selected names from a 100,000-file source, with bounded next and first pages', () => {
  const file = new File(['x'], 'source.md');
  const files = Array.from({ length: 100_000 }, (_, index) => ({ file, path: `Notes/${index}.md` }));
  show(<ImportSourceStep value={{ fault: null, files, bytes: files.length }} onChange={vi.fn()} />);
  const selected = screen.getByRole('list', { name: 'Selected files' });
  expect(within(selected).getAllByRole('listitem')).toHaveLength(50);
  expect(screen.getByText('Notes/0.md')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Next page' }));
  expect(within(selected).getAllByRole('listitem')).toHaveLength(50);
  expect(screen.getByText('Notes/50.md')).toBeVisible();
  expect(screen.queryByText('Notes/0.md')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Back to first page' }));
  expect(screen.getByText('Notes/0.md')).toBeVisible();
  expect(screen.getByText(/^100,000 files ·/)).toBeVisible();
});

it('draws 50 people from 100,000 source people without mounting their unopened matches', () => {
  const people = Array.from({ length: 100_000 }, (_, index) => ({ principal: `person-${index}`, username: `user-${index}`, display_name: `Person ${index}`, matched: 'none' as const, user: null, objects: 1 }));
  show(<ImportPeople people={people} authors="labels" mayAttribute onAuthors={vi.fn()} onPerson={vi.fn()} />);
  const table = screen.getByRole('table', { name: 'People' });
  expect(within(table).getAllByRole('row')).toHaveLength(51);
  expect(screen.getByText('Person 0')).toBeVisible();
  expect(screen.queryByText('Person 50')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Next page' }));
  expect(within(table).getAllByRole('row')).toHaveLength(51);
  expect(screen.getByText('Person 50')).toBeVisible();
  expect(screen.queryByText('Person 0')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Back to first page' }));
  expect(screen.getByText('Person 0')).toBeVisible();
});

it('draws 50 reasons from 100,000 losses or warnings and keeps their counts and pages', () => {
  const reasons = Array.from({ length: 100_000 }, (_, index) => ({ code: 'future_warning', message: `Reason ${index}`, params: { count: 2 } }));
  show(<ImportReasons reasons={reasons} label="Warnings" />);
  const list = screen.getByRole('list', { name: 'Warnings' });
  expect(within(list).getAllByRole('listitem')).toHaveLength(50);
  expect(screen.getByText('Reason 0 · 2')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Next page' }));
  expect(within(list).getAllByRole('listitem')).toHaveLength(50);
  expect(screen.getByText('Reason 50 · 2')).toBeVisible();
  expect(screen.queryByText('Reason 0 · 2')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Back to first page' }));
  expect(screen.getByText('Reason 0 · 2')).toBeVisible();
});
