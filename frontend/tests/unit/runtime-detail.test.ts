import { createInstance } from 'i18next';
import { expect, it } from 'vitest';

import { runtimeDetail } from '@/features/resources/runtimeDetail';
import english from '@/i18n/en.json';
import chinese from '@/i18n/zh-CN.json';

it.each(['en', 'zh-CN'])('explains deliberate provider stops in %s and keeps other host details', async (lng) => {
  const instance = createInstance();
  await instance.init({ lng, fallbackLng: false, resources: { en: { translation: english }, 'zh-CN': { translation: chinese } } });
  expect(runtimeDetail('provider_idle_stop', instance.t)).toBe(lng === 'en' ? 'Stopped after being idle' : '空闲后已停止');
  expect(runtimeDetail('provider_person_stop', instance.t)).toBe(lng === 'en' ? 'Stopped by a person' : '已由人员停止');
  expect(runtimeDetail('The host is unavailable', instance.t)).toBe('The host is unavailable');
  expect(runtimeDetail(null, instance.t)).toBeNull();
});
