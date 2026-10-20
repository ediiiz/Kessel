import { describe, expect, it, vi } from 'vitest';
import { TOKEN_HEADER } from './api';

/**
 * Under hash routing `$app/paths`' `base` is computed at runtime from the document URL, so under
 * Home Assistant ingress it is `/api/hassio_ingress/<token>`. Mocking it to a prefix is the only way
 * to catch a root-absolute URL in a test: at the loopback base of `''` a hard-coded `/api/devices`
 * and a correctly composed one are the same string.
 *
 * `vi.hoisted` is required because `vi.mock`'s factory runs before this module's body.
 */
const { INGRESS_BASE } = vi.hoisted(() => ({ INGRESS_BASE: '/api/hassio_ingress/tokenvalue' }));

vi.mock('$app/paths', () => ({ base: INGRESS_BASE }));

/** A `fetch` stand-in that records what it was asked for and answers with `body`. */
function recordingFetch(body: unknown, status = 200) {
  const calls: { url: string; init: RequestInit | undefined }[] = [];
  const fetchImpl = (url: string, init?: RequestInit): Promise<Response> => {
    calls.push({ url, init });
    // A write's body must be a `WriteOutcome` whatever else this recorder is answering with.
    const answer = url.endsWith('/write') ? { outcome: 'accepted' } : body;
    return Promise.resolve(
      new Response(JSON.stringify(answer), {
        status,
        headers: { 'content-type': 'application/json' },
      }),
    );
  };
  return { calls, fetchImpl };
}

describe('api_urls_are_built_from_base', () => {
  it('composes every request URL from `base`, so the app survives an ingress prefix', async () => {
    const { calls, fetchImpl } = recordingFetch([]);
    const { createApiClient } = await import('./api');
    const api = createApiClient({ fetchImpl, token: () => 'operator-token' });

    await api.health();
    await api.devices();
    await api.entities('dishwasher');
    const zip = new Blob([new Uint8Array([1, 2])], { type: 'application/zip' });
    await api.previewProfile(zip);
    await api.commitProfile(zip, { slug: 'dishwasher', host: '10.0.0.1', port: null });
    await api.setManualHost('dishwasher', '10.0.0.1');
    await api.write('dishwasher', { feature_key: 'Some.Feature', value: '1' });
    await api.deleteDevice('dishwasher');

    expect(calls).toHaveLength(8);
    for (const call of calls) {
      expect(call.url.startsWith(`${INGRESS_BASE}/api/`)).toBe(true);
    }
    expect(calls.map((call) => call.url)).toStrictEqual([
      `${INGRESS_BASE}/api/health`,
      `${INGRESS_BASE}/api/devices`,
      `${INGRESS_BASE}/api/devices/dishwasher/entities`,
      `${INGRESS_BASE}/api/onboarding/preview`,
      `${INGRESS_BASE}/api/onboarding/commit?slug=dishwasher&host=10.0.0.1`,
      `${INGRESS_BASE}/api/onboarding/dishwasher/host`,
      `${INGRESS_BASE}/api/devices/dishwasher/write`,
      `${INGRESS_BASE}/api/devices/dishwasher`,
    ]);
  });

  it('never puts the token in a URL or a query string', async () => {
    const { calls, fetchImpl } = recordingFetch({ outcome: 'refused', reason: 'no' });
    const { createApiClient } = await import('./api');
    const api = createApiClient({ fetchImpl, token: () => 'operator-token' });

    await api.write('dishwasher', { feature_key: 'Some.Feature', value: '1' });

    for (const call of calls) {
      expect(call.url).not.toContain('operator-token');
      expect(call.url).not.toContain('?');
    }
  });
});

/** Reads the `X-Kessel-Token` header off a recorded call, whatever form `headers` took. */
function tokenHeaderOf(init: RequestInit | undefined): string | null {
  return new Headers(init?.headers).get(TOKEN_HEADER);
}

