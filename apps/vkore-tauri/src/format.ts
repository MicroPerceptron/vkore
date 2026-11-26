export function mib(value: number): string {
  if (value >= 1024) {
    const gib = value / 1024;
    return `${Number.isInteger(gib) ? gib : gib.toFixed(1)} GiB`;
  }
  return `${value} MiB`;
}

export function seconds(ms: number | null): string {
  return ms === null ? '—' : `${(ms / 1000).toFixed(1)} s`;
}

export function plural(count: number, one: string, many = `${one}s`): string {
  return `${count} ${count === 1 ? one : many}`;
}

export function share(part: number, whole: number): number {
  return whole <= 0 ? 0 : Math.min(1, Math.max(0, part / whole));
}

/** A byte count in the unit a person would use. */
export function bytes(value: number): string {
  if (value < 1024) return `${value} B`;
  const units = ['KiB', 'MiB', 'GiB', 'TiB'];
  let size = value / 1024;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${size >= 10 || Number.isInteger(size) ? Math.round(size) : size.toFixed(1)} ${units[unit]}`;
}

/** A hex string as dump rows: offset, 16 bytes, and the printable characters. */
export function hexRows(hex: string): { offset: string; bytes: string; ascii: string }[] {
  const rows = [];
  for (let at = 0; at < hex.length; at += 32) {
    const chunk = hex.slice(at, at + 32).match(/../g) ?? [];
    rows.push({
      offset: (at / 2).toString(16).padStart(4, '0'),
      bytes: chunk.join(' '),
      ascii: chunk.map((byte) => {
        const code = parseInt(byte, 16);
        return code >= 0x20 && code < 0x7f ? String.fromCharCode(code) : '·';
      }).join(''),
    });
  }
  return rows;
}
