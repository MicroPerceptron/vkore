import { Boxes, Cpu, HardDrive, MemoryStick, Play, Server, Zap } from 'lucide-react';
import { Link } from 'react-router-dom';
import { MpMark } from '../MpMark';
import { mib, plural, share } from '../format';
import { useStore } from '../store';
import { Empty, Meter, Pill, Ring, isActive } from '../ui';

const ACCEL = { kvm: 'KVM', hvf: 'Hypervisor.framework', tcg: 'Software (TCG)' } as const;

export function Dashboard() {
  const { host, machines } = useStore();
  if (!host || !machines) return <p className="muted">Reading this host…</p>;

  const running = machines.filter((machine) => isActive(machine.run.phase));
  const reservedMemory = running.reduce((sum, machine) => sum + machine.memoryMib, 0);
  const reservedCpus = running.reduce((sum, machine) => sum + machine.cpus, 0);
  const recent = [...machines].sort((a, b) => Number(isActive(b.run.phase)) - Number(isActive(a.run.phase))).slice(0, 5);

  return (
    <>
      <section className="hero card">
        <MpMark size={96} motion="intro" title="MicroPerceptron" />
        <div className="hero-text">
          <p className="eyebrow">Overview</p>
          <h1>
            {running.length === 0
              ? 'All quiet on this host.'
              : `${plural(running.length, 'machine')} running.`}
          </h1>
          <p className="lead">
            {machines.length === 0
              ? 'No machines yet. Create one with the vkore command, then start it here.'
              : `${plural(machines.length, 'machine')} configured · ${mib(reservedMemory)} of ${mib(host.availableMemoryMib)} memory reserved.`}
          </p>
        </div>
        <Ring
          fraction={share(reservedMemory, host.availableMemoryMib)}
          value={`${Math.round(share(reservedMemory, host.availableMemoryMib) * 100)}%`}
          caption="memory"
        />
      </section>

      <section className="stats" aria-label="Summary">
        <Stat icon={Boxes} label="Machines" value={String(machines.length)} note="configured" />
        <Stat icon={Play} label="Running" value={`${running.length}`} note={`of ${host.maxRunning} at once`} />
        <Stat icon={Cpu} label="CPUs reserved" value={String(reservedCpus)} note={`of ${host.logicalCpus} threads`} />
        <Stat icon={MemoryStick} label="Memory reserved" value={mib(reservedMemory)} note={`of ${mib(host.memoryMib)}`} />
      </section>

      <div className="columns">
        <section className="card">
          <header className="card-head">
            <h2>Machines</h2>
            <Link to="/machines" className="link">View all</Link>
          </header>
          {recent.length === 0 ? (
            <Empty icon={Boxes} title="Nothing here yet">Machines you create appear here.</Empty>
          ) : (
            <ul className="rows">
              {recent.map((machine) => (
                <li key={machine.name}>
                  <Link to={`/machines/${encodeURIComponent(machine.name)}`} className="row">
                    <span className="plate"><Server size={18} /></span>
                    <span className="row-main">
                      <strong>{machine.name}</strong>
                      <span className="faint">{plural(machine.cpus, 'CPU')} · {mib(machine.memoryMib)} · {mib(machine.diskMib)} disk</span>
                    </span>
                    <Pill phase={machine.run.phase} />
                  </Link>
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className="card">
          <header className="card-head"><h2>This host</h2></header>
          <dl className="facts">
            <div><dt><Cpu size={16} /> Processor</dt><dd>{host.cpuName}</dd></div>
            <div><dt><Zap size={16} /> Acceleration</dt><dd><span className={`pill ${host.accel === 'tcg' ? 'warn' : 'good'}`}>{ACCEL[host.accel]}</span></dd></div>
            <div><dt><Server size={16} /> QEMU</dt><dd>{host.qemu ?? <span className="pill bad">Not found</span>}</dd></div>
            <div><dt><HardDrive size={16} /> Storage free</dt><dd>{mib(host.storageAvailableMib)}</dd></div>
          </dl>
          <Meter used={host.memoryMib - host.availableMemoryMib} of={host.memoryMib} label="Host memory in use" detail={`${mib(host.memoryMib - host.availableMemoryMib)} of ${mib(host.memoryMib)}`} />
        </section>
      </div>
    </>
  );
}

function Stat({ icon: Icon, label, value, note }: { icon: typeof Cpu; label: string; value: string; note: string }) {
  return (
    <div className="stat card tight">
      <span className="plate"><Icon size={18} /></span>
      <div>
        <div className="stat-value">{value}</div>
        <div className="stat-label">{label}</div>
        <div className="faint small">{note}</div>
      </div>
    </div>
  );
}
