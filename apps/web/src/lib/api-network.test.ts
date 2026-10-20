import { describe, expect, it, vi } from 'vitest';

/**
 * What the operator is told when the daemon was never reached at all.
 *
 * Kept apart from `api.test.ts`: that file asks which routes exist and what they send, this one asks
 * what happens when nothing answers, which is a property of the transport and route-independent.
 *
 * `fetch` rejects with a `TypeError` when the request never got an answer, and the UI renders
 * `error.message` behind `error instanceof Error ? error.message : 'the daemon could not be reached'`.
 * A `TypeError` is an `Error`, so without `DaemonUnreachableError` the operator would read the
 * browser's internal string instead.
 */

/** As in `api.test.ts`: mocked to a prefix so a root-absolute URL cannot hide behind an empty base. */
vi.mock('$app/paths', () => ({ base: '/api/hassio_ingress/tokenvalue' }));

/** A `fetch` stand-in that never answers, rejecting the way a real one does. */
function failingFetch(reason: unknown) {
  return (): Promise<Response> => Promise.reject(reason);
}

/** A `fetch` stand-in that *does* answer, so the "daemon said no" path can be told apart. */
function answeringFetch(body: unknown, status: number) {
  return (): Promise<Response> =>
    Promise.resolve(
      new Response(JSON.stringify(body), {
        status,
        headers: { 'content-type': 'application/json' },
      }),
    );
}

/** Runs `attempt` and hands back whatever it rejected with, or `null` if it resolved. */
function rejection(attempt: Promise<unknown>): Promise<unknown> {
  return attempt.then(
    () => null,
    (error: unknown) => error,
  );
}

/**
 * The wordings are engine-specific, which is why more than Chrome's is asserted: matching the message
 * text would work in one browser only. `networkAware` keys on where the rejection came from instead.
 */
const ENGINE_MESSAGES = [
  ['Chrome', 'Failed to fetch'],
  ['Firefox', 'NetworkError when attempting to fetch resource.'],
  ['Safari', 'Load failed'],
  ['undici/node', 'fetch failed'],
] as const;

describe('a daemon that cannot be reached says so', () => {
  for (const [engine, message] of ENGINE_MESSAGES) {
    it(`replaces ${engine}'s "${message}" with a sentence about the daemon`, async () => {
      const { createApiClient } = await import('./api');
      const api = createApiClient({ fetchImpl: failingFetch(new TypeError(message)) });

      const raised = await rejection(api.devices());

      expect(raised).toBeInstanceOf(Error);
      const shown = (raised as Error).message;
      expect(shown).not.toContain(message);
      expect(shown).toContain('daemon');
    });
  }

  it('does the same for a mutating route, because the choke point is shared', async () => {
    const { createApiClient } = await import('./api');
    const api = createApiClient({
      fetchImpl: failingFetch(new TypeError('Failed to fetch')),
      token: () => 'operator-token',
    });

    // Every field, because `ConfigUpdate` requires them all; the values do not matter, since this
    // request never leaves the client.
    const raised = await rejection(
      api.saveConfig({
        mqtt_host: null,
        mqtt_port: null,
        mqtt_username: null,
        mqtt_password: null,
        mqtt_base: null,
        mqtt_discovery: null,
        log_level: 'info',
        require_auth: null,
      }),
    );

    expect((raised as Error).message).not.toContain('Failed to fetch');
    expect((raised as Error).message).toContain('daemon');
  });

  it('is a distinct type, so a caller can tell "no answer" from "answered no"', async () => {
    const { createApiClient, DaemonUnreachableError } = await import('./api');
    const api = createApiClient({ fetchImpl: failingFetch(new TypeError('Failed to fetch')) });

    expect(await rejection(api.health())).toBeInstanceOf(DaemonUnreachableError);
  });

  /**
   * The engine's own wording separates a DNS failure from a CORS refusal, so it is kept as `cause`
   * for devtools.
   */
  it('keeps the underlying rejection as `cause` for diagnosis', async () => {
    const underlying = new TypeError('Failed to fetch');
    const { createApiClient } = await import('./api');
    const api = createApiClient({ fetchImpl: failingFetch(underlying) });

    expect(((await rejection(api.health())) as Error).cause).toBe(underlying);
  });

  /**
   * A 404 that came back as "no such device" is a real diagnostic from a daemon that is *running*.
   * Relabelling it "could not be reached" would send the operator to restart an add-on that is fine.
   */
  it('leaves an error raised from a real response alone', async () => {
    const { createApiClient, ApiRequestError } = await import('./api');
    const api = createApiClient({ fetchImpl: answeringFetch({ message: 'no such device' }, 404) });

    const raised = await rejection(api.entities('nope'));

    expect(raised).toBeInstanceOf(ApiRequestError);
    expect((raised as Error).message).toBe('no such device');
  });

  /**
   * `MissingTokenError` is raised *before* anything is sent, and names the operator's next action
   * ("paste the token"). It must not be relabelled either — the daemon's reachability is not in
   * question when the client refused to send.
   */
  it('leaves a locally refused request alone', async () => {
    const { createApiClient, MissingTokenError } = await import('./api');
    const api = createApiClient({ fetchImpl: failingFetch(new TypeError('Failed to fetch')) });

    expect(await rejection(api.deleteDevice('dishwasher'))).toBeInstanceOf(MissingTokenError);
  });
});
