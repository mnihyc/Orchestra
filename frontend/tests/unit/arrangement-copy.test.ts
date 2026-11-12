/** Every native arrangement can be named, including the built-in program with no environment. */
import { afterEach, expect, it } from 'vitest';
import type { ArrangementType } from '@/generated/api-types';
import { choiceCopy } from '@/i18n/choices';
import { i18n, setLocale } from '@/i18n';

const arrangements = {
  control_plane: true, separated: true, key_separated: true,
  credential_shared: true, handed_over: true, together: true,
} satisfies Record<ArrangementType, true>;
afterEach(async () => { await setLocale('en'); });

it.each([['en', 'Run in Orch'], ['zh-CN', '在 Orch 中运行']] as const)('names every native arrangement in %s', async (locale, builtIn) => {
  await setLocale(locale);
  for (const kind of Object.keys(arrangements) as (keyof typeof arrangements)[]) {
    const copy = choiceCopy(i18n.t, `arrangement.${kind}`);
    expect(copy.name).not.toMatch(/^choiceCatalog\./u);
    expect(copy.description).not.toMatch(/^choiceCatalog\./u);
    expect(copy.name.trim()).not.toBe(''); expect(copy.description.trim()).not.toBe('');
  }
  expect(choiceCopy(i18n.t, 'arrangement.control_plane').name).toBe(builtIn);
});
