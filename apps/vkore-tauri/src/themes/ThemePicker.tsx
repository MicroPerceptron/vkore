import { Check, Palette } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { PRESETS } from './presets';

/** A button that opens the list of colour presets. It owns no theme state: the caller applies the choice. */
export function ThemePicker({ value, onChange }: { value: string; onChange: (id: string) => void }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const close = (event: MouseEvent | KeyboardEvent) => {
      if (event instanceof KeyboardEvent) {
        if (event.key === 'Escape') setOpen(false);
      } else if (!root.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener('mousedown', close);
    document.addEventListener('keydown', close);
    return () => {
      document.removeEventListener('mousedown', close);
      document.removeEventListener('keydown', close);
    };
  }, [open]);

  return (
    <div className="picker" ref={root}>
      <button className="icon" onClick={() => setOpen(!open)} aria-haspopup="listbox" aria-expanded={open} aria-label="Colour theme" title="Colour theme">
        <Palette size={16} />
      </button>
      {open && (
        <ul className="picker-list card" role="listbox" aria-label="Colour themes">
          {PRESETS.map((preset) => (
            <li key={preset.id} role="option" aria-selected={preset.id === value}>
              <button
                className={`picker-item ${preset.id === value ? 'on' : ''}`}
                onClick={() => {
                  onChange(preset.id);
                  setOpen(false);
                }}
              >
                <span className="swatches" aria-hidden>
                  <i style={{ background: preset.body }} />
                  <i style={{ background: preset.panel }} />
                  <i style={{ background: preset.accent }} />
                </span>
                <span className="picker-text">
                  <strong>{preset.name}</strong>
                  <span className="faint small">{preset.desc}</span>
                </span>
                {preset.id === value && <Check size={16} />}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
