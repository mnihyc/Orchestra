/** The same class limitation is told in the selected language wherever a GPU is refused. */
import { screen } from '@testing-library/react';
import { expect, it } from 'vitest';
import { LimitLines } from '@/features/resources/isolation/RuntimeInfo';
import type { Boundary, RuntimeLimit } from '@/generated/api-types';
import { setLocale } from '@/i18n';
import { show } from './support';

const english = 'A GPU is not offered on this cluster: use a runc environment on a Docker host with the GPU capability.';
const chinese = '此集群不提供 GPU：请在 Docker 主机上使用具有 GPU 能力的 runc 环境。';
const gpu: RuntimeLimit = { key: 'kubernetes_gpu', touches: ['gpu'], todo: null, why: english };

for (const locale of ['en', 'zh-CN'] as const) {
  for (const boundary of ['shared_kernel', 'user_space_kernel', 'own_kernel'] as const satisfies readonly Boundary[]) {
    it(`keeps the Kubernetes GPU reason once (${locale}, ${boundary})`, async () => {
      await setLocale(locale);
      try {
        show(<LimitLines limits={[gpu]} unavailable={{ gpu: english }} boundary={boundary} />);
        expect(screen.getByText(locale === 'en' ? english : chinese, { exact: false })).toBeVisible();
        expect(screen.getAllByText(locale === 'en' ? english : chinese, { exact: false })).toHaveLength(1);
      } finally {
        await setLocale('en');
      }
    });
  }
}

it('keeps existing Docker boundary translations without a matching keyed reason', () => {
  show(<LimitLines limits={[]} unavailable={{ gpu: 'the host GPU cannot be passed' }} boundary="user_space_kernel" />);
  expect(screen.getByText(/host's graphics processors cannot be passed into a kernel in user space/)).toBeVisible();
});
