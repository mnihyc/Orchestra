/** A keyboard pickup stays spoken until a view actually moves; destinations and cancellation remain announced. */
import type { Active, Over } from '@dnd-kit/core';
import { afterEach, expect, it } from 'vitest';
import { viewDragAnnouncements } from '@/features/workbench/MoveView';
import { LAYOUT_VERSION, makeContainer, makeGroup, makeTab, type Layout } from '@/features/workbench/layout';
import { i18n, setLocale } from '@/i18n';

afterEach(async () => { await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`retains pickup before movement and speaks the destination after movement in ${locale}`, async () => {
  await setLocale(locale);
  const tab = makeTab('navigation'), container = makeContainer('navigation', [tab]);
  const layout: Layout = { version: LAYOUT_VERSION, main: makeGroup(), bottom: { open: false, size: 240, group: makeGroup() },
    left: { open: true, size: 280, active: 'navigation', containers: [container] }, right: { open: false, size: 400, active: null, containers: [] }, maximized: null };
  const title = i18n.t('views.navigation.title');
  const rect = { left: 0, top: 0, right: 200, bottom: 40, width: 200, height: 40 };
  const active: Active = { id: tab.id, data: { current: { title } }, rect: { current: { initial: rect, translated: rect } } };
  const over: Over = { id: 'navigation-home', rect, disabled: false, data: { current: { label: title, target: { to: 'container', bar: 'left', container: 'navigation' } } } };
  const announcements = viewDragAnnouncements(layout, i18n.t);
  expect(announcements.onDragStart({ active })).toBe(i18n.t('workbench.drag.pickedUp', { title }));
  expect(announcements.onDragOver({ active, over })).toBeUndefined();
  active.rect.current.translated = { ...rect, left: 32, right: 232 };
  expect(announcements.onDragOver({ active, over })).toBe(i18n.t('workbench.drag.over', { spot: title }));
  expect(announcements.onDragOver({ active, over: null })).toBe(i18n.t('workbench.drag.nowhere'));
  expect(announcements.onDragEnd({ active, over })).toBe(i18n.t('workbench.drag.dropped', { spot: title }));
  expect(announcements.onDragCancel({ active, over })).toBe(i18n.t('workbench.drag.putBack'));
});
