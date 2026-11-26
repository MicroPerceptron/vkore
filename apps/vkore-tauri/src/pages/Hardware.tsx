import { ArrowLeft, CircleAlert, CircleCheck, Cpu, HardDrive, KeyRound, Loader, MemoryStick, Network, Save, Undo2, Wand2 } from 'lucide-react';
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Link, useParams } from 'react-router-dom';
import { api, type HardwareCheck, type HardwareDraft, type HardwareView } from '../api';
import { mib, share } from '../format';
import { MEMORY_STEP_MIB, preferredCpus, sameDraft, snapMemory, toggleCpu, withCpuCount } from '../hardware';
import { useStore } from '../store';
import { Empty, Meter, Pill, isActive } from '../ui';

function Switch({ checked, onChange, disabled, label, hint }: { checked: boolean; onChange: (next: boolean) => void; disabled?: boolean; label: string; hint?: ReactNode }) {
  return (
    <label className={`switch-row ${disabled ? 'off' : ''}`}>
      <span className="switch-text">
        <strong>{label}</strong>
        {hint && <span className="faint small">{hint}</span>}
      </span>
      <input type="checkbox" role="switch" className="switch" checked={checked} disabled={disabled} onChange={(event) => onChange(event.target.checked)} />
    </label>
  );
}

function Choice<T extends string>({ value, options, onChange, disabled, label }: {
  value: T;
  options: { value: T; label: string; hint?: string; disabled?: boolean }[];
  onChange: (next: T) => void;
  disabled?: boolean;
  label: string;
}) {
  return (
    <div className="choices" role="radiogroup" aria-label={label}>
      {options.map((option) => (
        <button
          key={option.value}
          role="radio"
          aria-checked={value === option.value}
          className={`choice ${value === option.value ? 'on' : ''}`}
          disabled={disabled || option.disabled}
          onClick={() => onChange(option.value)}
        >
          <strong>{option.label}</strong>
          {option.hint && <span className="faint small">{option.hint}</span>}
        </button>
      ))}
    </div>
  );
}

