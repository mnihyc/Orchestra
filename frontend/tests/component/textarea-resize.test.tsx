/** The composer's resize-none replaces the ordinary form field's resize-y, independent of CSS order. */
import { screen } from '@testing-library/react';
import { expect, it } from 'vitest';
import { Textarea } from '@/ui/Textarea';
import { show } from './support';

it('keeps ordinary multiline fields resizable and honors a composer override without conflicting utilities', () => {
  const view = show(<Textarea label="Message" />);
  expect(screen.getByRole('textbox', { name: 'Message' })).toHaveClass('resize-y');
  view.rerender(<Textarea label="Message" className="resize-none" />);
  const box = screen.getByRole('textbox', { name: 'Message' });
  expect(box).toHaveClass('resize-none');
  expect(box).not.toHaveClass('resize-y');
  view.rerender(<Textarea label="Message" className="resize-x" />);
  expect(box).toHaveClass('resize-x');
  expect(box).not.toHaveClass('resize-y');
});
