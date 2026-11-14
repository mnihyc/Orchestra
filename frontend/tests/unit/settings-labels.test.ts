/** Settings keep one section name and distinguish the places whose tabs are open. */
import { afterEach, describe, expect, it } from 'vitest';

import { sectionLabel, settingsScreenTitle } from '@/features/settings/sectionLabels';
import { i18n, setLocale } from '@/i18n';
import { placeLabels } from '@/lib/placeLabel';

afterEach(() => setLocale('en'));

describe('a settings section in navigation and the title bar', () => {
  it('keeps email channels named consistently while destinations retain their own section name in both languages', async () => {
    for (const language of ['en', 'zh-CN'] as const) {
      await setLocale(language);
      const t = i18n.getFixedT(language);
      expect(sectionLabel('defaults', t)).toBe(t('day100.names.defaults'));
      expect(sectionLabel('roles', t)).toBe(t('day100.names.roles'));
      const channels = sectionLabel('channels', t);
      expect(channels).toBe(language === 'en' ? 'Email and channels' : '邮件与渠道');
      expect(channels).toBe(t('admin.sections.email.title'));
      expect(channels).toBe(t('choiceCatalog.adminSection.email.name'));
      expect(settingsScreenTitle(channels, null)).toBe(channels);
      expect(sectionLabel('destinations', t)).toBe(t('day100.names.outside'));
      expect(sectionLabel('destinations', t)).not.toBe(channels);
      expect(sectionLabel('general', t)).toBe(t('settings.org.sectionNames.general'));
    }
  });

  it('keeps General with its common qualified place label rather than making two identical tabs', () => {
    const places = placeLabels([
      { id: 'a-web', name: 'web', above: [{ id: 'acme', name: 'Acme' }] },
      { id: 'b-web', name: 'web', above: [{ id: 'beta', name: 'Beta' }] },
    ]);
    expect(settingsScreenTitle('General', places.get('a-web'))).toBe('General · web · Acme');
    expect(settingsScreenTitle('General', places.get('b-web'))).toBe('General · web · Beta');
    expect(settingsScreenTitle('General', null)).toBe('General');
  });
});
