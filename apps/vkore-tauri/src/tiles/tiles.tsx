import { Cpu, FileDigit, FolderGit2, HardDrive, MemoryStick, Monitor, ScrollText, Server, SlidersHorizontal, SquareTerminal, Zap, type LucideIcon } from 'lucide-react';
import { useEffect, useRef, type FC } from 'react';
import { Console } from '../Console';
import { mib, seconds, share } from '../format';
import { MpMark } from '../MpMark';
import { HardwareEditor } from '../pages/Hardware';
import { VolumeBrowser } from '../pages/Volumes';
import { useStore } from '../store';
import { Empty, Meter, Ring, Stages, isActive } from '../ui';

/** What a workspace can show. Each kind is one tile, opened at most once. */
export type TileKind = 'console' | 'boot' | 'events' | 'resources' | 'configuration' | 'volume' | 'hardware';

function useMachine(name: string) {
  const { machines, host } = useStore();
  return { machine: machines?.find((entry) => entry.name === name), host };
}

function ConsoleTile({ name }: { name: string }) {
  const { consoles } = useStore();
  const { machine } = useMachine(name);
  if (!machine) return <Empty icon={Server} title="No such machine" />;
  return <Console name={name} screen={consoles[name]} running={isActive(machine.run.phase)} fill />;
}

function BootTile({ name }: { name: string }) {
  const { machine } = useMachine(name);
  if (!machine) return null;
  const { phase, stage, uptimeMs } = machine.run;
  const active = isActive(phase);
  return (
    <div className="boot-tile">
      <div className="boot-head">
        <MpMark size={72} motion={['starting', 'booting', 'stopping'].includes(phase) ? 'busy' : 'still'} />
        <span className="faint small">
          {active && stage ? `${stage} at ${seconds(uptimeMs)}` : active ? 'Preparing…' : 'Start the machine to boot it'}
        </span>
      </div>
      <Stages machine={machine} vertical />
    </div>
  );
}

function EventsTile({ name }: { name: string }) {
  const { logs, loadLog } = useStore();
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
  if (lines.length === 0) return <Empty icon={ScrollText} title="No events yet">A boot writes its steps here as they happen.</Empty>;
  return (
    <ol className="log flat" ref={log} aria-label="Machine events">
      {lines.map((line, index) => (
        <li key={`${index}-${line.atMs}`}>
          <time className="faint">{seconds(line.atMs)}</time>
          <span>{line.text}</span>
        </li>
      ))}
    </ol>
  );
}

function ResourcesTile({ name }: { name: string }) {
  const { machine, host } = useMachine(name);
  if (!machine || !host) return null;
  return (
    <>
      <div className="gauges">
        <Ring fraction={share(machine.cpus, host.logicalCpus)} value={String(machine.cpus)} caption={`of ${host.logicalCpus} CPUs`} />
        <Ring fraction={share(machine.memoryMib, host.availableMemoryMib)} value={mib(machine.memoryMib)} caption="memory" />
      </div>
      <Meter used={machine.cpus} of={host.logicalCpus} label="CPU threads" detail={`${machine.cpus} of ${host.logicalCpus}`} />
      <Meter used={machine.memoryMib} of={host.availableMemoryMib} label="Memory" detail={`${mib(machine.memoryMib)} of ${mib(host.availableMemoryMib)} available`} />
      <Meter used={machine.diskMib} of={host.storageAvailableMib} label="Disk" detail={`${mib(machine.diskMib)} of ${mib(host.storageAvailableMib)} free`} />
    </>
  );
}

function ConfigurationTile({ name }: { name: string }) {
  const { machine, host } = useMachine(name);
  if (!machine || !host) return null;
  return (
    <dl className="facts">
      <div><dt><FileDigit size={16} /> Image</dt><dd>{machine.iso}{machine.isoSha256 && <span className="faint mono"> · {machine.isoSha256}</span>}</dd></div>
      <div><dt><Cpu size={16} /> CPUs</dt><dd>{machine.cpus}</dd></div>
      <div><dt><MemoryStick size={16} /> Memory</dt><dd>{mib(machine.memoryMib)}</dd></div>
      <div><dt><HardDrive size={16} /> Disk</dt><dd>{mib(machine.diskMib)}</dd></div>
      <div><dt><Monitor size={16} /> Display</dt><dd>{machine.framebuffer[0]} × {machine.framebuffer[1]}</dd></div>
      <div><dt><Zap size={16} /> Acceleration</dt><dd>{machine.accel === 'auto' ? `Automatic (${host.accel.toUpperCase()})` : machine.accel.toUpperCase()}</dd></div>
      <div><dt><FolderGit2 size={16} /> Project</dt><dd>{machine.workspace ? <>{machine.workspace.name} <span className="pill quiet">write-back {machine.workspace.writeback}</span></> : <span className="faint">None</span>}</dd></div>
    </dl>
  );
}

export const TILES: Record<TileKind, { title: string; Icon: LucideIcon; Body: FC<{ name: string }>; flush?: boolean }> = {
  console: { title: 'Console', Icon: SquareTerminal, Body: ConsoleTile, flush: true },
  boot: { title: 'Boot', Icon: Server, Body: BootTile },
  events: { title: 'Events', Icon: ScrollText, Body: EventsTile, flush: true },
  resources: { title: 'Resources', Icon: Cpu, Body: ResourcesTile },
  configuration: { title: 'Configuration', Icon: FileDigit, Body: ConfigurationTile },
  volume: { title: 'Volume', Icon: HardDrive, Body: ({ name }) => <VolumeBrowser name={name} /> },
  hardware: { title: 'Hardware', Icon: SlidersHorizontal, Body: ({ name }) => <HardwareEditor name={name} /> },
};

export const TILE_KINDS = Object.keys(TILES) as TileKind[];
