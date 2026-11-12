/** A button that may be cut by a narrow cell keeps the wrapper that carries its reason inside that cell. */
import { cleanup, screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { Button } from '@/ui/Button';
import { show } from './support';

afterEach(cleanup);

it('bounds the wrapper of a reason only when the button asks for it', () => {
  show(<><Button reason="Locked for now">Plain</Button><Button reason="Locked for now" bounded>Cut</Button></>);
  const plain = screen.getByRole('button', { name: 'Plain' }).parentElement;
  const cut = screen.getByRole('button', { name: 'Cut' }).parentElement;
  expect(plain).not.toHaveClass('min-w-0');
  expect(cut).toHaveClass('min-w-0', 'max-w-full');
});
