import { describe, expect, it, vi } from 'vitest';

/**
 * The programme-start route. `$app/paths` is mocked to an ingress prefix, as in `api.test.ts`, so a
 * root-absolute URL would fail here.
 */
const { INGRESS_BASE } = vi.hoisted(() => ({ INGRESS_BASE: '/api/hassio_ingress/tokenvalue' }));

vi.mock('$app/paths', () => ({ base: INGRESS_BASE }));

describe('start_program_posts_to_the_start_route', () => {
  it('posts the request to the device programme-start route with the credential', async () => {
    const calls: { url: string; init: RequestInit | undefined }[] = [];
    const fetchImpl = (url: string, init?: RequestInit): Promise<Response> => {
      calls.push({ url, init });
      return Promise.resolve(
        new Response(JSON.stringify({ outcome: 'accepted' }), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        }),
      );
    };
    const { createApiClient } = await import('./api');
    const api = createApiClient({ fetchImpl, token: () => 'operator-token' });

    const outcome = await api.startProgram('dishwasher', {
      program: 'Dishcare.Dishwasher.Program.Eco50',
    });

    expect(outcome).toStrictEqual({ outcome: 'accepted' });
    expect(calls).toHaveLength(1);
    const [call] = calls;
    expect(call?.url).toBe(`${INGRESS_BASE}/api/devices/dishwasher/program/start`);
    expect(call?.init?.method).toBe('POST');
    expect(call?.init?.body).toBe('{"program":"Dishcare.Dishwasher.Program.Eco50"}');
  });
});

describe('stop_program_posts_to_the_stop_route', () => {
  it('posts an empty body to the device programme-stop route', async () => {
    const calls: { url: string; init: RequestInit | undefined }[] = [];
    const fetchImpl = (url: string, init?: RequestInit): Promise<Response> => {
      calls.push({ url, init });
      return Promise.resolve(
        new Response(JSON.stringify({ outcome: 'accepted' }), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        }),
      );
    };
    const { createApiClient } = await import('./api');
    const api = createApiClient({ fetchImpl, token: () => 'operator-token' });

    expect(await api.stopProgram('dishwasher')).toStrictEqual({ outcome: 'accepted' });
    expect(calls.map((call) => call.url)).toStrictEqual([
      `${INGRESS_BASE}/api/devices/dishwasher/program/stop`,
    ]);
    expect(calls[0]?.init?.method).toBe('POST');
  });
});
