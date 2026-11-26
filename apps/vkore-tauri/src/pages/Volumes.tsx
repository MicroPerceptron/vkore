import { ArrowUp, ChevronRight, File, FileImage, FileText, Folder, HardDrive, Lock, ShieldAlert, SquareTerminal } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';
import { api, type Listing, type Preview, type VolumeEntry, type VolumeView, type Where } from '../api';
import { bytes, hexRows, plural } from '../format';
import { useStore } from '../store';
import { Empty, Pill, isActive } from '../ui';

function fileIcon(entry: VolumeEntry) {
  if (entry.directory) return <Folder size={17} />;
  if (/\.(png|jpe?g|gif|webp)$/i.test(entry.label)) return <FileImage size={17} />;
  if (/\.(txt|md|toml|json|rs|c|h|ts|py|sh|log|cfg)$/i.test(entry.label)) return <FileText size={17} />;
  return <File size={17} />;
}

/** One machine's volume: its places, folders and a preview. Used as a page and as a workspace tile. */
export function VolumeBrowser({ name }: { name: string }) {
  const { machines } = useStore();
  const selected = machines?.find((machine) => machine.name === name);

  const [volume, setVolume] = useState<VolumeView | null>(null);
  const [place, setPlace] = useState<'root' | number>(0);
  // Each step keeps the raw name (sent to the engine) beside its drawn label.
  const [path, setPath] = useState<{ name: string; label: string }[]>([]);
  const names = useMemo(() => path.map((step) => step.name), [path]);
  const [listing, setListing] = useState<Listing | null>(null);
  const [chosen, setChosen] = useState<VolumeEntry | null>(null);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  // Only the newest request may change what is shown.
  const latest = useRef(0);

  const where = useCallback(
    (path: string[]): Where => (place === 'root' ? { place: 'root', face: 0, names: path } : { place: 'home', face: place, names: path }),
    [place],
  );

  // Open the chosen machine's volume, and release it on leaving or switching.
  const active = selected ? isActive(selected.run.phase) : false;
  useEffect(() => {
    setVolume(null);
    setListing(null);
    setChosen(null);
    setPreview(null);
    setPath([]);
    setPlace(0);
    setError('');
    if (!name || active) return;
    const ticket = ++latest.current;
    setBusy(true);
    void api()
      .then((engine) => engine.volumeOpen(name))
      .then((view) => ticket === latest.current && setVolume(view))
      .catch((failure) => ticket === latest.current && setError(String(failure)))
      .finally(() => ticket === latest.current && setBusy(false));
    return () => {
      latest.current += 1;
      void api().then((engine) => engine.volumeClose()).catch(() => undefined);
    };
  }, [name, active]);

  // List the current directory.
  useEffect(() => {
    if (!name || !volume) return;
    const ticket = ++latest.current;
    setBusy(true);
    setChosen(null);
    setPreview(null);
    void api()
      .then((engine) => engine.volumeList(name, where(names)))
      .then((result) => {
        if (ticket !== latest.current) return;
        setListing(result);
        setError('');
      })
      .catch((failure) => ticket === latest.current && (setListing(null), setError(String(failure))))
      .finally(() => ticket === latest.current && setBusy(false));
  }, [name, volume, names, where]);

  const choose = (entry: VolumeEntry) => {
    if (entry.directory) {
      setPath([...path, { name: entry.name, label: entry.label }]);
      return;
    }
    if (!name) return;
    const ticket = ++latest.current;
    setChosen(entry);
    setPreview(null);
    void api()
      .then((engine) => engine.volumePreview(name, where([...names, entry.name])))
      .then((result) => ticket === latest.current && (setPreview(result), setError('')))
      .catch((failure) => ticket === latest.current && setError(String(failure)));
  };

  const entries = listing?.kind === 'directory' ? listing.entries : [];
  const trail = [place === 'root' ? 'volume' : `terminal ${place}`, ...path.map((step) => step.label)];

  return (
    <>
      {name && active && (
        <div className="card">
          <Empty icon={Lock} title={`${name} is running`}>
            A running machine owns its disk. Stop it to look inside.{' '}
            <Link to={`/machines/${encodeURIComponent(name)}`} className="link">Go to the machine</Link>
          </Empty>
        </div>
      )}

      {name && !active && error && !volume && (
        <p className="notice bad" role="alert">{error}</p>
      )}

      {name && !active && volume && (
        <section className="card">
          <header className="card-head">
            <h2>{volume.machine}</h2>
            <span className="chips-inline">
              <span className="pill quiet">generation {volume.generation}</span>
              <span className="pill quiet">format {volume.formatVersion}</span>
              <span className="pill quiet"><Lock size={13} /> read-only</span>
            </span>
          </header>
          {volume.usedFallbackRoot && (
            <p className="notice bad"><ShieldAlert size={14} /> Recovery used the fallback root. Kore keeps this volume read-only until a boot repairs it.</p>
          )}

          <div className="places" role="tablist" aria-label="Where on the volume">
            {volume.faces.slice(0, 4).map((face) => (
              <button key={face} role="tab" aria-selected={place === face} className={`place ${place === face ? 'on' : ''}`} onClick={() => { setPlace(face); setPath([]); }}>
                <SquareTerminal size={15} /> Terminal {face}
              </button>
            ))}
            <button role="tab" aria-selected={place === 'root'} className={`place ${place === 'root' ? 'on' : ''}`} onClick={() => { setPlace('root'); setPath([]); }}>
              <HardDrive size={15} /> Volume root
            </button>
          </div>

          <nav className="crumbs" aria-label="Path">
            <button className="icon" disabled={names.length === 0} onClick={() => setPath(path.slice(0, -1))} aria-label="Up one folder" title="Up one folder">
              <ArrowUp size={15} />
            </button>
            {trail.map((part, index) => (
              <span key={index} className="crumb">
                {index > 0 && <ChevronRight size={14} className="faint" />}
                <button className="link-button" disabled={index === trail.length - 1} onClick={() => setPath(path.slice(0, index))}>
                  {part}
                </button>
              </span>
            ))}
          </nav>

          <div className="browser">
            <div className="listing" aria-busy={busy}>
              {listing?.kind === 'emptyHome' && (
                <Empty icon={SquareTerminal} title="Nothing here yet">The machine creates this terminal&apos;s home on the boot that first uses it.</Empty>
              )}
              {listing?.kind === 'directory' && entries.length === 0 && <Empty icon={Folder} title="Empty folder" />}
              {entries.length > 0 && (
                <ul className="files">
                  {entries.map((entry) => (
                    <li key={entry.name}>
                      <button className={`file ${chosen?.name === entry.name ? 'on' : ''}`} onClick={() => choose(entry)}>
                        <span className={`file-icon ${entry.directory ? 'dir' : ''}`}>{fileIcon(entry)}</span>
                        <span className="file-name">{entry.label}</span>
                        <span className="faint small">{entry.directory ? '' : bytes(entry.len)}</span>
                      </button>
                    </li>
                  ))}
                </ul>
              )}
              {listing?.kind === 'directory' && listing.truncated && <p className="faint small">Showing the first entries only.</p>}
              {listing?.kind === 'directory' && entries.length > 0 && (
                <p className="faint small footnote">{plural(entries.length, 'item')}</p>
              )}
              {error && <p className="notice bad" role="alert">{error}</p>}
            </div>

            <aside className="preview" aria-live="polite">
              {!chosen && <Empty icon={FileText} title="Pick a file">Its first 32 KiB is read to show it.</Empty>}
              {chosen && !preview && !error && <p className="muted">Reading {chosen.label}…</p>}
              {chosen && preview && <PreviewBody label={chosen.label} preview={preview} />}
            </aside>
          </div>
        </section>
      )}

      {name && !active && busy && !volume && !error && <p className="muted">Opening the volume…</p>}
    </>
  );
}

