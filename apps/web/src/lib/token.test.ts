import { beforeEach, describe, expect, it } from 'vitest';
import { clearToken, getToken, hasToken, setToken, TOKEN_STORAGE_KEY } from './token';

describe('token_is_held_in_the_least_durable_store_that_works', () => {
  beforeEach(() => {
    sessionStorage.clear();
    localStorage.clear();
  });

  it('keeps the token for the tab only, and never writes it to localStorage', () => {
    setToken('operator-token');

    expect(getToken()).toBe('operator-token');
    expect(hasToken()).toBe(true);
    expect(sessionStorage.getItem(TOKEN_STORAGE_KEY)).toBe('operator-token');
    // Not `localStorage`: the token should not outlive the tab.
    expect(localStorage.length).toBe(0);
    expect(JSON.stringify(localStorage)).not.toContain('operator-token');
  });

  it('forgets it on request, and reports honestly when there is none', () => {
    setToken('operator-token');
    clearToken();

    expect(getToken()).toBeNull();
    expect(hasToken()).toBe(false);
    expect(sessionStorage.getItem(TOKEN_STORAGE_KEY)).toBeNull();
  });

  it('treats whitespace as no token, so a stray paste cannot arm a write', () => {
    setToken('   ');

    expect(getToken()).toBeNull();
    expect(hasToken()).toBe(false);
  });
});
