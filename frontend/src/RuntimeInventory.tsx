import { useState } from 'react';
import { Box, LoaderCircle, Search, Square, Trash2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Card } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { AlertDialog, AlertDialogContent, AlertDialogDescription, AlertDialogHeader, AlertDialogTitle, AlertDialogFooter, AlertDialogAction, AlertDialogCancel } from '@/components/ui/alert-dialog';
import { sandboxAction, type Snapshot } from './api';

export function RuntimeInventory({ snapshot, token, busy, refresh, notice }: {
  snapshot: Snapshot | null; token: string; busy: boolean; refresh: () => void; notice: (message: string) => void;
}) {
  const [search, setSearch] = useState('');
  const [filter, setFilter] = useState('all');
  const [pending, setPending] = useState<{ id: string; name: string; action: 'stop' | 'remove' } | null>(null);
  const [working, setWorking] = useState(false);
  const [error, setError] = useState('');
  const items = (snapshot?.workloads ?? []).filter(box => box.name.toLowerCase().includes(search.toLowerCase()) && (filter === 'all' || box.status === filter));
  const ready = snapshot?.connection.status === 'ready';
  async function apply() {
    if (!pending || working) return;
    setWorking(true); setError('');
    try {
      await sandboxAction(pending.id, pending.action, token);
      notice(pending.action === 'stop' ? 'Sandbox stopped.' : 'Stopped sandbox and guest disk removed permanently.');
      setPending(null); refresh();
    } catch (error) { setError(error instanceof Error ? error.message : 'Lifecycle action failed.'); }
    finally { setWorking(false); }
  }
  return <>
    <div className="toolbar"><div className="search"><Search size={17} /><Input aria-label="Search runtime inventory" placeholder="Search sandboxes…" value={search} onChange={event => setSearch(event.target.value)} /></div><Select value={filter} onValueChange={setFilter}><SelectTrigger className="w-40" aria-label="Filter runtime inventory"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="all">All statuses</SelectItem><SelectItem value="running">Running</SelectItem><SelectItem value="stopped">Stopped</SelectItem></SelectContent></Select><span className="section-meta">{items.length} SANDBOXES</span></div>
    <Card className="panel gap-0 py-0">{items.length ? items.map(box => <div className="run-row" key={box.id}>
      <Badge variant="outline" className={box.status === 'running' ? 'text-emerald-300 border-emerald-900 bg-emerald-950/50' : 'text-zinc-300 bg-zinc-800'}>{box.status}</Badge><span className="run-name"><strong>{box.name}</strong><span className="mono">{box.runtime}</span><span className="mono">{new Date(box.updated_at).toLocaleString()}</span></span>
      <Button variant="ghost" size="icon-sm" aria-label={`Stop ${box.name}`} title="Stop sandbox" disabled={!ready || busy || working} onClick={() => { setError(''); setPending({ ...box, action: 'stop' }); }}><Square /></Button>
      <Button variant="ghost" size="icon-sm" aria-label={`Remove ${box.name}`} title="Remove stopped sandbox" disabled={!ready || busy || working || box.status !== 'stopped'} onClick={() => { setError(''); setPending({ ...box, action: 'remove' }); }}><Trash2 /></Button>
    </div>) : <div className="empty"><span className="empty-icon"><Box size={27} /></span><h3>{search || filter !== 'all' ? 'No matching sandboxes' : 'No retained sandboxes'}</h3><p>{ready ? 'Create a sandbox to populate the persistent runtime inventory.' : 'Connect the BoxLite runtime to load persistent inventory.'}</p></div>}</Card>
    <p className="section-note">Real BoxLite inventory, retained across reloads. Stop preserves guest disks; remove permanently deletes them. Completed executions stop automatically.</p>
    <AlertDialog open={!!pending} onOpenChange={open => { if (!open && !working) setPending(null); }}><AlertDialogContent><AlertDialogHeader><AlertDialogTitle>{pending?.action === 'remove' ? 'Remove sandbox permanently?' : 'Stop this sandbox?'}</AlertDialogTitle><AlertDialogDescription>{pending?.name}. {pending?.action === 'remove' ? 'Its stopped guest disk will be deleted. This cannot be undone.' : 'Active guest processes will be interrupted. The guest disk is retained.'}</AlertDialogDescription></AlertDialogHeader>{error && <p role="alert" className="text-sm text-red-300 break-all">{error}</p>}<AlertDialogFooter><AlertDialogCancel disabled={working}>Cancel</AlertDialogCancel><AlertDialogAction variant="destructive" disabled={working} onClick={event => { event.preventDefault(); void apply(); }}>{working && <LoaderCircle className="spin" />}{pending?.action === 'remove' ? 'Remove sandbox' : 'Stop sandbox'}</AlertDialogAction></AlertDialogFooter></AlertDialogContent></AlertDialog>
  </>;
}
