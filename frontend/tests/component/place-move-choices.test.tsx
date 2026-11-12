/** A long destination list is a named choice viewport and keeps native radio access inside its dialog. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';

import { PlaceChooser, type ChooserRow } from '@/features/org/Move';
import { i18n, setLocale } from '@/i18n';
import { Dialog } from '@/ui/Dialog';

import { show } from './support';

afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`keeps the move choices named, keyboard selectable and independently scrollable (${locale})`, async () => {
  await setLocale(locale);
  // Twenty ordinary rows exceed the picker's nine-row viewport in the browser.
  const rows: ChooserRow[] = Array.from({ length: 20 }, (_, index) => ({ type: 'place', destination: {
    id: String(index), name: `Destination ${index}`, kind: 'folder', path: `Acme / Destination ${index}`, depth: 1,
    why: index === 0 ? 'here' : null,
  } }));
  const changed = vi.fn();
  function Scene() {
    const [value, setValue] = useState('1');
    return <Dialog open title="Move a topic" onOpenChange={() => undefined}>
      <PlaceChooser label={i18n.t('places.move.to')} rows={rows} value={value} hint="Acme" onChange={(next) => { changed(next); setValue(next); }} />
    </Dialog>;
  }
  show(<Scene />);
  const choices = screen.getByRole('radiogroup', { name: i18n.t('places.move.to') });
  expect(choices).toHaveAttribute('data-scroll-allow', 'choices');
  expect(within(choices).getAllByRole('radio')).toHaveLength(20);
  const unavailable = within(choices).getByRole('radio', { name: 'Destination 0' });
  expect(unavailable).toBeDisabled();
  expect(unavailable).toHaveAccessibleDescription(i18n.t('places.move.why.here'));
  expect(within(choices).getByRole('radio', { name: 'Destination 1' })).toBeChecked();
  const target = within(choices).getByRole('radio', { name: 'Destination 19' });
  target.focus();
  await userEvent.setup().keyboard(' ');
  expect(target).toHaveFocus();
  expect(target).toBeChecked();
  expect(changed).toHaveBeenCalledExactlyOnceWith('19');
  expect(within(choices).getByRole('radio', { name: 'Destination 1' })).not.toBeChecked();
});
