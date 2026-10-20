import { describe, expect, it, vi } from 'vitest';
import { TOKEN_HEADER } from '$lib/api';
import { click, type } from '$lib/dom-testing';
import {
  commitSpy,
  failed,
  onboard,
  preview,
  readProfile,
  result,
  wizard,
} from './onboarding-fixtures';

/**
 * Phase 2: what the confirmation sends, and what the wizard does with the verdict.
 *
 * As in `api.test.ts`: `base` is `''` on loopback, so only a non-empty prefix can catch a root-absolute
 * URL. `vi.hoisted` is required because `vi.mock`'s factory runs before this module's body.
 */
const { INGRESS_BASE } = vi.hoisted(() => ({ INGRESS_BASE: '/api/hassio_ingress/tokenvalue' }));

vi.mock('$app/paths', () => ({ base: INGRESS_BASE }));

/** Reads a profile, fills the dialog, confirms, and waits for the commit. */
async function confirmWith(
  commit: ReturnType<typeof commitSpy>,
  fill: (view: ReturnType<typeof wizard>) => void,
) {
  const view = wizard({ commit });
  await readProfile(view);
  fill(view);
  click(view.queryPortal('confirm-submit'));
  await vi.waitFor(() => {
    expect(commit).toHaveBeenCalledTimes(1);
  });
  view.flush();
  return view;
}

describe('confirming_sends_the_operators_choices', () => {
  it('sends the edited name, host and port', async () => {
    const commit = commitSpy();
    const view = await confirmWith(commit, (mounted) => {
      type(mounted.queryPortal('confirm-slug'), 'kitchen-dishwasher');
      type(mounted.queryPortal('confirm-host'), '192.0.2.56');
      type(mounted.queryPortal('confirm-port'), '8443');
    });

    expect(commit.mock.calls[0]?.[1]).toEqual({
      slug: 'kitchen-dishwasher',
      host: '192.0.2.56',
      port: 8443,
    });
    view.destroy();
  });

  it('sends a null port when it is left at the transport default', async () => {
    const commit = commitSpy();
    const view = await confirmWith(commit, (mounted) => {
      type(mounted.queryPortal('confirm-host'), '192.0.2.56');
    });

    // The field says 443 because that is what was prefilled. Sending it as an explicit override would
    // pin the number, so a later change to the transport's default would not follow.
    expect(commit.mock.calls[0]?.[1].port).toBeNull();
    view.destroy();
  });

  it('sends the host verbatim — no trimming, no lower-casing', async () => {
    // Deliberately awkward: surrounding whitespace, mixed case and a trailing root dot. None of it is
    // the client's business; the daemon decides what counts as a host.
    const typed = '  Dishwasher.LAN.  ';
    const commit = commitSpy();
    const view = await confirmWith(commit, (mounted) => {
      type(mounted.queryPortal('confirm-host'), typed);
    });

    expect(commit.mock.calls[0]?.[1].host).toBe(typed);
    view.destroy();
  });
});

describe('an_absent_host_means_the_derived_one', () => {
  it('sends a null host when the field is left empty', async () => {
    const commit = commitSpy();
    const view = await confirmWith(commit, (mounted) => {
      type(mounted.queryPortal('confirm-host'), '   ');
    });

    // Whitespace-only counts as empty. That is a judgement about *emptiness*, not about the address.
    expect(commit.mock.calls[0]?.[1].host).toBeNull();
    view.destroy();
  });
});

describe('the_dialog_survives_a_refusal', () => {
  it('stays open and shows the reason', async () => {
    const view = wizard({
      commit: () =>
        Promise.reject(new Error('another appliance is already onboarded as `dishwasher`')),
    });
    await readProfile(view);

    click(view.queryPortal('confirm-submit'));
    await vi.waitFor(() => {
      expect(view.queryPortal('confirm-error')).not.toBeNull();
    });
    view.flush();

    // The fix for an occupied name is made *here*; closing would send the operator back to the file
    // picker for a mistake they can correct in the field in front of them.
    expect(view.queryPortal('confirm-error')?.textContent).toContain('already onboarded');
    expect(view.queryPortal('confirm-slug')).not.toBeNull();
    expect(view.query('result-slug')).toBeNull();
    view.destroy();
  });

  it('closes and renders the result on success', async () => {
    const view = wizard();
    await onboard(view);

    expect(view.query('result-slug')?.textContent).toContain('dishwasher');
    expect(view.query('result-entity-count')?.textContent).toContain('270');
    expect(view.query('result-connection')?.textContent).toContain('connected');
    expect(view.queryPortal('confirm-slug')).toBeNull();
    view.destroy();
  });
});

