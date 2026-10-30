/** Kubernetes scheduler requests retain the driver-specific text and the other fields. */
import { fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { SchemaForm } from '@/features/resources/templates/SchemaSettings';
import type { Schema } from '@/features/resources/templates/schema';
import { setLocale } from '@/i18n';
import { show } from './support';

const schema: Schema = { type: 'object', properties: {
  image: { type: 'string', title: 'Driver image setting' },
  requests: { type: 'object', properties: {
    cpu_millis: { type: 'integer', title: 'Driver CPU setting', minimum: 0, default: 0, 'x-unit': 'mCPU' },
    memory_mb: { type: 'integer', title: 'Driver memory setting', minimum: 0, default: 0, 'x-unit': 'MiB' },
  } },
} };
afterEach(async () => { await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`shows translated scheduler requests and keeps sibling values (${locale})`, async () => {
    await setLocale(locale);
    const changed = vi.fn();
    show(<SchemaForm schema={schema} spec={{ requests: { memory_mb: 256 }, untouched: true }} driver="kubernetes" onChange={changed} />);
    const title = locale === 'en' ? 'Kubernetes requests' : 'Kubernetes 资源请求';
    const cpu = locale === 'en' ? 'CPU request' : 'CPU 请求';
    const memory = locale === 'en' ? 'Memory request' : '内存请求';
    expect(screen.getByRole('group', { name: title })).toBeVisible();
    expect(screen.getByRole('textbox', { name: locale === 'en' ? 'Image reference' : '镜像引用' })).toBeVisible();
    expect(screen.getByText(locale === 'en' ? 'Pull an image from a registry; pin a digest for repeatable starts.' : '从镜像仓库拉取镜像；固定摘要可确保每次启动使用同一镜像。')).toBeVisible();
    expect(screen.getByText(locale === 'en' ? 'Kubernetes reserves this many thousandths of a CPU when it schedules the Pod.' : 'Kubernetes 调度 Pod 时预留这些千分之一 CPU。')).toBeVisible();
    const input = screen.getByRole('spinbutton', { name: cpu });
    expect(input).toHaveValue(0);
    expect(screen.getByRole('spinbutton', { name: memory })).toHaveValue(256);
    fireEvent.change(input, { target: { value: '500' } });
    expect(changed).toHaveBeenLastCalledWith({ requests: { cpu_millis: 500, memory_mb: 256 }, untouched: true });
  });
}

it('leaves another known driver\'s meaning for a same-named field intact', async () => {
  await setLocale('en');
  const changed = vi.fn();
  show(<SchemaForm schema={schema} spec={{}} driver="docker" onChange={changed} />);
  expect(screen.getByRole('spinbutton', { name: 'Driver CPU setting' })).toBeVisible();
  expect(screen.queryByRole('spinbutton', { name: 'CPU request' })).not.toBeInTheDocument();
});

it('uses shared translated choices for the described driver', async () => {
  await setLocale('zh-CN');
  const network: Schema = { type: 'object', properties: { network: { type: 'object', properties: {
    egress: { type: 'string', enum: ['open', 'restricted'] },
  } } } };
  const changed = vi.fn();
  show(<SchemaForm schema={network} spec={{ network: { egress: 'open' } }} driver="kubernetes" onChange={changed} />);
  await userEvent.click(screen.getByRole('combobox', { name: '对外连接' }));
  const option = screen.getByRole('option', { name: '受限' });
  expect(option).toHaveAccessibleDescription('仅访问 Orch 和所在位置允许列表中的主机名。');
  await userEvent.click(option);
  expect(changed).toHaveBeenLastCalledWith({ network: { egress: 'restricted' } });
});