describe('mutating_requests_send_the_token', () => {
  it('sends `X-Kessel-Token` on every write and onboarding request', async () => {
    const { calls, fetchImpl } = recordingFetch({});
    const { createApiClient } = await import('./api');
    const api = createApiClient({ fetchImpl, token: () => 'operator-token' });

    const zip = new Blob([new Uint8Array([1])], { type: 'application/zip' });
    await api.previewProfile(zip);
    await api.commitProfile(zip, { slug: 'dishwasher', host: null, port: null });
    await api.setManualHost('dishwasher', '10.0.0.1');
    await api.write('dishwasher', { feature_key: 'Some.Feature', value: '1' });

    expect(calls).toHaveLength(4);
    for (const call of calls) {
      expect(call.init?.method).toBe('POST');
      expect(tokenHeaderOf(call.init)).toBe('operator-token');
    }
  });

  it('does not require a token for reads, and sends none', async () => {
    const { calls, fetchImpl } = recordingFetch([]);
    const { createApiClient } = await import('./api');
    const api = createApiClient({ fetchImpl, token: () => null });

    await api.health();
    await api.devices();
    await api.entities('dishwasher');

    expect(calls).toHaveLength(3);
    for (const call of calls) {
      expect(tokenHeaderOf(call.init)).toBeNull();
    }
  });

  it('refuses a mutating call with no token locally, without sending it', async () => {
    const { calls, fetchImpl } = recordingFetch({});
    const { createApiClient, MissingTokenError } = await import('./api');
    const api = createApiClient({ fetchImpl, token: () => null });

    await expect(
      api.write('dishwasher', { feature_key: 'Some.Feature', value: '1' }),
    ).rejects.toThrow(MissingTokenError);
    expect(calls).toHaveLength(0);
  });
});

describe('profile_upload_respects_the_daemons_size_bound', () => {
  it('refuses a ZIP over MAX_PROFILE_BYTES locally, without sending it', async () => {
    const { calls, fetchImpl } = recordingFetch({});
    const { createApiClient, MAX_PROFILE_BYTES } = await import('./api');
    const api = createApiClient({ fetchImpl, token: () => 'operator-token' });

    const tooBig = new Blob([new Uint8Array(MAX_PROFILE_BYTES + 1)], { type: 'application/zip' });
    expect(tooBig.size).toBe(MAX_PROFILE_BYTES + 1);

    // Both phases upload, so both must refuse: phase 2 re-posts the archive, and a bound checked on
    // only one of them would let 16 MiB over the wire on the other.
    await expect(api.previewProfile(tooBig)).rejects.toThrow(/16 MiB/u);
    await expect(
      api.commitProfile(tooBig, { slug: 'dishwasher', host: null, port: null }),
    ).rejects.toThrow(/16 MiB/u);
    // The point of a client-side bound: nothing goes over the wire to be dropped.
    expect(calls).toHaveLength(0);
  });

  it('sends a ZIP exactly at the bound', async () => {
    const { calls, fetchImpl } = recordingFetch({});
    const { createApiClient, MAX_PROFILE_BYTES } = await import('./api');
    const api = createApiClient({ fetchImpl, token: () => 'operator-token' });

    // Boundary: the daemon accepts "at most" 16 MiB, so 16 MiB itself must not be refused here.
    await api.previewProfile(
      new Blob([new Uint8Array(MAX_PROFILE_BYTES)], { type: 'application/zip' }),
    );
    expect(calls).toHaveLength(1);
  });
});

/**
 * A client whose every request answers with `body` — which is *not* JSON — at `status`.
 *
 * Returns the `ApiRequestError` class too, so each case below can assert the type without a second
 * dynamic import.
 */
async function clientAnswering(body: string, status: number, contentType: string) {
  const module = await import('./api');
  const api = module.createApiClient({
    fetchImpl: () =>
      Promise.resolve(new Response(body, { status, headers: { 'content-type': contentType } })),
    token: () => 'operator-token',
  });
  return { api, ApiRequestError: module.ApiRequestError };
}

