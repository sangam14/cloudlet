import { useCallback, useEffect, useRef, useState, type FormEvent, type ReactNode } from 'react';
import {
  Activity, ArrowDownToLine, ArrowRight, Box, Boxes, Check, ChevronDown,
  CircleHelp, Code2, Copy, Cpu, ExternalLink, FileCode2, LayoutDashboard,
  LoaderCircle, Menu, MoreHorizontal, Network, Play, Plus, Radio, RefreshCw,
  Server, Settings2, ShieldCheck, Square, Terminal, TriangleAlert,
  Unplug, Wifi, X, Zap, type LucideIcon,
} from 'lucide-react';
import { ApiError, appendOutput, connectionLabel, execute, executionOutput, executionStage, overview, shutdown, type Snapshot, type Language } from './api';
import { Button } from '@/components/ui/button';
import { Badge as UiBadge } from '@/components/ui/badge';
import { Dialog as UiDialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { Label } from '@/components/ui/label';
import { AlertDialog, AlertDialogContent, AlertDialogHeader, AlertDialogTitle, AlertDialogDescription, AlertDialogFooter, AlertDialogAction, AlertDialogCancel } from '@/components/ui/alert-dialog';
import { RuntimeInventory } from './RuntimeInventory';
import { Models } from './Models';

type Page = 'Dashboard' | 'Sandboxes' | 'Templates' | 'Activity' | 'Models' | 'Settings';
type Run = { id: string; name: string; stage: string; output: string; started: string; exitCode?: number };
type Template = { name: string; description: string; language: Language; icon: LucideIcon; code: string };

const templates: Template[] = [
  { name: 'Hello sandbox', description: 'Your first isolated Rust workload', language: 'rust', icon: Box,
    code: 'fn main() {\n    println!("Hello from Cloudlet!");\n    println!("Running inside a KVM guest.");\n}\n' },
  { name: 'Python sandbox', description: 'Inspect the guest environment', language: 'python', icon: Terminal,
    code: 'import platform, os\nprint("Hello from BoxLite!")\nprint("System:", platform.system())\nprint("Architecture:", platform.machine())\nprint("User ID:", os.getuid())\n' },
  { name: 'Node workspace', description: 'Run JavaScript with Node.js 22', language: 'node', icon: Cpu,
    code: 'console.log("Hello from Cloudlet!");\nconsole.log("Node:", process.version);\nconsole.log("Platform:", process.platform);\nconsole.log("User ID:", process.getuid());\n' },
];

const nav: { group: string; items: { page: Page; icon: LucideIcon }[] }[] = [
  { group: '', items: [{ page: 'Dashboard', icon: LayoutDashboard }] },
  { group: 'WORKFLOW', items: [{ page: 'Sandboxes', icon: Box }, { page: 'Templates', icon: FileCode2 }, { page: 'Activity', icon: Terminal }] },
  { group: 'INFRASTRUCTURE', items: [{ page: 'Models', icon: Boxes }, { page: 'Settings', icon: Settings2 }] },
];

function Badge({ stage }: { stage: string }) {
  const tone = stage === 'Running' || stage === 'Done' ? 'green' : stage === 'Failed' || stage === 'Interrupted' ? 'red' : stage === 'Building' || stage === 'Pending' ? 'amber' : 'muted';
  return <UiBadge variant="outline" className={`badge ${tone}`}><span className="dot" />{stage === 'Done' ? 'Completed' : stage}</UiBadge>;
}

function Dialog({ title, children, close }: { title: string; children: ReactNode; close: () => void }) {
  return <UiDialog open onOpenChange={open => { if (!open) close(); }}><DialogContent className="code-dialog max-h-[90dvh] overflow-y-auto sm:max-w-2xl"><DialogHeader><DialogTitle>{title}</DialogTitle><DialogDescription>Review source before executing it in a network-isolated BoxLite guest.</DialogDescription></DialogHeader>{children}</DialogContent></UiDialog>;
}

export function App() {
  const [page, setPage] = useState<Page>('Dashboard');
  const [sidebar, setSidebar] = useState(false);
  const [token, setToken] = useState('');
  const [tokenDraft, setTokenDraft] = useState('');
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [connection, setConnection] = useState<'loading' | 'ready' | 'offline' | 'locked'>('loading');
  const [apiError, setApiError] = useState('');
  const [refreshKey, setRefreshKey] = useState(0);
  const [refreshing, setRefreshing] = useState(false);
  const [runs, setRuns] = useState<Run[]>([]);
  const [activeRun, setActiveRun] = useState<string | null>(null);
  const [selectedRun, setSelectedRun] = useState<string | null>(null);
  const [template, setTemplate] = useState<Template | null>(null);
  const [name, setName] = useState('');
  const [code, setCode] = useState('');
  const [formError, setFormError] = useState('');
  const [notice, setNotice] = useState('');
  const [confirmShutdown, setConfirmShutdown] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [copied, setCopied] = useState(false);
  const execution = useRef<AbortController | null>(null);
  const runGuard = useRef(false);
  const runSequence = useRef(0);
  const ready = connection === 'ready';
  const viewing = runs.find(run => run.id === selectedRun);
  const inventory = snapshot?.workloads ?? [];
  const runtimeLabel = connectionLabel(connection, snapshot);

  const navigate = (next: Page) => { setPage(next); setSidebar(false); };
  useEffect(() => () => execution.current?.abort(), []);
  useEffect(() => {
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const controller = new AbortController();
    async function poll() {
      setRefreshing(true);
      try {
        const data = await overview(token, AbortSignal.any([controller.signal, AbortSignal.timeout(5000)]));
        if (!disposed) {
          setSnapshot(data);
          setConnection(data.connection.status === 'ready' ? 'ready' : 'offline');
          setApiError('');
        }
      } catch (error) {
        if (!disposed) {
          setSnapshot(null);
          setConnection(error instanceof ApiError && error.status === 401 ? 'locked' : 'offline');
          setApiError(error instanceof Error ? error.message : 'Unable to reach the API.');
        }
      } finally {
        if (!disposed) { setRefreshing(false); timer = setTimeout(poll, 5000); }
      }
    }
    void poll();
    return () => { disposed = true; controller.abort(); clearTimeout(timer); };
  }, [token, refreshKey]);

  const openCreate = (choice = templates[0]) => {
    setTemplate(choice); setName(choice.name.toLowerCase().replaceAll(' ', '-'));
    setCode(choice.code); setFormError('');
  };

  const updateRun = useCallback((id: string, change: (run: Run) => Run) => {
    setRuns(current => current.map(run => run.id === id ? change(run) : run));
  }, []);

  async function startRun(event: FormEvent) {
    event.preventDefault();
    if (runGuard.current || !ready) return;
    if (!/^[a-z0-9-]{1,64}$/.test(name)) { setFormError('Use 1–64 lowercase letters, numbers, or hyphens.'); return; }
    if (!code.trim() || new TextEncoder().encode(code).length > 240 * 1024) { setFormError('Enter source code, up to 240 KiB.'); return; }
    runGuard.current = true;
    const id = `${Date.now()}-${++runSequence.current}`;
    const controller = new AbortController();
    execution.current = controller;
    setRuns(current => [{ id, name, output: '', stage: 'Pending', started: new Date().toLocaleTimeString() }, ...current].slice(0, 100));
    setActiveRun(id); setSelectedRun(id); setPage('Activity'); setTemplate(null);
    let terminal = false;
    try {
      await execute(name, code, token, controller.signal, event => {
        if (event.stage === 'Done' || event.stage === 'Failed' || (event.exit_code != null && event.exit_code !== 0)) terminal = true;
        updateRun(id, run => ({ ...run,
          stage: executionStage(run.stage, event),
          exitCode: event.exit_code ?? run.exitCode,
          output: appendOutput(run.output, executionOutput(event)),
        }));
      }, template?.language ?? 'rust');
      if (!terminal) throw new Error('Stream ended without a final status. Check sandbox inventory.');
    } catch (error) {
      updateRun(id, run => ({ ...run, stage: terminal ? run.stage : error instanceof ApiError ? 'Failed' : 'Interrupted',
        output: appendOutput(run.output, `\n[console] ${error instanceof Error ? error.message : 'Execution request failed.'}\n`) }));
    } finally {
      runGuard.current = false; execution.current = null; setActiveRun(null); setRefreshKey(key => key + 1);
    }
  }

  async function stopGuest() {
    if (stopping) return;
    setStopping(true);
    try {
      await shutdown(token);
      setConfirmShutdown(false);
      setNotice('Cancellation requested. The output stream reports cleanup completion; check inventory if it disconnects.');
      setRefreshKey(key => key + 1);
    } catch (error) { setNotice(error instanceof Error ? error.message : 'Shutdown request failed.'); }
    finally { setStopping(false); }
  }

  async function copyOutput() {
    try { await navigator.clipboard.writeText(viewing?.output ?? ''); setCopied(true); setTimeout(() => setCopied(false), 2000); }
    catch { setNotice('Clipboard access is unavailable. Select and copy the output directly.'); }
  }

  function downloadOutput() {
    if (!viewing) return;
    const url = URL.createObjectURL(new Blob([viewing.output], { type: 'text/plain' }));
    const link = document.createElement('a'); link.href = url; link.download = `${viewing.name}.log`; link.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }


  function runList(items: Run[]) {
    return items.length ? <div className="run-list">{items.map(run => <Button variant="ghost" className="run-row" key={run.id} onClick={() => { setSelectedRun(run.id); navigate('Activity'); }}>
      <Badge stage={run.stage} /><span className="run-name"><strong>{run.name}</strong><span className="mono">BoxLite · KVM guest</span></span><span className="run-time">{run.started}</span><MoreHorizontal size={18} aria-hidden="true" />
    </Button>)}</div> : <div className="empty"><span className="empty-icon"><Box size={27} strokeWidth={1.4} /></span><h3>Your next sandbox starts here</h3><p>Run a Rust, Python, or Node workload to see its output in this session.</p><Button variant="outline" className="secondary" onClick={() => openCreate()}><Plus size={16} />Create sandbox</Button></div>;
  }

  return <div className="app-shell">
    {sidebar && <Button variant="ghost" className="sidebar-scrim" aria-label="Close navigation" onClick={() => setSidebar(false)} />}
    <aside className={`sidebar ${sidebar ? 'is-open' : ''}`}>
      <a className="brand" href="#" onClick={event => { event.preventDefault(); navigate('Dashboard'); }}><Box size={29} strokeWidth={1.5} /><span>Cloudlet<span className="brand-suffix"> / console</span></span></a>
      <Button variant="ghost" className="host-select" onClick={() => navigate('Settings')}><Server size={18} /><span>Local environment</span><ChevronDown size={15} /></Button>
      <nav aria-label="Main navigation">{nav.map(group => <div className="nav-group" key={group.group}>{group.group && <span className="nav-label">{group.group}</span>}{group.items.map(({ page: label, icon: Icon }) => <Button variant="ghost" key={label} className={`nav-item ${page === label ? 'active' : ''}`} aria-current={page === label ? 'page' : undefined} onClick={() => navigate(label)}><Icon size={19} strokeWidth={1.6} /><span>{label}</span>{label === 'Sandboxes' && runs.length > 0 && <span className="nav-count">{runs.length}</span>}</Button>)}</div>)}</nav>
      <div className="sidebar-bottom"><div className="local-only"><ShieldCheck size={17} /><span>Embedded BoxLite runtime</span></div><div className={`connection ${ready ? 'green' : 'amber'}`}><Wifi size={17} /><span>{runtimeLabel}</span></div><div className="sidebar-endpoint mono">{snapshot?.connection.endpoint.replace('http://', '') ?? 'Local control plane'}</div><div className="sidebar-links"><span>v0.1.0</span><a href="https://github.com/virt-do/cloudlet" target="_blank" rel="noreferrer">GitHub<ExternalLink size={12} /></a></div></div>
    </aside>

    <div className="workspace"><header className="topbar"><div className="breadcrumb"><Button variant="ghost" className="icon-button menu-button" aria-label="Open navigation" onClick={() => setSidebar(true)}><Menu size={20} /></Button><span>Workspace</span><span className="slash">/</span><strong>{page}</strong></div><div className="topbar-right"><span className="local-chip"><span className="dot" />Local</span><Button variant="ghost" size="icon-sm" className="icon-button" aria-label="Refresh connection" title="Refresh connection" onClick={() => setRefreshKey(key => key + 1)} disabled={refreshing}><RefreshCw size={16} className={refreshing ? 'spin' : ''} /></Button><Button variant="ghost" size="icon-sm" className="icon-button" aria-label="Connection help" onClick={() => navigate('Settings')}><CircleHelp size={18} /></Button></div></header>
      <main id="main-content">
        <div className="page-heading"><div><div className="eyebrow">LOCAL CONTROL PLANE</div><h1>{page}</h1></div><div className="heading-actions"><Button variant="outline" className="secondary" onClick={() => openCreate(templates[1])}><Play size={15} />Quick run</Button><Button className="primary" onClick={() => openCreate()}><Plus size={17} />Create sandbox</Button></div></div>
        {notice && <div className="notice" role="status"><Activity size={17} /><span>{notice}</span><Button variant="ghost" size="icon-sm" className="icon-button" onClick={() => setNotice('')} aria-label="Dismiss notice"><X size={16} /></Button></div>}
        {connection !== 'ready' && connection !== 'loading' && <div className="connection-banner"><Unplug size={19} /><div><strong>{runtimeLabel}</strong><p>{connection === 'locked' ? 'Enter your API token in Settings to access this environment.' : snapshot ? snapshot.connection.detail : apiError || 'Check that the Cloudlet server is running.'}</p></div><Button variant="ghost" className="text-button" onClick={() => navigate('Settings')}>{connection === 'locked' ? 'Add token' : 'Setup details'}<ArrowRight size={15} /></Button></div>}

        {page === 'Dashboard' && <>
          <section className="metrics" aria-label="Live BoxLite sandbox allocations"><Metric icon={Activity} value={ready ? String(snapshot?.capacity.running ?? 0) : "—"} label="running" tone="green" /><Metric icon={Square} value={ready ? String(snapshot?.capacity.stopped ?? 0) : "—"} label="stopped" /><Metric icon={Box} value={ready ? String(inventory.length) : "—"} label="total" /><Metric icon={Cpu} value={ready ? String(snapshot?.capacity.vcpus ?? 0) : "—"} label="vCPUs" tone="blue" /><Metric icon={Network} value={ready ? `${snapshot?.capacity.memory_mib ?? 0} MiB` : "—"} label="memory" tone="purple" /><div className="metrics-caption">Sandbox allocations<br /><span>{ready ? "Live inventory" : "not reported"}</span></div></section>
          <div className="dashboard-grid"><section><div className="section-heading"><h2>Recent sandboxes</h2><Button variant="ghost" className="text-button" onClick={() => navigate('Sandboxes')}>View all<ArrowRight size={14} /></Button></div><div className="panel">{runList(runs.slice(0, 6))}</div><p className="section-note">Recent requests from this tab. Open Sandboxes for the persistent BoxLite inventory.</p><div className="runtime-strip"><div className="runtime-icon"><ShieldCheck size={20} /></div><div><strong>One console. Isolated execution.</strong><p>React + shadcn/ui → Rust API → BoxLite → KVM guest.</p></div><span className="subtle-tag">KVM</span></div></section>
          <aside className="quickstart"><div className="section-heading"><h2>Quickstart</h2><span className="section-meta">OCI TEMPLATES</span></div><div className="panel">{templates.map(item => <div className="quickstart-row" key={item.name}><span className="template-icon"><item.icon size={20} strokeWidth={1.5} /></span><div><strong>{item.name}</strong><p>{item.description}</p></div><Button variant="outline" size="sm" className="launch" onClick={() => openCreate(item)}><Play size={13} />Run</Button></div>)}</div><div className="model-card"><div className="model-card-top"><span className="model-icon"><Zap size={21} /></span><span className="subtle-tag">LLMMAN</span></div><h3>Bring your models</h3><p>Download, run, and unload local models through llmman. Inference runs on the host; guests stay isolated.</p><Button variant="ghost" className="text-button" onClick={() => navigate('Models')}>Manage models<ArrowRight size={15} /></Button></div><p className="quiet-note"><Radio size={13} />Connection checked every 5 seconds</p></aside></div>
        </>}

        {page === 'Sandboxes' && <><RuntimeInventory snapshot={snapshot} token={token} busy={!!activeRun} refresh={() => setRefreshKey(key => key + 1)} notice={setNotice} /><div className="danger-zone"><div><h3>Execution lifecycle</h3><p>One execution at a time across clients. Completion, cancellation, and disconnected output streams trigger guest cleanup. No broker restart is needed.</p></div><Button variant="destructive" disabled={!activeRun || stopping} onClick={() => setConfirmShutdown(true)}><Square size={15} />Cancel execution</Button></div></>}

        {page === 'Templates' && <><p className="view-description">Choose Rust, Python, or Node.js. Review source before running it in a fresh OCI-backed guest.</p><div className="template-grid">{templates.map(item => <section className="panel template-card" key={item.name}><item.icon size={26} strokeWidth={1.5} /><h2>{item.name}</h2><p>{item.description}</p><pre>{item.code}</pre><Button variant="outline" className="secondary" onClick={() => openCreate(item)}><Code2 size={16} />Use template<ArrowRight size={15} /></Button></section>)}</div><p className="section-note">Fixed OCI templates only. External packages, arbitrary images, and agent installers are not configured.</p></>}

        {page === 'Activity' && <><div className="activity-layout"><aside className="panel activity-runs"><div className="panel-label">SESSION EXECUTIONS</div>{runs.length ? runs.map(run => <Button variant="ghost" className={`activity-item ${selectedRun === run.id ? 'selected' : ''}`} onClick={() => setSelectedRun(run.id)} key={run.id}><span><strong>{run.name}</strong><small>{run.started}</small></span><Badge stage={run.stage} /></Button>) : <p className="padded-muted">No execution requests yet.</p>}</aside><section className="panel output-panel"><div className="output-heading"><div><Terminal size={18} /><strong>{viewing?.name ?? 'Execution output'}</strong>{viewing && <Badge stage={viewing.stage} />}</div><div><Button variant="ghost" size="icon-sm" className="icon-button" aria-label="Copy output" disabled={!viewing} onClick={() => void copyOutput()}>{copied ? <Check size={16} /> : <Copy size={16} />}</Button><Button variant="ghost" size="icon-sm" className="icon-button" aria-label="Download output" disabled={!viewing} onClick={downloadOutput}><ArrowDownToLine size={16} /></Button></div></div><pre className="terminal-output" tabIndex={0} aria-label="Execution output">{viewing ? viewing.output || 'Waiting for build output…' : 'Run a workload to stream build logs, stdout, and stderr here.'}{viewing?.id === activeRun && <span className="cursor">▌</span>}</pre><div className="output-footer"><span>{viewing ? `Started ${viewing.started}` : 'No workload selected'}</span><span>{viewing?.exitCode != null ? `Exit code ${viewing.exitCode}` : 'HTTP / SSE'}</span></div></section></div><p className="section-note">Closing this page disconnects the stream and requests guest cleanup. Output is kept in memory, limited to the last 100,000 characters per request.</p></>}

        {page === 'Models' && <Models token={token} />}

        {page === 'Settings' && <div className="details-grid"><section className="panel info-panel"><h2>Connection</h2><dl><dt>HTTP control plane</dt><dd className="mono">{typeof window === 'undefined' ? 'Same origin' : window.location.origin}</dd><dt>Primary runtime</dt><dd className="mono">{snapshot?.connection.endpoint ?? 'Unavailable until authenticated'}</dd><dt>Status</dt><dd><span className={ready ? 'green' : 'amber'}>{runtimeLabel}</span></dd></dl><form className="token-form" onSubmit={event => { event.preventDefault(); setToken(tokenDraft); setTokenDraft(''); setRefreshKey(key => key + 1); setNotice('API credentials updated for this tab.'); }}><Label htmlFor="api-token">API bearer token <span className="optional">optional for local API</span></Label><Input id="api-token" type="password" autoComplete="off" value={tokenDraft} onChange={event => setTokenDraft(event.target.value)} placeholder={token ? 'A token is set for this tab' : 'CLOUDLET_API_AUTH_TOKEN'} pattern="[\x21-\x7e]{16,}" /><p>Held in memory for this tab only. Never enter a model provider key here.</p><div className="form-actions"><Button className="primary" type="submit">Apply token</Button>{token && <Button variant="outline" className="secondary" type="button" onClick={() => { setToken(''); setTokenDraft(''); }}>Clear token</Button>}</div></form>{apiError && <p className="error-detail">{apiError}</p>}</section><section className="panel info-panel"><h2>BoxLite host setup</h2><p>Run Cloudlet as your normal user on a Linux host with read/write access to KVM. No separate broker, TAP bridge, or CAP_NET_ADMIN is required for these templates.</p><code className="command">./target/release/cloudlet dashboard</code><p>{snapshot?.connection.detail}</p><code className="command">ls -l /dev/kvm<br />id -nG<br />test -r /dev/kvm &amp;&amp; test -w /dev/kvm</code><div className="info-callout"><TriangleAlert size={19} /><span>A restricted container may hide KVM. Fix host access and restart Cloudlet; do not disable sandbox protections just to make readiness green.</span></div><h3>Execution policy</h3><ul className="limits"><li>One active execution across clients</li><li>2 vCPUs, 1 GiB RAM, 8 GiB requested disk</li><li>60-second command / 10-minute cold-start limit</li><li>Guest user 65534; no network or host mounts</li><li>Retained disks, maximum 100 sandboxes</li><li>1 MiB output cap; slow consumers cancel execution</li></ul></section></div>}
        <footer className="page-footer"><span><span className="dot" />Cloudlet control plane</span><span>shadcn/ui · Rust · BoxLite</span></footer>
      </main>
    </div>

    {template && <Dialog title="Create sandbox" close={() => setTemplate(null)}><form onSubmit={event => void startRun(event)}><p className="dialog-description">Build and execute source inside a BoxLite KVM guest.</p><Label htmlFor="workload-name">Sandbox name</Label><Input autoFocus id="workload-name" required maxLength={64} pattern="[a-z0-9-]+" value={name} onChange={event => setName(event.target.value)} /><div className="editor-label"><Label htmlFor="source">Source code</Label><span>{template.language} · source</span></div><Textarea id="source" className="code-editor" value={code} onChange={event => setCode(event.target.value)} required spellCheck={false} /><p className="dialog-hint">First use downloads the OCI image. Templates have no guest network or host mounts, 2 vCPUs, 1 GiB RAM, and a 60-second execution limit.</p>{!ready && <p className="form-warning"><Unplug size={15} />Resolve BoxLite runtime setup before running.</p>}{activeRun && <p className="form-warning">An execution request is already in progress.</p>}{formError && <p role="alert" className="red">{formError}</p>}<div className="dialog-actions"><Button variant="outline" className="secondary" type="button" onClick={() => setTemplate(null)}>Cancel</Button><Button className="primary" type="submit" disabled={!ready || !!activeRun}><Play size={15} />Build & run</Button></div></form></Dialog>}
    <AlertDialog open={confirmShutdown} onOpenChange={open => { if (!stopping) setConfirmShutdown(open); }}><AlertDialogContent><AlertDialogHeader><AlertDialogTitle>Cancel active execution?</AlertDialogTitle><AlertDialogDescription>Active guest processes will be interrupted. The stream reports when guest cleanup finishes. No broker restart is needed.</AlertDialogDescription></AlertDialogHeader><AlertDialogFooter><AlertDialogCancel disabled={stopping}>Keep running</AlertDialogCancel><AlertDialogAction variant="destructive" disabled={stopping} onClick={event => { event.preventDefault(); void stopGuest(); }}>{stopping ? <LoaderCircle className="spin" /> : <Square />}Cancel execution</AlertDialogAction></AlertDialogFooter></AlertDialogContent></AlertDialog>
  </div>;
}

function Metric({ icon: Icon, value, label, tone = 'muted' }: { icon: LucideIcon; value: string; label: string; tone?: string }) {
  return <div className="metric"><Icon size={20} className={tone} strokeWidth={1.7} /><strong>{value}</strong><span>{label}</span></div>;
}
