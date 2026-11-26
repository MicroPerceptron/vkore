import { describe, expect, it } from 'vitest';
import type { HardwareDraft } from './api';
import { preferredCpus, sameDraft, snapMemory, toggleCpu, withCpuCount } from './hardware';

// Four physical cores with two threads each, as Linux numbers them.
const cores = [0, 1, 2, 3].map((id) => ({ id, kind: 'Core', threads: [id, id + 4] }));

const draft: HardwareDraft = {
  cpus: 4, memoryMib: 4096, pinCpus: true, hostCpus: [0, 1, 2, 3], cpuPowerManagement: false,
  pinMemory: false, pages: 'system', network: 'isolated', guestNuma: 'split',
};

describe('hardware helpers', () => {
  it('spreads over physical cores before SMT siblings', () => {
    expect(preferredCpus(cores, 4)).toEqual([0, 1, 2, 3]);
    expect(preferredCpus(cores, 6)).toEqual([0, 1, 2, 3, 4, 5]);
    expect(preferredCpus(cores, 99)).toHaveLength(8);
    expect(preferredCpus([], 4)).toEqual([]);
  });
  it('toggles a host CPU, keeping the order chosen', () => {
    expect(toggleCpu([3, 1], 2)).toEqual([3, 1, 2]);
    expect(toggleCpu([3, 1, 2], 1)).toEqual([3, 2]);
  });
  it('snaps memory to its step and bounds', () => {
    expect(snapMemory(4100, 2048, 16384)).toBe(4096);
    expect(snapMemory(100, 2048, 16384)).toBe(2048);
    expect(snapMemory(99999, 2048, 16384)).toBe(16384);
  });
  it('compares drafts, ignoring chosen CPUs while pinning is off', () => {
    expect(sameDraft(draft, { ...draft })).toBe(true);
    expect(sameDraft(draft, { ...draft, memoryMib: 8192 })).toBe(false);
    expect(sameDraft(draft, { ...draft, hostCpus: [3, 2, 1, 0] })).toBe(false);
    const free = { ...draft, pinCpus: false };
    expect(sameDraft(free, { ...free, hostCpus: [7] })).toBe(true);
  });
  it('keeps pinned CPUs matching the guest CPU count', () => {
    expect(withCpuCount(draft, cores, 2).hostCpus).toEqual([0, 1]);
    expect(withCpuCount(draft, cores, 6).hostCpus).toEqual([0, 1, 2, 3, 4, 5]);
    expect(withCpuCount({ ...draft, pinCpus: false }, cores, 6).hostCpus).toEqual([0, 1, 2, 3]);
  });
});