/** Whatever a call rejected with, as a value — `rejects.toThrow` cannot assert on `.status`. */
function failureFrom(call: Promise<unknown>): Promise<unknown> {
  return call.then(() => null).catch((error: unknown) => error);
}

/** `{ status, message }` of a rejection, so each case below is three lines instead of eight. */
function shapeOf(failure: unknown): { status: unknown; message: string } {
  return {
    status: (failure as { status?: unknown }).status,
    message: (failure as Error).message,
  };
}

/**
 * The daemon is not the only thing that can answer these requests: axum answers a body over its
 * limit with its own plain-text `413`, and a reverse proxy such as Home Assistant ingress can answer
 * with an HTML `502`. Neither may surface as `SyntaxError: Failed to parse JSON` without the status.
 */
describe('a_non_json_error_body_reports_the_status_not_a_parse_error', () => {
  it('reports the status when a write answers with a non-JSON body', async () => {
    // The body and content-type axum returns for a 17 MiB upload.
    const { api, ApiRequestError } = await clientAnswering(
      'Failed to buffer the request body: length limit exceeded',
      413,
      'text/plain; charset=utf-8',
    );

    const failure = await failureFrom(
      api.write('dishwasher', { feature_key: 'Some.Feature', value: '1' }),
    );

    expect(failure).toBeInstanceOf(ApiRequestError);
    expect(shapeOf(failure)).toStrictEqual({ status: 413, message: 'the daemon answered 413' });
    // Not a `SyntaxError: Failed to parse JSON`.
    expect(shapeOf(failure).message).not.toMatch(/JSON|Unexpected/iu);
  });

  it('reports the status when the profile upload answers with an HTML gateway error', async () => {
    const { api, ApiRequestError } = await clientAnswering(
      '<html><body>502 Bad Gateway</body></html>',
      502,
      'text/html',
    );

    const failure = await failureFrom(
      api.previewProfile(new Blob([new Uint8Array([1])], { type: 'application/zip' })),
    );

    expect(failure).toBeInstanceOf(ApiRequestError);
    expect(shapeOf(failure)).toStrictEqual({ status: 502, message: 'the daemon answered 502' });
  });
});

/**
 * The `200` case has its own block: three `it`s in one `describe` trips `max-lines-per-function`.
 *
 * The daemon itself never answers an API path with the SPA fallback (an unknown route is a JSON
 * `404`), but a proxy can answer `200` with an HTML interstitial. Parsing that as `DeviceSummary[]`
 * must not surface as a `SyntaxError`.
 */
describe('a_non_json_success_body_reports_the_status_not_a_parse_error', () => {
  it('reports the status when a *successful* read answers with a non-JSON body', async () => {
    const { api, ApiRequestError } = await clientAnswering(
      '<!doctype html><title>Kessel</title>',
      200,
      'text/html',
    );

    const failure = await failureFrom(api.devices());

    expect(failure).toBeInstanceOf(ApiRequestError);
    expect(shapeOf(failure)).toStrictEqual({
      status: 200,
      message: 'the daemon answered 200 but the body was not JSON',
    });
  });
});

describe('the restart route', () => {
  it('is a mutating POST that carries the token', async () => {
    const { calls, fetchImpl } = recordingFetch(
      { mechanism: 'supervisor', message: 'restarting' },
      202,
    );
    const { createApiClient } = await import('./api');
    const api = createApiClient({ fetchImpl, token: () => 'operator-token' });

    const outcome = await api.restart();

    expect(calls).toHaveLength(1);
    expect(calls[0]?.url).toBe(`${INGRESS_BASE}/api/restart`);
    expect(calls[0]?.init?.method).toBe('POST');
    expect(tokenHeaderOf(calls[0]?.init)).toBe('operator-token');
    // The UI says something different for each mechanism, because only one promises the daemon
    // comes back.
    expect(outcome.mechanism).toBe('supervisor');
  });
});
