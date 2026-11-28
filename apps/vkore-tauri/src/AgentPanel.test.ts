import { describe, expect, it } from 'vitest';
import { rememberLabel } from './AgentPanel';

const ask = { id: 'a', title: 'Type into the console', detail: 'ls⏎' };

describe('what a remembered approval says it covers', () => {
  it('names the scope, and offers nothing when none may be remembered', () => {
    expect(rememberLabel({ ...ask, remember: null })).toBeNull();
    expect(rememberLabel({ ...ask, remember: 'exact' })).toContain('this exact action');
    expect(rememberLabel({ ...ask, remember: 'kind' })).toContain('every “Type into the console”');
  });
});