/** A picture, and a plain statement if the window could not decode it. */
function Picture({ url, label, width, height }: { url: string; label: string; width: number; height: number }) {
  const [failed, setFailed] = useState(false);
  if (failed) {
    return <p className="notice bad" role="alert">The window could not draw this picture ({width} × {height}, {Math.round(url.length / 1024)} KiB sent).</p>;
  }
  return <img className="shot" src={url} alt={`Preview of ${label}`} width={width} height={height} onError={() => setFailed(true)} />;
}

function PreviewBody({ label, preview }: { label: string; preview: Preview }) {
  return (
    <>
      <header className="card-head">
        <h3>{label}</h3>
        <span className="faint small">{bytes(preview.len)}</span>
      </header>
      {preview.kind === 'image' && (
        <>
          {preview.dataUrl ? (
            <Picture key={preview.dataUrl.length + label} url={preview.dataUrl} label={label} width={preview.width} height={preview.height} />
          ) : (
            <p className="notice">
              Not shown: {preview.len > 2 << 20 ? `the file is over 2 MiB (${bytes(preview.len)})` : `the picture is too large (${preview.width} × ${preview.height})`}.
            </p>
          )}
          <p className="faint small">{preview.width} × {preview.height} PNG</p>
          {preview.capture && (
            <dl className="facts compact">
              {([
                ['Taken by', preview.capture.software],
                ['Created', preview.capture.created],
                ['Kernel build', preview.capture.kernelBuild],
                ['Uptime', preview.capture.uptimeMs ? `${preview.capture.uptimeMs} ms` : null],
                ['Mode', preview.capture.mode],
                ['App', preview.capture.app],
              ] as const).filter(([, value]) => value).map(([key, value]) => (
                <div key={key}><dt>{key}</dt><dd>{value}</dd></div>
              ))}
            </dl>
          )}
        </>
      )}
      {preview.kind === 'text' && (
        <>
          <pre className="text-preview">{preview.text}</pre>
          <p className="faint small">{plural(preview.lines, 'line')}{preview.truncated ? ' shown; the file goes on' : ''}</p>
        </>
      )}
      {preview.kind === 'binary' && (
        <>
          <p className="faint small">Binary file. Its first bytes:</p>
          <pre className="text-preview hex">
            {hexRows(preview.headHex).map((row) => `${row.offset}  ${row.bytes.padEnd(47)}  ${row.ascii}`).join('\n')}
          </pre>
        </>
      )}
    </>
  );
}

