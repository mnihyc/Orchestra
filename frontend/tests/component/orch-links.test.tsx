/** Markdown navigates declared aliases locally, while external look-alikes remain external. */
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { queryClient } from '@/api/query';
import { Markdown } from '@/ui/Markdown';

afterEach(() => { cleanup(); queryClient.removeQueries({ queryKey: ['meta'], exact: true }); });

it('rewrites links from a declared public address and keeps all parts', () => {
  queryClient.setQueryData(['meta'], { public_url: 'https://public.orch.test' });
  render(<Markdown text="[Plan](https://public.orch.test/doc/plan?block=b#part) and [Outside](https://public.orch.test.evil.test/doc/plan)" />);
  expect(screen.getByRole('link', { name: 'Plan' })).toHaveAttribute('href', '/doc/plan?block=b#part');
  expect(screen.getByRole('link', { name: 'Plan' })).not.toHaveAttribute('target');
  expect(screen.getByRole('link', { name: 'Outside' })).toHaveAttribute('href', 'https://public.orch.test.evil.test/doc/plan');
  expect(screen.getByRole('link', { name: 'Outside' })).toHaveAttribute('target', '_blank');
});

it('does not follow a backslash-disguised authority as a local link', () => {
  render(<Markdown text={'[Disguised](/\\\\evil.test/doc/source)'} />);
  expect(screen.queryByRole('link', { name: 'Disguised' })).not.toBeInTheDocument();
});