describe('a_failed_connection_test_still_added_the_appliance', () => {
  it('says so, and emphasises the correction', async () => {
    const view = wizard({ commit: () => Promise.resolve(failed) });
    await onboard(view);

    // A powered-off appliance does not cost the operator the upload.
    expect(view.query('saved-anyway')).not.toBeNull();
    expect(view.query('manual-host-prompt')?.classList.contains('emphasised')).toBe(true);
    expect(view.query('manual-host-reason')?.textContent).toContain('Not reachable');
    view.destroy();
  });

  it('does not emphasise the correction when the appliance answered', async () => {
    const view = wizard();
    await onboard(view);

    // Asserted in both directions so the emphasis cannot pass by being unconditional.
    expect(view.query('saved-anyway')).toBeNull();
    expect(view.query('manual-host')).not.toBeNull();
    expect(view.query('manual-host-prompt')?.classList.contains('emphasised')).toBe(false);
    view.destroy();
  });

  it('drops the emphasis once a corrected host connects', async () => {
    const setManualHost = vi.fn(() => Promise.resolve(result({ host_source: 'manual' })));
    const view = wizard({ commit: () => Promise.resolve(failed), setManualHost });
    await onboard(view);
    expect(view.query('manual-host-prompt')?.classList.contains('emphasised')).toBe(true);

    type(view.query('manual-host'), '192.0.2.99');
    click(view.query('manual-host-submit'));
    // Waited on the rendered verdict, not on the spy: the call resolves before the component has applied
    // its result, so asserting on the mock would race the re-render. `host_source` is no use as the
    // signal — `failed` already reports `manual` — so the connection status is what actually changes.
    await vi.waitFor(() => {
      expect(view.query('result-connection')?.textContent).toContain('connected');
    });
    view.flush();

    expect(setManualHost).toHaveBeenCalledWith('dishwasher', '192.0.2.99');
    expect(view.query('manual-host')).not.toBeNull();
    expect(view.query('manual-host-prompt')?.classList.contains('emphasised')).toBe(false);
    view.destroy();
  });
});

describe('the_correction_is_gated_on_having_a_device', () => {
  it('offers no correction before anything has been added', () => {
    const view = wizard();
    // There is no slug to post to until a commit has returned one.
    expect(view.query('manual-host')).toBeNull();
    expect(view.query('manual-host-submit')).toBeNull();
    view.destroy();
  });
});

/** A `fetch` stand-in that records what it was asked for and always answers with `body`. */
function recordingFetch(body: unknown) {
  const calls: { url: string; init: RequestInit | undefined }[] = [];
  const fetchImpl = (url: string, init?: RequestInit): Promise<Response> => {
    calls.push({ url, init });
    return Promise.resolve(
      new Response(JSON.stringify(body), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
    );
  };
  return { calls, fetchImpl };
}

describe('the_commit_reaches_the_daemon_over_the_real_client', () => {
  it('posts the ZIP with the choices in the query string, the token in the header, under `base`', async () => {
    const { calls, fetchImpl } = recordingFetch(result());
    const { createApiClient } = await import('$lib/api');
    const api = createApiClient({ fetchImpl, token: () => 'operator-token' });

    const view = wizard({
      preview: () => Promise.resolve(preview()),
      commit: (zip, request) => api.commitProfile(zip, request),
    });
    await readProfile(view);

    // An IPv6 literal: `:` and the zone-id `%` must be percent-encoded to survive the query string,
    // which is why the client composes it with `URLSearchParams` rather than by interpolation.
    type(view.queryPortal('confirm-host'), 'fe80::1%eth0');
    type(view.queryPortal('confirm-port'), '8443');
    click(view.queryPortal('confirm-submit'));
    await vi.waitFor(() => {
      expect(calls).toHaveLength(1);
    });
    view.flush();

    const sent = calls[0];
    expect(sent?.init?.method).toBe('POST');
    expect(new Headers(sent?.init?.headers).get(TOKEN_HEADER)).toBe('operator-token');
    // Built from `base`, so the add-on works under ingress.
    expect(sent?.url.startsWith(`${INGRESS_BASE}/api/onboarding/commit?`)).toBe(true);

    // The address survives encoding intact — read back through `URLSearchParams`, which is what the
    // daemon's own query extractor does.
    const query = new URLSearchParams(sent?.url.split('?')[1] ?? '');
    expect(query.get('slug')).toBe('dishwasher');
    expect(query.get('host')).toBe('fe80::1%eth0');
    expect(query.get('port')).toBe('8443');

    // The token is never in the URL.
    expect(sent?.url).not.toContain('operator-token');
    view.destroy();
  });
});
