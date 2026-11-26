import { describe, expect, it } from 'vitest';
import { bytes, hexRows, mib, plural, seconds, share } from './format';

describe('format', () => {
  it('writes sizes in the unit a person would', () => {
    expect(mib(512)).toBe('512 MiB');
    expect(mib(4096)).toBe('4 GiB');
    expect(mib(1536)).toBe('1.5 GiB');
  });
  it('pluralises', () => {
    expect(plural(1, 'machine')).toBe('1 machine');
    expect(plural(3, 'machine')).toBe('3 machines');
  });
  it('shows an unknown time as a dash', () => {
    expect(seconds(null)).toBe('—');
    expect(seconds(1250)).toBe('1.3 s');
  });
  it('clamps a share to the unit interval', () => {
    expect(share(5, 0)).toBe(0);
    expect(share(20, 10)).toBe(1);
    expect(share(-1, 10)).toBe(0);
    expect(share(5, 10)).toBe(0.5);
  });
  it('writes byte counts', () => {
    expect(bytes(512)).toBe('512 B');
    expect(bytes(1536)).toBe('1.5 KiB');
    expect(bytes(5 << 20)).toBe('5 MiB');
    expect(bytes(123 << 20)).toBe('123 MiB');
  });
  it('lays hex out as a dump with printable characters', () => {
    expect(hexRows('48690aff')).toEqual([{ offset: '0000', bytes: '48 69 0a ff', ascii: 'Hi··' }]);
    expect(hexRows('')).toEqual([]);
  });
});
