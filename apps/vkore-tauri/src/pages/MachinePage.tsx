import { ArrowLeft, Cpu, FileDigit, FolderGit2, HardDrive, MemoryStick, Monitor, ScrollText, Server, Zap } from 'lucide-react';
import { useEffect, useRef } from 'react';
import { Link, useParams } from 'react-router-dom';
import { mib, seconds, share } from '../format';
import { useStore } from '../store';
import { MpMark } from '../MpMark';
import { Empty, Meter, Pill, Ring, Stages, isActive } from '../ui';
import { StartStop } from './Machines';

export function MachinePage() {
  const { name = '' } = useParams();
  const { machines, host, logs, loadLog } = useStore();
  const machine = machines?.find((entry) => entry.name === name);
  const lines = logs[name] ?? [];
  const log = useRef<HTMLOListElement>(null);

  useEffect(() => {
    void loadLog(name);
  }, [name, loadLog]);

  // Follow the newest line, unless the reader has scrolled up to read.
  useEffect(() => {
    const element = log.current;
    if (element && element.scrollHeight - element.scrollTop - element.clientHeight < 80) {
      element.scrollTop = element.scrollHeight;
    }
  }, [lines.length]);

  if (!machines || !host) return <p className="muted">Reading machine…</p>;
  if (!machine) {
    return (
      <div className="card">
        <Empty icon={Server} title="No such machine">
          <Link to="/machines" className="link">Back to machines</Link>
        </Empty>
      </div>
    );
  }

  const active = isActive(machine.run.phase);
  const cpuShare = share(machine.cpus, host.logicalCpus);
  return (
    <>
      <Link to="/machines" className="back"><ArrowLeft size={15} /> Machines</Link>
      <header className="page-head">
        <div className="machine-title-row">
          <span className="plate big"><Server size={24} /></span>
          <div>
            <p className="eyebrow">Machine</p>
            <h1>{machine.name}</h1>
          </div>
          <Pill phase={machine.run.phase} />
        </div>
        <div className="actions"><StartStop machine={machine} /></div>
      </header>

      {machine.run.phase === 'failed' && machine.run.message && <p className="notice bad">{machine.run.message}</p>}
      {machine.run.phase === 'idle' && machine.run.message && <p className="notice">{machine.run.message}</p>}

      <section className="card">
        <header className="card-head">
          <h2>Boot progress</h2>
          <span className="faint small">
            {active && machine.run.stage ? `${machine.run.stage} at ${seconds(machine.run.uptimeMs)}` : active ? 'Preparing…' : 'Start the machine to boot it'}
          </span>
        </header>
        <div className="boot">
          <MpMark
            size={84}
            motion={['starting', 'booting', 'stopping'].includes(machine.run.phase) ? 'busy' : 'still'}
          />
          <Stages machine={machine} />
        </div>
      </section>

      <div className="columns">
        <section className="card">
          <header className="card-head"><h2>Resources</h2><span className="faint small">against this host</span></header>
          <div className="gauges">
            <Ring fraction={cpuShare} value={String(machine.cpus)} caption={`of ${host.logicalCpus} CPUs`} />
            <Ring fraction={share(machine.memoryMib, host.availableMemoryMib)} value={mib(machine.memoryMib)} caption="memory" />
          </div>
          <Meter used={machine.cpus} of={host.logicalCpus} label="CPU threads" detail={`${machine.cpus} of ${host.logicalCpus}`} />
          <Meter used={machine.memoryMib} of={host.availableMemoryMib} label="Memory" detail={`${mib(machine.memoryMib)} of ${mib(host.availableMemoryMib)} available`} />
          <Meter used={machine.diskMib} of={host.storageAvailableMib} label="Disk" detail={`${mib(machine.diskMib)} of ${mib(host.storageAvailableMib)} free`} />
        </section>

        <section className="card">
          <header className="card-head"><h2>Configuration</h2></header>
          <dl className="facts">
            <div><dt><FileDigit size={16} /> Image</dt><dd>{machine.iso}{machine.isoSha256 && <span className="faint mono"> · {machine.isoSha256}</span>}</dd></div>
            <div><dt><Cpu size={16} /> CPUs</dt><dd>{machine.cpus}</dd></div>
            <div><dt><MemoryStick size={16} /> Memory</dt><dd>{mib(machine.memoryMib)}</dd></div>
            <div><dt><HardDrive size={16} /> Disk</dt><dd>{mib(machine.diskMib)}</dd></div>
            <div><dt><Monitor size={16} /> Display</dt><dd>{machine.framebuffer[0]} × {machine.framebuffer[1]}</dd></div>
            <div><dt><Zap size={16} /> Acceleration</dt><dd>{machine.accel === 'auto' ? `Automatic (${host.accel.toUpperCase()})` : machine.accel.toUpperCase()}</dd></div>
            <div><dt><FolderGit2 size={16} /> Project</dt><dd>{machine.workspace ? <>{machine.workspace.name} <span className="pill quiet">write-back {machine.workspace.writeback}</span></> : <span className="faint">None</span>}</dd></div>
          </dl>
        </section>
      </div>

      <section className="card">
        <header className="card-head"><h2>Events</h2><span className="faint small">{lines.length} {lines.length === 1 ? 'line' : 'lines'}</span></header>
        {lines.length === 0 ? (
          <Empty icon={ScrollText} title="No events yet">A boot writes its steps here as they happen.</Empty>
        ) : (
          <ol className="log" ref={log} aria-label="Machine events">
            {lines.map((line, index) => (
              <li key={`${index}-${line.atMs}`}>
                <time className="faint">{seconds(line.atMs)}</time>
                <span>{line.text}</span>
              </li>
            ))}
          </ol>
        )}
      </section>
    </>
  );
}
