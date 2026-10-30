/** Work screens share familiar names; exact identifiers remain protocol/configuration values. */
import { afterEach, expect, it } from 'vitest';
import { permissionNameOf } from '@/features/access/names';
import { architectureName } from '@/features/resources/machines/facts';
import { defaultLevelName, levelOptions } from '@/features/notifications/levelWords';
import { i18n, setLocale } from '@/i18n';
import { knownPermissionLabel, permissionLabel } from '@/lib/permissions';

afterEach(async () => { await setLocale('en'); });
it.each(['en', 'zh-CN'] as const)('uses the same appearance, processor and permission names in %s', async (locale) => {
  await setLocale(locale);
  expect(i18n.t('profile.look.themeDevice')).toBe(i18n.t('choiceCatalog.theme.system.name'));
  expect(i18n.t('security.sessions.endOthersConfirm')).toBe(i18n.t('security.sessions.endOthers'));
  expect(i18n.t('agents.list.harness')).toBe(i18n.t('agents.add.harness'));
  expect(i18n.t('agents.list.harnessFilter')).toBe(i18n.t('agents.overview.harness'));
  expect(architectureName('x86_64', i18n.t)).toBe(i18n.t('choiceCatalog.architecture.x86_64.name'));
  expect(architectureName('aarch64', i18n.t)).toBe(i18n.t('choiceCatalog.architecture.aarch64.name'));
  expect(architectureName('x86_64', i18n.t)).not.toContain('x86_64');
  expect(architectureName('vendor_new_cpu', i18n.t)).toBe(i18n.t('machines.otherProcessor'));
  for (const permission of ['chat.view', 'doc.view', 'doc.edit', 'doc.manage']) {
    expect(knownPermissionLabel(permission)).not.toBeNull();
    expect(permissionLabel(permission)).not.toContain(permission);
  }
  expect(knownPermissionLabel('doc')).toBeNull();
  expect(permissionLabel('vendor.new_permission')).toBe(i18n.t('resourceAccessWords.unavailable'));
  expect(permissionNameOf({ name: 'vendor.new_permission', label: 'Open lab reports' })).toBe('Open lab reports');
  expect(permissionNameOf({ name: 'vendor.new_permission', label: 'vendor.new_permission' })).toBe(i18n.t('resourceAccessWords.unavailable'));
  expect(i18n.t('day100.builtin.sandbox.mode.title')).toBe(i18n.t('settingRegistry.sandbox.mode.title'));
  expect(i18n.t('day100.builtin.sandbox.mode.description')).toBe(i18n.t('settingRegistry.sandbox.mode.description'));
  for (const mode of ['ask', 'on_request', 'auto', 'bypass'] as const) {
    expect(i18n.t(`choiceCatalog.approvalMode.${mode}.description`)).not.toMatch(/harness|sandbox|运行框架|沙箱/i);
    expect(i18n.t(`machinePrograms.approval.${mode}`)).toBe(i18n.t(`composer.mode.labels.${mode}`));
  }
  for (const key of ['start.agent.unreported', 'agents.overview.notReported', 'providerLogin.homeDenied', 'inspector.parameters.subagentsUnsupported', 'side.unavailable.readOnly', 'environmentData.harnessAttach', 'choiceCatalog.sandboxLevel.open.description'] as const) {
    expect(i18n.t(key), key).not.toMatch(/\bharness\b|\bsandbox\b|file job|运行框架|执行器|沙箱|provider\.signin/i);
  }
});

it.each(['en', 'zh-CN'] as const)('names the actual inherited notification level independently of an exception in %s', async (locale) => {
  await setLocale(locale);
  const levels = ['all', 'important', 'off'] as const;
  const options = levelOptions(i18n.t, levels);
  expect(options.map((option) => option.name)).toEqual(levels.map((level) => i18n.t(`notificationBell.level.${level}`)));
  for (const option of options) expect(Object.keys(option).sort()).toEqual(['name', 'value']);
  expect(defaultLevelName(i18n.t, 'all')).toContain(i18n.t('notificationBell.level.all'));
  expect(defaultLevelName(i18n.t, 'all')).not.toContain(i18n.t('notificationBell.level.off'));
  expect(defaultLevelName(i18n.t, null)).toContain(i18n.t('notificationBell.unknownLevel'));
});
