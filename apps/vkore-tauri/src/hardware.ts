// Pure helpers for the hardware editor. The engine is the judge of what is valid;
// these only choose sensible values to offer and describe what is chosen.
import type { HardwareDraft, HostHardware } from './api';

/** Host CPUs for `count` guest CPUs: one per physical core first, then SMT siblings (as the engine's own default does). */
export function preferredCpus(cores: HostHardware['cores'], count: number): number[] {
  const out: number[] = [];
  const widest = Math.max(0, ...cores.map((core) => core.threads.length));
  for (let sibling = 0; sibling < widest; sibling += 1) {
    for (const core of cores) {
      const id = core.threads[sibling];
      if (id !== undefined) out.push(id);
    }
  }
  return out.slice(0, count);
}

/** Toggle one host CPU in a selection, keeping the order it was chosen in. */
export function toggleCpu(selected: number[], id: number): number[] {
  return selected.includes(id) ? selected.filter((cpu) => cpu !== id) : [...selected, id];
}

/** The step a memory size moves by: a whole number of 2 MiB pages and of two guest nodes. */
export const MEMORY_STEP_MIB = 512;

export function snapMemory(mib: number, min: number, max: number): number {
  const stepped = Math.round(mib / MEMORY_STEP_MIB) * MEMORY_STEP_MIB;
  return Math.min(max, Math.max(min, stepped));
}

export function sameDraft(a: HardwareDraft, b: HardwareDraft): boolean {
  return (
    a.cpus === b.cpus &&
    a.memoryMib === b.memoryMib &&
    a.pinCpus === b.pinCpus &&
    a.cpuPowerManagement === b.cpuPowerManagement &&
    a.pinMemory === b.pinMemory &&
    a.pages === b.pages &&
    a.network === b.network &&
    a.guestNuma === b.guestNuma &&
    // The chosen CPUs only matter while pinning is on.
    (!a.pinCpus || (a.hostCpus.length === b.hostCpus.length && a.hostCpus.every((cpu, at) => cpu === b.hostCpus[at])))
  );
}

/** A draft after the guest CPU count changed: pinned CPUs follow it, trimmed or topped up. */
export function withCpuCount(draft: HardwareDraft, cores: HostHardware['cores'], count: number): HardwareDraft {
  if (!draft.pinCpus) return { ...draft, cpus: count };
  const kept = draft.hostCpus.slice(0, count);
  const extra = preferredCpus(cores, cores.length * 8).filter((id) => !kept.includes(id));
  return { ...draft, cpus: count, hostCpus: [...kept, ...extra].slice(0, count) };
}
