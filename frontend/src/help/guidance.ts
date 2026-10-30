/** The four release guides, their real setting vocabulary and their short screen walks. */
import type { TranslationKey } from '@/i18n';

export type GuideId = 'how-approvals-work' | 'let-agents-open-ports' | 'change-a-running-environment' | 'who-can-see-and-do-what';
export interface Guidance { id: GuideId; title: TranslationKey; settings: readonly string[]; tutorial: string; words: readonly string[] }

export const GUIDANCE: readonly Guidance[] = [
  { id: 'how-approvals-work', title: 'helpRelease.guides.approvals', tutorial: 'approvals-walk',
    settings: ['approval.mode', 'approval.allowed_modes', 'requests.expiry_hours', 'access.request_expiry_days', 'access.max_pending_requests', 'access.quick_grant_durations', 'capability.policy'],
    words: ['how do I approve', 'ask', 'on request', 'auto', 'bypass', 'reviewer', 'rules', '审批', '请求', '如何批准'] },
  { id: 'let-agents-open-ports', title: 'helpRelease.guides.ports', tutorial: 'ports-walk',
    settings: ['ports.enabled', 'ports.max_open', 'ports.link_max_secs', 'requests.expiry_hours', 'capability.policy', 'network.allowed_hosts'],
    words: ['how do I open ports without asking', 'port_place', 'port_host', 'port_link', 'template defaults', '端口', '无需询问', '能力'] },
  { id: 'change-a-running-environment', title: 'helpRelease.guides.liveEnvironment', tutorial: 'live-environment-walk',
    settings: ['env.resources', 'env.needs', 'env.mounts', 'env.storage', 'env.template_id', 'env.update', 'mcp.enabled', 'capability.policy', 'capability.recreate_grace_secs', 'network.allowed_hosts'],
    words: ['how do I change a running environment', 'how do I restart a server', 'restart', 'MCP', 'failed server', 'memory', 'limits', 'mounts', 'kvm', 'docker_socket', 'recreate', 'same image', '环境', '内存', '挂载', '重建', '重启', '服务器'] },
  { id: 'who-can-see-and-do-what', title: 'helpRelease.guides.access', tutorial: 'access-walk',
    settings: ['sharing.allow_external', 'sharing.allow_holders', 'collab.attribution'],
    words: ['how do I share', 'create', 'make', 'new', 'start', 'project', 'topic', 'thread', 'permission', 'holders', 'roles', 'nested groups', 'members', 'everyone', 'override', 'permissions', '创建项目', '创建话题', '开始线程', '权限', '分享', '覆盖', '群组'] },
];

export function guidesForSetting(key: string): GuideId[] { return GUIDANCE.filter((guide) => guide.settings.includes(key)).map((guide) => guide.id); }

export function guideWords(id: string): readonly string[] {
  const guide = GUIDANCE.find((one) => one.id === id);
  return guide === undefined ? [] : [...guide.words, ...guide.settings];
}
