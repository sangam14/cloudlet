import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, expect, it, vi } from 'vitest';
import { Models } from './Models';
import { models, modelOperation } from './api';

afterEach(() => vi.unstubAllGlobals());

it('renders real setup and inference boundaries without fabricated activity', () => {
  const html = renderToStaticMarkup(<Models token="" />);
  expect(html).toContain('Inventory unavailable');
  expect(html).toContain('Guest networking stays disabled');
  expect(html).toContain('Run prompt');
  expect(html).not.toContain('Operation completed');
});

it('loads inventory only through the same-origin authenticated API', async () => {
  const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify({ ready: true, models: [] })));
  vi.stubGlobal('fetch', fetcher);
  await models('test-only-token', new AbortController().signal);
  expect(fetcher).toHaveBeenCalledWith('/api/v1/models', expect.objectContaining({
    headers: { Authorization: 'Bearer test-only-token' }, cache: 'no-store',
  }));
});

it('submits bounded operation fields, never a daemon URL or provider credentials', async () => {
  const fetcher = vi.fn().mockResolvedValue(new Response('{"accepted":true}', { status: 202 }));
  vi.stubGlobal('fetch', fetcher);
  await modelOperation('run', 'fixture-model', 'Hello', 'test-only-token');
  expect(fetcher).toHaveBeenCalledWith('/api/v1/models/operations', expect.objectContaining({
    method: 'POST', body: JSON.stringify({ action: 'run', model: 'fixture-model', prompt: 'Hello' }),
    headers: { Authorization: 'Bearer test-only-token', 'Content-Type': 'application/json' },
  }));
});

it('reports admission errors instead of inventing a successful model run', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('A model operation is already in progress', { status: 409 })));
  await expect(modelOperation('run', 'fixture-model', 'Hello', '')).rejects.toThrow('already in progress');
});
