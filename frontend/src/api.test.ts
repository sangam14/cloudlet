import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError, SseDecoder, appendOutput, execute, executionOutput, executionStage, overview, shutdown, workloadRequest } from './api';

afterEach(() => vi.unstubAllGlobals());

describe('SSE decoder', () => {
  it('restores line boundaries removed by the guest and preserves stderr labels', () => {
    expect(executionOutput({ stage: 'Running', stdout: 'hello', stderr: 'warning' })).toBe('hello\n[stderr] warning\n');
    expect(executionOutput({ stage: 'Running', stdout: 'hello\n' })).toBe('hello\n');
    expect(executionOutput({ stage: 'Running', stdout: '' })).toBe('\n');
    expect(executionOutput({ stage: 'Done' })).toBe('');
  });
  it('retains terminal status when late stdout or stderr arrives', () => {
    expect(executionStage('Done', { stage: 'Running', stderr: 'late stderr' })).toBe('Done');
    expect(executionStage('Failed', { stage: 'Running', stdout: 'late stdout' })).toBe('Failed');
    expect(executionStage('Running', { stage: 'Done', exit_code: 0 })).toBe('Done');
    expect(executionStage('Building', { stage: 'Debug' })).toBe('Building');
  });
  it('handles fragmented CRLF, keepalives, multiple events, and a trailing frame', () => {
    const events: unknown[] = [];
    const decoder = new SseDecoder(event => events.push(event));
    const stream = ': keep-alive\r\n\r\ndata: {"stage":"Running","stdout":"hi"}\r\n\r\ndata: {"stage":"Done","exit_code":0}';
    for (const character of stream) decoder.push(character);
    decoder.finish();
    expect(events).toEqual([{ stage: 'Running', stdout: 'hi' }, { stage: 'Done', exit_code: 0 }]);
  });
  it('handles multiline data and ignores heartbeat-only streams', () => {
    const event = vi.fn();
    const decoder = new SseDecoder(event);
    decoder.push(': heartbeat\n\ndata: {"stage":"Done",\ndata: "exit_code":0}\n\n');
    expect(event).toHaveBeenCalledWith({ stage: 'Done', exit_code: 0 });
  });
  it('surfaces backend stream errors instead of claiming completion', () => {
    expect(() => new SseDecoder(vi.fn()).push(': execution stream error: connection lost\n\n')).toThrow('connection lost');
  });
  it('rejects malformed and oversized frames', () => {
    expect(() => new SseDecoder(vi.fn()).push('data: {"stage":"fake"}\n\n')).toThrow('Invalid execution event');
    expect(() => new SseDecoder(vi.fn()).push('data: {"stage":"Running","stdout":{}}\n\n')).toThrow('Invalid execution event');
    expect(() => new SseDecoder(vi.fn()).push('x'.repeat(262145))).toThrow('output limit');
  });
});

describe('control plane transport', () => {
  it('builds the existing Rust API request contract', () => {
    expect(workloadRequest('hello', 'fn main() {}')).toEqual({
      workload_name: 'hello', code: 'fn main() {}', language: 'rust', log_level: 'info',
      action: 'prepare-and-run', server: { address: 'localhost', port: 50051 },
      build: { 'source-code-path': '/unused-by-api', release: true },
    });
  });
  it('uses same-origin authenticated POST and decodes chunked UTF-8', async () => {
    const bytes = new TextEncoder().encode('data: {"stage":"Running","stdout":"你好"}\n\ndata: {"stage":"Done","exit_code":0}\n\n');
    const body = new ReadableStream({ start(controller) {
      for (const byte of bytes) controller.enqueue(new Uint8Array([byte]));
      controller.close();
    } });
    const fetch = vi.fn().mockResolvedValue(new Response(body, { headers: { 'Content-Type': 'text/event-stream' } }));
    vi.stubGlobal('fetch', fetch);
    const event = vi.fn();
    await execute('hello', 'fn main() {}', 'secret-token', new AbortController().signal, event);
    expect(fetch).toHaveBeenCalledWith('/api/v1/workloads', expect.objectContaining({ method: 'POST', headers: expect.objectContaining({ Authorization: 'Bearer secret-token' }) }));
    expect(event).toHaveBeenCalledWith({ stage: 'Running', stdout: '你好' });
    expect(event).toHaveBeenLastCalledWith({ stage: 'Done', exit_code: 0 });
  });
  it('surfaces HTTP authentication and runtime errors', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('Token required', { status: 401 })));
    await expect(overview('', new AbortController().signal)).rejects.toBeInstanceOf(ApiError);
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('BoxLite unavailable', { status: 503 })));
    await expect(execute('x', 'code', '', new AbortController().signal, vi.fn())).rejects.toThrow('BoxLite unavailable');
  });
  it('does not treat an HTML fallback as an execution stream', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('<html/>', { headers: { 'Content-Type': 'text/html' } })));
    await expect(execute('x', 'code', '', new AbortController().signal, vi.fn())).rejects.toThrow('Expected an execution stream');
  });
  it('checks shutdown acknowledgement', async () => {
    const fetch = vi.fn().mockResolvedValue(Response.json({ success: false }));
    vi.stubGlobal('fetch', fetch);
    await expect(shutdown('token')).rejects.toThrow('did not acknowledge');
    expect(fetch).toHaveBeenCalledWith('/api/v1/workloads/cancel', expect.objectContaining({ method: 'POST' }));
  });
  it('caps retained output', () => {
    const output = appendOutput('a'.repeat(100000), 'last line');
    expect(output.startsWith('[Earlier output truncated]')).toBe(true);
    expect(output.endsWith('last line')).toBe(true);
    expect(output.length).toBeLessThan(100100);
  });
});
