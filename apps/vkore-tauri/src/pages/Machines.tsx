import { Cpu, FileDigit, HardDrive, MemoryStick, Play, Plus, Server, Square } from 'lucide-react';
import { useState } from 'react';
import { Link } from 'react-router-dom';
import type { Machine } from '../api';
import { mib } from '../format';
import { useStore } from '../store';
import { Empty, Pill, Stages, isActive } from '../ui';

export function Machines() {
  const { machines } = useStore();
  if (!machines) return <p className="muted">Reading machines…</p>;
  return (
    <>
      <header className="page-head">
        <div>
          <p className="eyebrow">Machines</p>
          <h1>Your machines</h1>
        </div>
      </header>
      {machines.length === 0 ? (
        <div className="card">
          <Empty icon={Server} title="No machines yet">
            Create one with <code>vkore machine create</code>, then start it from here.
          </Empty>
        </div>
      ) : (
        <div className="grid">
          {machines.map((machine) => <MachineCard key={machine.name} machine={machine} />)}
        </div>
      )}
      <p className="faint small footnote"><Plus size={13} /> Creating and editing machines stays in the vkore command and the iced desktop for now.</p>
    </>
  );
}

export function StartStop({ machine }: { machine: Machine }) {
  const { start, stop } = useStore();
  const [error, setError] = useState<string | null>(null);
  const active = isActive(machine.run.phase);
  const stopping = machine.run.phase === 'stopping';
  const act = async () => setError(await (active ? stop : start)(machine.name));
  return (
    <>
      <button className={active ? '' : 'primary'} onClick={act} disabled={stopping}>
        {active ? <Square size={15} /> : <Play size={15} />}
        {stopping ? 'Stopping…' : active ? 'Stop' : 'Start'}
      </button>
      {error && <span className="inline-error" role="alert">{error}</span>}
    </>
  );
}

function MachineCard({ machine }: { machine: Machine }) {
  const { run } = machine;
  return (
    <article className={`card machine ${isActive(run.phase) ? 'live' : ''}`}>
      <header className="machine-head">
        <span className="plate big"><Server size={22} /></span>
        <div className="machine-title">
          <h2><Link to={`/machines/${encodeURIComponent(machine.name)}`}>{machine.name}</Link></h2>
          <span className="faint small">{machine.iso}</span>
        </div>
        <Pill phase={run.phase} />
      </header>
      <ul className="chips">
        <li><Cpu size={14} />{machine.cpus} CPUs</li>
        <li><MemoryStick size={14} />{mib(machine.memoryMib)}</li>
        <li><HardDrive size={14} />{mib(machine.diskMib)}</li>
        {machine.isoSha256 && <li className="mono"><FileDigit size={14} />{machine.isoSha256}</li>}
      </ul>
      {isActive(run.phase) && <Stages machine={machine} />}
      {run.phase === 'failed' && run.message && <p className="notice bad">{run.message}</p>}
      <footer className="machine-foot">
        <StartStop machine={machine} />
        <Link to={`/machines/${encodeURIComponent(machine.name)}`} className="link">Details</Link>
      </footer>
    </article>
  );
}
