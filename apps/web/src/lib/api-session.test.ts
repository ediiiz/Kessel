import { describe, expect, it, vi } from 'vitest';

/**
 * The client half of the session exchange:
 *
 *   1. a mutating call with **no** token but an open session is sent, with no auth header on it;
 *   2. a mutating call with neither still refuses locally;
 *   3. every request carries `credentials: 'same-origin'`, without which the browser would not attach the
 *      cookie and every call would fail as an opaque 401.
 *
 * `base` is mocked to an ingress prefix for the same reason `api.test.ts` mocks it: at the loopback base of
 * `''` a root-absolute URL and a correctly composed one are the same string.
 */
const { INGRESS_BASE } = vi.hoisted(() => ({ INGRESS_BASE: '/api/hassio_ingress/tokenvalue' }));

vi.mock('$app/paths', () => ({ base: INGRESS_BASE }));

/**
 * A `fetch` stand-in that records calls and answers with `body`.
 *
 * A write is answered with a `WriteOutcome` whatever else is being returned, as in `api.test.ts`:
 * `readWriteOutcome` rejects any other shape.
 */
function recordingFetch(body: unknown, status = 200) {
  const calls: { url: string; init: RequestInit | undefined }[] = [];
  const fetchImpl = (url: string, init?: RequestInit): Promise<Response> => {
    calls.push({ url, init });
    const answer = url.endsWith('/write') ? { outcome: 'accepted' } : body;
    return Promise.resolve(
      new Response(status === 204 ? null : JSON.stringify(answer), {
        status,
        headers: { 'content-type': 'application/json' },
      }),
    );
  };
  return { calls, fetchImpl };
}

describe('session_exchange', () => {
  it('posts the token to /api/session in the header, not the body or the URL', async () => {
    const { calls, fetchImpl } = recordingFetch(null, 204);
    const { createApiClient, TOKEN_HEADER } = await import('$lib/api');

    await createApiClient({ fetchImpl }).signIn('operator-token');

    expect(calls).toHaveLength(1);
    const call = calls[0];
    expect(call).toBeDefined();
    if (call === undefined) return;
    expect(call.url).toBe(`${INGRESS_BASE}/api/session`);
    expect(call.init?.method).toBe('POST');
    const headers = call.init?.headers as Record<string, string> | undefined;
    expect(headers?.[TOKEN_HEADER]).toBe('operator-token');
    // The token must not travel anywhere a log or a Referer could pick it up.
    expect(call.url).not.toContain('operator-token');
    expect(call.init?.body ?? null).toBeNull();
  });

  it('reports a refused exchange rather than pretending it worked', async () => {
    const { fetchImpl } = recordingFetch({ message: 'the token is wrong' }, 401);
    const { createApiClient } = await import('$lib/api');

    await expect(createApiClient({ fetchImpl }).signIn('wrong')).rejects.toThrow(
      'the token is wrong',
    );
  });

  it('treats an already-gone session as a successful sign-out', async () => {
    const { fetchImpl } = recordingFetch({ message: 'unauthorized' }, 401);
    const { createApiClient } = await import('$lib/api');

    await expect(createApiClient({ fetchImpl }).signOut()).resolves.toBeUndefined();
  });
});

describe('session_replaces_the_token_on_mutating_calls', () => {
  it('sends a mutating call with no auth header when a session is open', async () => {
    const { calls, fetchImpl } = recordingFetch({ outcome: 'accepted' });
    const { createApiClient, TOKEN_HEADER } = await import('$lib/api');
    const api = createApiClient({ fetchImpl, token: () => null, hasSession: () => true });

    await api.write('dishwasher', { feature_key: 'Some.Feature', value: '1' });

    expect(calls).toHaveLength(1);
    const headers = calls[0]?.init?.headers as Record<string, string>;
    expect(headers[TOKEN_HEADER]).toBeUndefined();
    expect(headers['content-type']).toBe('application/json');
  });

  it('still refuses locally with neither a token nor a session', async () => {
    const { calls, fetchImpl } = recordingFetch({ outcome: 'accepted' });
    const { createApiClient, MissingTokenError } = await import('$lib/api');
    const api = createApiClient({ fetchImpl, token: () => null, hasSession: () => false });

    await expect(
      api.write('dishwasher', { feature_key: 'Some.Feature', value: '1' }),
    ).rejects.toBeInstanceOf(MissingTokenError);
    expect(calls, 'nothing may be sent when no credential is held').toHaveLength(0);
  });

  it('prefers a held token over the session, so an explicit token always wins', async () => {
    const { calls, fetchImpl } = recordingFetch({ outcome: 'accepted' });
    const { createApiClient, TOKEN_HEADER } = await import('$lib/api');
    const api = createApiClient({
      fetchImpl,
      token: () => 'operator-token',
      hasSession: () => true,
    });

    await api.write('dishwasher', { feature_key: 'Some.Feature', value: '1' });

    const headers = calls[0]?.init?.headers as Record<string, string>;
    expect(headers[TOKEN_HEADER]).toBe('operator-token');
  });
});

describe('cookies_are_attached_to_every_request', () => {
  it("sets credentials: 'same-origin' on reads and writes alike", async () => {
    const { calls, fetchImpl } = recordingFetch([]);
    const { createApiClient } = await import('$lib/api');
    const api = createApiClient({ fetchImpl, hasSession: () => true });

    await api.devices();
    await api.session();
    await api.write('dishwasher', { feature_key: 'Some.Feature', value: '1' });

    expect(calls).toHaveLength(3);
    for (const call of calls) {
      expect(call.init?.credentials, `${call.url} must send the session cookie`).toBe(
        'same-origin',
      );
    }
  });
});
