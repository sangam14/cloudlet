import { useEffect, useState, type FormEvent } from 'react';
import { Boxes, Download, LoaderCircle, Play, RefreshCw, ShieldCheck, Square, Terminal, Zap } from 'lucide-react';
import { models, modelOperation, type ModelAction, type ModelSnapshot } from './api';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Card } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { Label } from '@/components/ui/label';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { AlertDialog, AlertDialogContent, AlertDialogHeader, AlertDialogTitle, AlertDialogDescription, AlertDialogFooter, AlertDialogAction, AlertDialogCancel } from '@/components/ui/alert-dialog';

export function Models({ token }: { token: string }) {
  const [snapshot, setSnapshot] = useState<ModelSnapshot | null>(null);
  const [error, setError] = useState('');
  const [connectionError, setConnectionError] = useState('');
  const [refreshKey, setRefreshKey] = useState(0);
  const [model, setModel] = useState('');
  const [reference, setReference] = useState('');
  const [prompt, setPrompt] = useState('Describe what a microVM is in two sentences.');
  const [submitting, setSubmitting] = useState(false);
  const [confirmDownload, setConfirmDownload] = useState(false);
  const operation = snapshot?.operation;
  const busy = submitting || operation?.status === 'running' || operation?.status === 'unconfirmed';
  const ready = snapshot?.ready === true;
  const selected = snapshot?.models.find(item => item.name === model);

  useEffect(() => {
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      try {
        const next = await models(token, AbortSignal.any([controller.signal, AbortSignal.timeout(5000)]));
        if (!controller.signal.aborted) {
          setSnapshot(next); setConnectionError('');
          setModel(current => next.models.some(item => item.name === current) ? current : next.models[0]?.name ?? '');
        }
      } catch (reason) {
        if (!controller.signal.aborted) { setSnapshot(null); setConnectionError(reason instanceof Error ? reason.message : 'Model service unavailable'); }
      } finally { if (!controller.signal.aborted) timer = setTimeout(poll, 2000); }
    }
    void poll();
    return () => { controller.abort(); clearTimeout(timer); };
  }, [token, refreshKey]);

  async function perform(action: ModelAction, name: string) {
    if (busy || !ready) return;
    setSubmitting(true); setError('');
    try {
      await modelOperation(action, name, action === 'run' ? prompt : '', token);
      setConfirmDownload(false);
      // Disable controls immediately; do not leave a gap before the next poll.
      setSnapshot(current => current && ({ ...current, operation: { action, model: name, status: 'running', detail: 'Request accepted', output: '' } }));
      setRefreshKey(key => key + 1);
    } catch (reason) { setError(reason instanceof Error ? reason.message : 'Model request failed'); }
    finally { setSubmitting(false); }
  }

  function run(event: FormEvent) { event.preventDefault(); void perform('run', model); }

  return <div className="model-workspace">
    <Card className="panel model-service"><div className="model-service-title"><span className="model-icon"><Zap size={23} /></span><div><h2>llmman</h2><p className="mono">{snapshot?.endpoint ?? 'Local inference service'}</p></div><Badge variant="outline" className={ready ? 'green' : 'amber'}>{ready ? 'Connected' : snapshot || error ? 'Needs setup' : 'Checking'}</Badge></div><Button variant="outline" size="sm" onClick={() => setRefreshKey(key => key + 1)} aria-label="Refresh models"><RefreshCw size={15} />Refresh</Button></Card>
    {(error || connectionError) && <p className="error-detail" role="alert">{error || connectionError}</p>}
    {snapshot && !ready && <Card className="panel info-panel gap-3 p-6"><h3>Start the local model service</h3><p>{snapshot.detail}</p><code className="command">LLMMAN_HOST=127.0.0.1:17434 LLMMAN_NOHISTORY=1 llmman serve</code><p>Use tools/bin/llmman if installed in this checkout. Cloudlet connects to an existing daemon. Depending on its version, llmman may download an inference engine when starting or first running a model.</p></Card>}
    <div className="model-columns"><div className="model-stack">
      <Card className="panel model-inventory"><div className="section-heading"><h2><Boxes size={19} />Model inventory</h2><span className="section-meta">{snapshot?.models.length ?? 0} MODELS</span></div>
        {snapshot?.models.length ? snapshot.models.map(item => <Button key={item.name} variant="ghost" className={`model-row ${model === item.name ? 'selected' : ''}`} onClick={() => setModel(item.name)}><span><strong className="mono">{item.name}</strong><small>{(item.size_bytes / 1024 ** 3).toFixed(2)} GiB {item.stored ? 'on disk' : 'loaded · missing from store'}</small></span><Badge variant="outline" className={item.loaded ? 'green' : ''}>{item.loaded ? 'Loaded' : 'On disk'}</Badge></Button>) : <div className="model-empty"><Boxes size={28} /><h3>{ready ? 'No models downloaded' : 'Inventory unavailable'}</h3><p>{ready ? 'Download a model below, or attach a llmman daemon that already has your models.' : 'Connect llmman to see real model inventory.'}</p></div>}
      </Card>
      <Card className="panel info-panel gap-4 p-6"><h2>Download a model</h2><form className="model-form" onSubmit={event => { event.preventDefault(); setConfirmDownload(true); }}><Label htmlFor="model-reference">OCI or Hugging Face reference</Label><Input id="model-reference" required maxLength={256} value={reference} onChange={event => setReference(event.target.value)} placeholder="hf.co/owner/model-GGUF:Q4_K_M" /><p>Downloads use host network and disk space. Size and hardware requirements depend on the model.</p><Button variant="outline" type="submit" disabled={!ready || busy || !reference.trim()}><Download size={16} />Review download</Button></form></Card>
    </div><div className="model-stack">
      <Card className="panel info-panel gap-4 p-6"><h2>Run a model</h2><form className="model-form" onSubmit={run}><Label htmlFor="active-model">Downloaded model</Label><Select value={selected ? model : ''} onValueChange={setModel} disabled={!ready || !snapshot?.models.length || busy}><SelectTrigger id="active-model" className="w-full min-w-0"><SelectValue placeholder="Select a downloaded model" /></SelectTrigger><SelectContent>{snapshot?.models.map(item => <SelectItem key={item.name} value={item.name}>{item.name}</SelectItem>)}</SelectContent></Select><Label htmlFor="model-prompt">Prompt</Label><Textarea id="model-prompt" value={prompt} onChange={event => setPrompt(event.target.value)} required maxLength={8192} rows={5} /><p>Up to 256 output tokens. First run may download llmman’s inference engine. The model stays loaded for 5 minutes while idle.</p><div className="form-actions"><Button type="submit" disabled={!ready || busy || !selected?.stored || !prompt.trim()}>{busy && operation?.action === 'run' ? <LoaderCircle className="spin" size={16} /> : <Play size={16} />}Run prompt</Button><Button type="button" variant="outline" disabled={!ready || busy || !selected?.loaded} onClick={() => void perform('unload', model)}><Square size={16} />Unload</Button></div></form></Card>
      <Card className="panel model-response"><div className="output-heading"><div><Terminal size={18} /><strong>Model output</strong></div>{operation && <Badge variant="outline" className={(operation.status === 'failed' || operation.status === 'unconfirmed') ? 'red' : operation.status === 'running' ? 'amber' : 'green'}>{operation.status === 'running' && <LoaderCircle size={13} className="spin" />}{operation.status}</Badge>}</div><div role="status" className="model-operation"><p>{operation?.detail ?? 'Run a prompt to see the model’s reply here.'}</p>{operation && <small className="mono">{operation.action} · {operation.model}</small>}</div>{operation?.output && <pre className="model-output" tabIndex={0}>{operation.output}</pre>}</Card>
    </div></div>
    <div className="info-callout"><ShieldCheck size={20} /><span>Inference runs on the host, not inside a BoxLite sandbox. Guest networking stays disabled. Cloudlet forwards no browser tokens or provider keys to llmman. Only the latest operation is retained in API memory.</span></div>
    <AlertDialog open={confirmDownload} onOpenChange={setConfirmDownload}><AlertDialogContent><AlertDialogHeader><AlertDialogTitle>Download this model?</AlertDialogTitle><AlertDialogDescription>llmman will fetch {reference} using host bandwidth and disk space. Model weights can be several gigabytes. The download continues if you leave this page; Cloudlet does not expose cancellation.</AlertDialogDescription></AlertDialogHeader><AlertDialogFooter><AlertDialogCancel disabled={submitting}>Cancel</AlertDialogCancel><AlertDialogAction disabled={busy} onClick={event => { event.preventDefault(); void perform('pull', reference.trim()); }}>Download model</AlertDialogAction></AlertDialogFooter></AlertDialogContent></AlertDialog>
  </div>;
}
