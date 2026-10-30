/** Expanding read-only text changes what can be read, with the disclosure named in either language. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import { setLocale } from '@/i18n';
import { TextPreview } from '@/ui/Preview';
import { show } from './support';

it.each([['en', 'Show all', 'Show less'], ['zh-CN', '显示全部', '收起']] as const)('expands with named keyboard controls in %s', async (locale, all, less) => {
  try {
    await setLocale(locale);
    const user = userEvent.setup();
    show(<TextPreview value={Array.from({ length: 12 }, (_, index) => `Entry ${index + 1}`).join('\n')} />);
    expect(screen.queryByText(/Entry 12/)).not.toBeInTheDocument();
    const expand = screen.getByRole('button', { name: all });
    expand.focus();
    await user.keyboard('{Enter}');
    expect(screen.getByText(/Entry 12/)).toBeVisible();
    const collapse = screen.getByRole('button', { name: less });
    expect(collapse).toHaveAttribute('aria-expanded', 'true');
    await user.click(collapse);
    expect(screen.queryByText(/Entry 12/)).not.toBeInTheDocument();
  } finally { await setLocale('en'); }
});
