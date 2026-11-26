import { describe, expect, it } from 'vitest';
import { keyToInput, paintCss } from './keys';

const press = (key: string, mods: Partial<{ ctrlKey: boolean; altKey: boolean; metaKey: boolean; shiftKey: boolean }> = {}) => ({
  key,
  ctrlKey: false,
  altKey: false,
  metaKey: false,
  ...mods,
});

describe('keyToInput', () => {
  it('sends printable characters as they are', () => {
    expect(keyToInput(press('a'), false)).toBe('a');
    expect(keyToInput(press('é'), false)).toBe('é');
    expect(keyToInput(press(' '), false)).toBe(' ');
  });
  it('sends the bytes a terminal would for editing keys', () => {
    expect(keyToInput(press('Enter'), false)).toBe('\r');
    expect(keyToInput(press('Backspace'), false)).toBe('\x7f');
    expect(keyToInput(press('Tab'), false)).toBe('\t');
    expect(keyToInput(press('Delete'), false)).toBe('\x1b[3~');
  });
  it('follows the guest when it asks for application cursor keys', () => {
    expect(keyToInput(press('ArrowUp'), false)).toBe('\x1b[A');
    expect(keyToInput(press('ArrowUp'), true)).toBe('\x1bOA');
  });
  it('turns control letters into control codes', () => {
    expect(keyToInput(press('c', { ctrlKey: true }), false)).toBe('\x03');
    expect(keyToInput(press('D', { ctrlKey: true }), false)).toBe('\x04');
    expect(keyToInput(press('1', { ctrlKey: true }), false)).toBeNull();
  });
  it('prefixes alt with escape', () => {
    expect(keyToInput(press('b', { altKey: true }), false)).toBe('\x1bb');
  });
  it('lets Shift+Tab leave the console', () => {
    expect(keyToInput(press('Tab', { shiftKey: true }), false)).toBeNull();
  });
  it('leaves window shortcuts and non-text keys alone', () => {
    expect(keyToInput(press('v', { metaKey: true }), false)).toBeNull();
    expect(keyToInput(press('Shift'), false)).toBeNull();
    expect(keyToInput(press('F5'), false)).toBeNull();
  });
});

describe('paintCss', () => {
  it('maps the three kinds of terminal colour', () => {
    expect(paintCss(null)).toBeNull();
    expect(paintCss(1)).toBe('#d95f4f');
    expect(paintCss(196)).toBe('rgb(255, 0, 0)');
    expect(paintCss(232)).toBe('rgb(8, 8, 8)');
    expect(paintCss('#ff0010')).toBe('#ff0010');
  });
  it('refuses a colour that is not one, so nothing else reaches a style', () => {
    expect(paintCss('red; background: url(x)')).toBeNull();
    expect(paintCss(999)).toBeNull();
  });
});
