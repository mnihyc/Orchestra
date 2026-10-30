/** A real control opens its shipped interface and its links stay in the help; schema links are assets. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ArticleView } from '@/features/help/Article';
import { ProviderGuideLink } from '@/features/help/ProviderGuideLink';
import { readArticle } from '@/help/guide';
import { show } from './support';

const opens = vi.hoisted(() => vi.fn());
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => opens }));

describe('provider guide entrances', () => {
  beforeEach(() => { opens.mockClear(); });

  it('names the same page a provider source control opens', async () => {
    show(<ProviderGuideLink guide="provider-facts" />);
    await userEvent.click(screen.getByRole('button', { name: 'Implement a provider facts source' }));
    expect(opens).toHaveBeenCalledWith('provider-facts');
  });

  it('explains commands from their control', async () => {
    show(<ProviderGuideLink guide="commands-after-start" />);
    await userEvent.click(screen.getByRole('button', { name: 'Run commands after a unit starts' }));
    expect(opens).toHaveBeenCalledWith('commands-after-start');
  });

  it('follows another interface without mistaking its schema for a screen', async () => {
    const article = { ...readArticle('provider-facts', '---\nlink_prefix: provider-\n---\n# Provider facts\n\n[Models](models.md) [Schema](schema/source-ask.json)'), files: { 'schema/source-ask.json': '/assets/source-ask.json' } };
    const onArticle = vi.fn();
    const onScreen = vi.fn();
    show(<ArticleView article={article} heading={null} onArticle={onArticle} onScreen={onScreen} />);
    await userEvent.click(screen.getByRole('link', { name: 'Models' }));
    expect(onArticle).toHaveBeenCalledWith('provider-models', null);
    expect(screen.getByRole('link', { name: 'Schema' })).toHaveAttribute('href', '/assets/source-ask.json');
    await userEvent.click(screen.getByRole('link', { name: 'Schema' }));
    expect(screen.getByRole('link', { name: 'Schema' })).toHaveAttribute('target', '_blank');
    expect(onScreen).not.toHaveBeenCalled();
  });
});