/** A machine's hardware form. Used as a page and as a workspace tile. */
export function HardwareEditor({ name }: { name: string }) {
  const { machines, refresh } = useStore();
  const machine = machines?.find((entry) => entry.name === name);
  const locked = machine ? isActive(machine.run.phase) : false;

  const [view, setView] = useState<HardwareView | null>(null);
  const [draft, setDraft] = useState<HardwareDraft | null>(null);
  const [check, setCheck] = useState<HardwareCheck | null>(null);
  const [loadError, setLoadError] = useState('');
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState('');
  const [saved, setSaved] = useState(false);
  const ticket = useRef(0);

  const load = () => {
    setLoadError('');
    void api()
      .then((engine) => engine.hardwareGet(name))
      .then((loaded) => {
        setView(loaded);
        setDraft(loaded.draft);
      })
      .catch((failure) => setLoadError(String(failure)));
  };
  useEffect(load, [name]); // eslint-disable-line react-hooks/exhaustive-deps

  // Ask the engine whether the draft would be accepted, once typing settles.
  useEffect(() => {
    if (!draft || locked) return;
    const mine = ++ticket.current;
    const timer = setTimeout(() => {
      void api()
        .then((engine) => engine.hardwareCheck(name, draft))
        .then((result) => mine === ticket.current && setCheck(result))
        .catch((failure) => mine === ticket.current && setCheck({ ok: false, message: String(failure), needsAuthorization: false }));
    }, 250);
    return () => clearTimeout(timer);
  }, [draft, name, locked]);

  const changed = useMemo(() => !!(view && draft && !sameDraft(view.draft, draft)), [view, draft]);

  if (loadError) return <p className="notice bad" role="alert">{loadError}</p>;
  if (!machine && machines) {
    return <div className="card"><Empty icon={HardDrive} title="No such machine"><Link to="/machines" className="link">Back to machines</Link></Empty></div>;
  }
  if (!view || !draft) return <p className="muted">Reading hardware…</p>;

  const { host } = view;
  const set = (patch: Partial<HardwareDraft>) => { setSaved(false); setDraft({ ...draft, ...patch }); };
  const pool = (page: string) => host.pools.find((entry) => entry.page === page);
  const memoryMax = Math.min(host.memoryMib, 1 << 20);
  const busy = saving || locked;

  const save = async () => {
    setSaving(true);
    setSaveError('');
    try {
      await (await api()).hardwareSave(name, draft);
      const fresh = await (await api()).hardwareGet(name);
      setView(fresh);
      setDraft(fresh.draft);
      setSaved(true);
      void refresh();
    } catch (failure) {
      setSaveError(String(failure));
    } finally {
      setSaving(false);
    }
  };

  return (
    <>
      {locked && <p className="notice">A running machine owns its hardware. Stop it to change anything here.</p>}

      <section className="card">
        <header className="card-head"><h2><Cpu size={18} /> Compute</h2><span className="faint small">against this host</span></header>
        <div className="field">
          <div className="field-head"><strong>CPUs</strong><span className="value">{draft.cpus}</span></div>
          <input type="range" min={view.minCpus} max={view.maxCpus} step={1} value={draft.cpus} disabled={busy} aria-label="Guest CPUs"
            onChange={(event) => { setSaved(false); setDraft(withCpuCount(draft, host.cores, Number(event.target.value))); }} />
          <Meter used={draft.cpus} of={host.logicalCpus} label="Host threads" detail={`${draft.cpus} of ${host.logicalCpus} (${host.physicalCores} cores)`} />
        </div>
        <div className="field">
          <div className="field-head"><strong>Memory</strong><span className="value">{mib(draft.memoryMib)}</span></div>
          <input type="range" min={view.minMemoryMib} max={memoryMax} step={MEMORY_STEP_MIB} value={draft.memoryMib} disabled={busy} aria-label="Guest memory"
            onChange={(event) => set({ memoryMib: snapMemory(Number(event.target.value), view.minMemoryMib, memoryMax) })} />
          <Meter used={draft.memoryMib} of={host.availableMemoryMib} label="Host memory available" detail={`${mib(draft.memoryMib)} of ${mib(host.availableMemoryMib)}`} />
        </div>
      </section>

      <section className="card">
        <header className="card-head"><h2><Cpu size={18} /> CPU placement</h2></header>
        <Switch label="Pin guest CPUs to host CPUs" checked={draft.pinCpus} disabled={busy || !host.cpuPinning}
          hint={host.cpuPinning ? 'Each guest CPU runs on one host CPU you choose.' : 'This host cannot pin CPUs exactly.'}
          onChange={(next) => set({ pinCpus: next, hostCpus: next && draft.hostCpus.length !== draft.cpus ? preferredCpus(host.cores, draft.cpus) : draft.hostCpus, cpuPowerManagement: next ? draft.cpuPowerManagement : false })} />
        {draft.pinCpus && (
          <div className="field">
            <div className="field-head">
              <strong>Host CPUs</strong>
              <span className={draft.hostCpus.length === draft.cpus ? 'faint small' : 'small bad-text'}>{draft.hostCpus.length} of {draft.cpus} chosen</span>
            </div>
            <div className="cores" role="group" aria-label="Host CPUs">
              {host.cores.map((core) => (
                <div key={core.id} className="core" title={core.kind}>
                  {core.threads.map((id) => (
                    <button key={id} className={`cpu ${draft.hostCpus.includes(id) ? 'on' : ''}`} disabled={busy}
                      aria-pressed={draft.hostCpus.includes(id)} onClick={() => set({ hostCpus: toggleCpu(draft.hostCpus, id) })}>
                      {id}
                    </button>
                  ))}
                </div>
              ))}
            </div>
            <button disabled={busy} onClick={() => set({ hostCpus: preferredCpus(host.cores, draft.cpus) })}><Wand2 size={15} /> Choose automatically</button>
          </div>
        )}
        <Switch label="Native CPU waits" checked={draft.cpuPowerManagement} disabled={busy || !draft.pinCpus || !!host.nativeWaitsError}
          hint={host.nativeWaitsError ?? 'The guest idles with the host CPU instead of spinning. Needs pinned CPUs and KVM.'}
          onChange={(next) => set({ cpuPowerManagement: next })} />
      </section>

      <section className="card">
        <header className="card-head"><h2><MemoryStick size={18} /> Memory</h2></header>
        <Switch label="Lock guest memory" checked={draft.pinMemory} disabled={busy || !host.memoryPinning}
          hint={host.memoryPinning ? 'Keeps the guest in RAM. May ask your desktop to authenticate.' : 'This host cannot lock memory.'}
          onChange={(next) => set({ pinMemory: next })} />
        <div className="field">
          <div className="field-head"><strong>Page size</strong></div>
          <Choice label="Page size" value={draft.pages} disabled={busy} onChange={(pages) => set({ pages })}
            options={[
              { value: 'system', label: 'System pages', hint: 'The default; needs no setup' },
              { value: '2m', label: '2 MiB hugepages', hint: pool('2m') ? `${mib(pool('2m')!.freeMib)} free` : 'Not available here', disabled: !pool('2m') || !host.hugetlb },
              { value: '1g', label: '1 GiB hugepages', hint: pool('1g') ? `${mib(pool('1g')!.freeMib)} free` : 'Not available here', disabled: !pool('1g') || !host.hugetlb },
            ]} />
        </div>
        <div className="field">
          <div className="field-head"><strong>Guest memory nodes</strong></div>
          <Choice label="Guest memory nodes" value={draft.guestNuma} disabled={busy} onChange={(guestNuma) => set({ guestNuma })}
            options={[{ value: 'single', label: 'One node' }, { value: 'split', label: 'Two nodes', hint: 'Memory in multiples of 2' }]} />
        </div>
      </section>

      <section className="card">
        <header className="card-head"><h2><Network size={18} /> Network</h2></header>
        <Choice label="Network" value={draft.network} disabled={busy} onChange={(network) => set({ network })}
          options={[{ value: 'isolated', label: 'Isolated', hint: 'No connection to the host or outside' }, { value: 'e1000e', label: 'Shared (e1000e)', hint: 'Reaches the network through the host' }]} />
      </section>

      <section className="card">
        <header className="card-head"><h2><HardDrive size={18} /> Fixed after creation</h2></header>
        <dl className="facts">
          <div><dt>Disk</dt><dd>{mib(view.diskMib)}{view.reserveStorage ? ' · reserved up front' : ''}</dd></div>
          <div><dt>Assigned devices</dt><dd>{view.devices === 0 ? 'None' : `${view.devices} (edit in the iced desktop for now)`}</dd></div>
        </dl>
        <p className="faint small">The engine does not resize a disk after it is made, and devices are kept exactly as they are when you save.</p>
      </section>

      <footer className="savebar" aria-live="polite">
        <div className="savebar-text">
          {saving ? <span className="muted"><Loader size={16} className="spin" /> {check?.needsAuthorization ? 'Waiting for your desktop to authenticate…' : 'Saving…'}</span>
            : saveError ? <span className="bad-text" role="alert"><CircleAlert size={16} /> {saveError}</span>
            : saved && !changed ? <span className="good-text"><CircleCheck size={16} /> Saved. The next start uses this hardware.</span>
            : check && !check.ok && changed ? <span className="bad-text"><CircleAlert size={16} /> {check.message}</span>
            : check?.ok && changed && check.needsAuthorization ? <span className="muted"><KeyRound size={16} /> Saving will ask your desktop to authenticate.</span>
            : check?.ok && changed ? <span className="good-text"><CircleCheck size={16} /> This hardware is accepted.</span>
            : <span className="faint">{share(draft.cpus, host.logicalCpus) > 0.75 ? 'Most of the host is reserved by this machine.' : 'No changes.'}</span>}
        </div>
        <div className="savebar-actions">
          <button disabled={!changed || saving} onClick={() => { setDraft(view.draft); setSaveError(''); }}><Undo2 size={15} /> Revert</button>
          <button className="primary" disabled={busy || !changed || !check?.ok} onClick={save}><Save size={15} /> Save hardware</button>
        </div>
      </footer>
    </>
  );
}

export function Hardware() {
  const { name = '' } = useParams();
  const { machines } = useStore();
  const machine = machines?.find((entry) => entry.name === name);
  return (
    <>
      <Link to={`/machines/${encodeURIComponent(name)}`} className="back"><ArrowLeft size={15} /> {name}</Link>
      <header className="page-head">
        <div className="machine-title-row">
          <span className="plate big"><Cpu size={24} /></span>
          <div>
            <p className="eyebrow">Hardware</p>
            <h1>{name}</h1>
          </div>
          {machine && <Pill phase={machine.run.phase} />}
        </div>
      </header>

      <HardwareEditor key={name} name={name} />
    </>
  );
}