export function Volumes() {
  const { name } = useParams();
  const navigate = useNavigate();
  const { machines } = useStore();
  return (
    <>
      <header className="page-head">
        <div className="machine-title-row">
          <span className="plate big"><HardDrive size={24} /></span>
          <div>
            <p className="eyebrow">Volumes</p>
            <h1>Look inside a disk</h1>
          </div>
        </div>
      </header>

      <div className="pickers" role="tablist" aria-label="Machines">
        {(machines ?? []).map((machine) => {
          const locked = isActive(machine.run.phase);
          return (
            <button
              key={machine.name}
              role="tab"
              aria-selected={machine.name === name}
              className={`pick ${machine.name === name ? 'on' : ''}`}
              onClick={() => navigate(`/volumes/${encodeURIComponent(machine.name)}`)}
            >
              {locked ? <Lock size={15} /> : <HardDrive size={15} />}
              {machine.name}
              {locked && <Pill phase={machine.run.phase} />}
            </button>
          );
        })}
      </div>

      {name ? (
        <VolumeBrowser key={name} name={name} />
      ) : (
        <div className="card">
          <Empty icon={HardDrive} title="Choose a machine">
            Its disk opens read-only, through Kore&apos;s own filesystem engine, so you see what the machine sees.
          </Empty>
        </div>
      )}
    </>
  );
}
