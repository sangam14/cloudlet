export interface Snapshot {
  connection: { label: string; endpoint: string; status: 'ready' | 'degraded'; detail: string };
  capacity: { running: number; stopped: number; vcpus: number; memory_mib: number };
  workloads: { id: string; name: string; runtime: string; status: string; updated_at: string }[];
  models: { name: string; provider: string; status: string }[];
}

export interface ExecutionEvent {
  stage: 'Pending' | 'Building' | 'Running' | 'Done' | 'Failed' | 'Debug';
  stdout?: string | null;
  stderr?: string | null;
  exit_code?: number | null;
  raw_output?: boolean;
}

export class ApiError extends Error {
  constructor(public status: number, message: string) { super(message); }
}

function headers(token: string): HeadersInit {
  return token ? { Authorization: `Bearer ${token}` } : {};
}

async function checked(response: Response): Promise<Response> {
  if (!response.ok) {
    const body = (await response.text()).slice(0, 1500);
    throw new ApiError(response.status, body || `Request failed (${response.status})`);
  }
  return response;
}

export async function overview(token: string, signal: AbortSignal): Promise<Snapshot> {
  return (await checked(await fetch('/api/v1/overview', {
    headers: headers(token), signal, cache: 'no-store',
  }))).json();
}

export type Language = 'rust' | 'python' | 'node';

export function workloadRequest(name: string, code: string, language: Language = 'rust') {
  return {
    workload_name: name, language, code, log_level: 'info',
    action: 'prepare-and-run',
    server: { address: 'localhost', port: 50051 },
    build: { 'source-code-path': '/unused-by-api', release: true },
  };
}

// Incremental SSE decoding: POST streams cannot use the EventSource API.
export class SseDecoder {
  private pending = '';
  constructor(private onEvent: (event: ExecutionEvent) => void) {}

  push(chunk: string) {
    this.pending += chunk;
    for (;;) {
      const delimiter = /\r?\n\r?\n/.exec(this.pending);
      if (!delimiter) break;
      const frame = this.pending.slice(0, delimiter.index);
      this.pending = this.pending.slice(delimiter.index + delimiter[0].length);
      if (frame.length > 256 * 1024) throw new Error('Execution event exceeded the output limit.');
      this.frame(frame);
    }
    if (this.pending.length > 256 * 1024) throw new Error('Execution event exceeded the output limit.');
  }

  finish() {
    if (this.pending.trim()) this.frame(this.pending);
    this.pending = '';
  }

  private frame(frame: string) {
    const lines = frame.split(/\r?\n/);
    const error = lines.find(line => /^:\s*(VMM stream error|serialisation error):/.test(line));
    if (error) throw new Error(error.slice(1).trim());
    const data = lines.filter(line => line.startsWith('data:'))
      .map(line => line.slice(5).replace(/^ /, '')).join('\n');
    if (!data) return;
    const event = JSON.parse(data) as ExecutionEvent;
    if (!['Pending', 'Building', 'Running', 'Done', 'Failed', 'Debug'].includes(event.stage)
      || (event.stdout != null && typeof event.stdout !== 'string')
      || (event.stderr != null && typeof event.stderr !== 'string')
      || (event.exit_code != null && typeof event.exit_code !== 'number')) {
      throw new Error('Invalid execution event received from the API.');
    }
    this.onEvent(event);
  }
}

export async function execute(name: string, code: string, token: string,
  signal: AbortSignal, onEvent: (event: ExecutionEvent) => void, language: Language = 'rust') {
  const response = await checked(await fetch('/api/v1/workloads', {
    method: 'POST', headers: { ...headers(token), 'Content-Type': 'application/json', Accept: 'text/event-stream' },
    body: JSON.stringify(workloadRequest(name, code, language)), signal,
  }));
  if (!response.headers.get('content-type')?.includes('text/event-stream')) {
    throw new Error('Expected an execution stream from the API.');
  }
  if (!response.body) throw new Error('The API returned no execution stream.');
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  const sse = new SseDecoder(onEvent);
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      sse.push(decoder.decode(value, { stream: true }));
    }
    sse.push(decoder.decode());
    sse.finish();
  } finally {
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}

export async function shutdown(token: string) {
  const response = await checked(await fetch('/api/v1/vmm/shutdown', {
    method: 'POST', headers: headers(token), signal: AbortSignal.timeout(10_000),
  }));
  const result = await response.json() as { success: boolean };
  if (!result.success) throw new Error('The VMM did not acknowledge the shutdown request.');
}

export async function sandboxAction(id: string, action: 'stop' | 'remove', token: string) {
  await checked(await fetch(`/api/v1/sandboxes/${encodeURIComponent(id)}${action === 'stop' ? '/stop' : ''}`, {
    method: action === 'stop' ? 'POST' : 'DELETE', headers: headers(token), signal: AbortSignal.timeout(35_000),
  }));
}

export function connectionLabel(status: 'loading' | 'ready' | 'offline' | 'locked', snapshot: Snapshot | null) {
  return status === 'loading' ? 'Checking connection' : status === 'ready' ? 'BoxLite ready' : status === 'locked' ? 'Authentication required' : snapshot ? 'Runtime needs attention' : 'API unreachable';
}

export function appendOutput(previous: string, chunk: string) {
  const combined = previous + chunk;
  return combined.length > 100_000 ? '[Earlier output truncated]\n' + combined.slice(-100_000) : combined;
}

export function executionStage(previous: string, event: ExecutionEvent): string {
  if (previous === 'Failed' || event.stage === 'Failed' || (event.exit_code != null && event.exit_code !== 0)) return 'Failed';
  // stdout and stderr arrive independently; late output must not undo Done.
  if (previous === 'Done' || event.stage === 'Debug') return previous;
  return event.stage;
}

export function executionOutput(event: ExecutionEvent): string {
  if (event.raw_output) return (event.stdout ?? '') + (event.stderr ?? '');
  // The guest sends one line per field, with the newline removed by BufRead.
  const line = (text: string | null | undefined, prefix = '') => text == null ? '' : prefix + text + (text.endsWith('\n') ? '' : '\n');
  return line(event.stdout) + line(event.stderr, '[stderr] ');
}
