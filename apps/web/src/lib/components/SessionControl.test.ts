import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ApiClient } from '$lib/api';
import { render } from '$lib/dom-testing';
import { session } from '$lib/session.svelte';
import SessionControl from './SessionControl.svelte';
import { TOKEN_STORAGE_KEY } from '$lib/token';

vi.mock('$app/paths', () => ({ base: '' }));

/**
 * The token is asked for once, and once exchanged it is gone from the browser's storage.
 */
function fakeApi(overrides: Partial<ApiClient> = {}): ApiClient {
  return {
    health: vi.fn(),
    devices: vi.fn(),
    entities: vi.fn(),
    uploadProfile: vi.fn(),
    setManualHost: vi.fn(),
    write: vi.fn(),
    config: vi.fn(),
    saveConfig: vi.fn(),
    session: vi.fn(() => Promise.resolve({ authenticated: false, auth_required: true })),
    signIn: vi.fn(() => Promise.resolve()),
    signOut: vi.fn(() => Promise.resolve()),
    ...overrides,
  } as ApiClient;
}

function click(view: { query: (id: string) => HTMLElement | null }, testId: string) {
  const button = view.query(testId);
  expect(button, `${testId} must be rendered`).not.toBeNull();
  button?.click();
}

beforeEach(() => {
  session.authenticated = false;
  session.authRequired = true;
  session.checked = true;
  sessionStorage.clear();
});

describe('session_control', () => {
  it('renders nothing at all when the daemon does not authenticate', () => {
    // The default deployment: the add-on is reached through Home Assistant's ingress, which has already
    // authenticated this browser. A sign-in card here would ask an operator to go and find a token file
    // for a gate that is not switched on.
    session.authRequired = false;
    const view = render(SessionControl, { api: fakeApi() });

    expect(view.query('session-sign-in')).toBeNull();
    expect(
      view.query('session-active'),
      'and not the signed-in line either — there is no session to report',
    ).toBeNull();
    view.destroy();
  });

  it('asks for the token when there is no session', () => {
    const view = render(SessionControl, { api: fakeApi() });

    expect(view.query('session-sign-in')).not.toBeNull();
    expect(view.query('session-active')).toBeNull();
    view.destroy();
  });

  it('shows a signed-in indicator instead of the token field when a session is open', () => {
    session.authenticated = true;
    const view = render(SessionControl, { api: fakeApi() });

    expect(view.query('session-active')).not.toBeNull();
    expect(
      view.query('token-input'),
      'a signed-in operator is never asked for the token',
    ).toBeNull();
    view.destroy();
  });

  it('renders nothing until the session has been checked, so the field does not flash', () => {
    session.checked = false;
    const view = render(SessionControl, { api: fakeApi() });

    expect(view.query('session-sign-in')).toBeNull();
    expect(view.query('session-active')).toBeNull();
    view.destroy();
  });
});

/** The flows, in their own `describe`: the rendering block above is at the `max-lines-per-function` ceiling. */
describe('session_control_flows', () => {
  it('exchanges the token and then deletes it from sessionStorage', async () => {
    sessionStorage.setItem(TOKEN_STORAGE_KEY, 'operator-token');
    const signIn = vi.fn(() => Promise.resolve());
    const view = render(SessionControl, { api: fakeApi({ signIn }) });

    click(view, 'session-submit');
    // Waiting on the post-condition, not on the call: `clearToken` runs after `signIn` resolves, so
    // asserting once the mock has been called races the continuation.
    await vi.waitFor(() => {
      expect(
        sessionStorage.getItem(TOKEN_STORAGE_KEY),
        'the whole point: the SPA stops holding an operator credential',
      ).toBeNull();
    });

    expect(signIn).toHaveBeenCalledWith('operator-token');
    expect(session.authenticated).toBe(true);
    view.destroy();
  });

  it('reports a refused token and stays signed out', async () => {
    sessionStorage.setItem(TOKEN_STORAGE_KEY, 'wrong');
    const signIn = vi.fn(() => Promise.reject(new Error('the token is wrong')));
    const view = render(SessionControl, { api: fakeApi({ signIn }) });

    click(view, 'session-submit');
    await vi.waitFor(() => {
      view.flush();
      expect(view.query('session-error')?.textContent).toContain('the token is wrong');
    });

    expect(session.authenticated, 'a failed sign-in must not look like a successful one').toBe(
      false,
    );
    view.destroy();
  });
});

describe('session_control_sign_out', () => {
  it('signs out', async () => {
    session.authenticated = true;
    const signOut = vi.fn(() => Promise.resolve());
    const view = render(SessionControl, { api: fakeApi({ signOut }) });

    click(view, 'session-sign-out');
    await vi.waitFor(() => {
      expect(session.authenticated).toBe(false);
    });

    expect(signOut).toHaveBeenCalled();
    view.destroy();
  });
});
