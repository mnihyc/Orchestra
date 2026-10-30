/** A package export starts with the contents needed for a round trip and keeps each explicit choice. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';
import { PlaceOptions } from '@/features/export/PlaceOptions';
import { OPENING_CHOICES, type Choices } from '@/features/export/choices';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

function Options() {
  const [choices, setChoices] = useState<Choices>(OPENING_CHOICES);
  return <PlaceOptions choices={choices} onChange={(patch) => setChoices((current) => ({ ...current, ...patch }))} />;
}

it.each(['en', 'zh-CN'] as const)('offers a package and allows its files and comments to be switched off in %s', async (locale) => {
  try {
    await setLocale(locale);
    const user = userEvent.setup();
    show(<Options />);
    const packageChoice = screen.getByRole('radio', { name: i18n.t('choiceCatalog.exportFormat.package.name') });
    await user.click(packageChoice);
    expect(packageChoice).toBeChecked();
    expect(screen.getByText(i18n.t('importWizard.export.packageNote'))).toBeVisible();
    const files = screen.getByRole('checkbox', { name: i18n.t('export.more.files') });
    const comments = screen.getByRole('checkbox', { name: i18n.t('export.more.comments') });
    expect(files).toBeChecked();
    expect(comments).toBeChecked();
    await user.click(files);
    await user.click(comments);
    expect(files).not.toBeChecked();
    expect(comments).not.toBeChecked();
  } finally { await setLocale('en'); }
});
