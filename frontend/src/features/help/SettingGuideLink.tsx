/** Numeric and retention hints open the matching authored heading in the reader's language. */
import { useTranslation } from 'react-i18next';
import type { SettingsTarget } from '@/api/settings';
import type { SettingDef } from '@/generated/api-types';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';

const GUIDES = {
  cache: { article: 'storage-cleanup', heading: 'unused-caches', chinese: '未使用缓存' },
  releases: { article: 'storage-cleanup', heading: 'unused-releases', chinese: '未使用发布' },
  kept: { article: 'storage-cleanup', heading: 'kept-data', chinese: '保留的数据' },
  zero: { article: 'numeric-settings', heading: 'zero-values', chinese: '零值' },
  batch: { article: 'switch-threads', heading: 'batch-size', chinese: '批量大小' },
} as const;
const ZERO_KEYS = new Set(['env.recycle_idle_minutes', 'capability.recreate_grace_secs', 'tags.max_per_place', 'switch.wait_minutes', 'references.preview_lines']);

export function settingGuide(def: Pick<SettingDef, 'key' | 'display'>) {
  if (def.key === 'cleanup.unused_keep_days') return GUIDES.cache;
  if (def.key === 'cleanup.release_keep_count') return GUIDES.releases;
  if (def.key === 'env.removed_keep_days') return GUIDES.kept;
  if (def.key === 'switch.batch_threads') return GUIDES.batch;
  return def.display?.zero === 'off' || ZERO_KEYS.has(def.key) ? GUIDES.zero : null;
}

export function SettingGuideLink({ def, target }: { def: Pick<SettingDef, 'key' | 'display'>; target?: SettingsTarget }) {
  const { t } = useTranslation();
  const guide = settingGuide(def);
  if (guide === null) return null;
  return <ArticleGuideLink article={guide.article} heading={guide.heading}
    title={t(guide.article === 'storage-cleanup' ? 'storageCleanup.guide' : guide.article === 'switch-threads' ? 'change.batch.guide' : 'numericSettings.guide')} {...(target === undefined ? {} : {target})} />;
}
