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
