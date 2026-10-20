/**
 * The onboarding routes, split out of `$lib/api` to keep that file within its line limit.
 *
 * Onboarding is two requests: `preview` parses and reports, `commit` carries the operator's confirmed
 * choices. `deleteDevice` lives here because it exists for onboarding's sake: a failed connection test
 * still persists the device, so removing one has to be possible.
 *
 * Every URL is composed from `base` through `apiUrl`, and types come from `$lib/api-types`.
 */

import type { CommitRequest, OnboardingResult, ProfilePreview } from '$lib/api-types';
import {
  ApiRequestError,
  apiUrl,
  type AuthHeaders,
  errorMessage,
  type FetchLike,
  parseJsonBody,
  readJson,
} from '$lib/api-http';
import { MAX_PROFILE_BYTES, type ApiClient } from '$lib/api';

/**
 * Refuses a ZIP the daemon would reject with a bare `413`.
 *
 * The daemon's rejection is a bare status rather than an `ApiError`, so checking here names the real
 * problem and avoids uploading tens of MiB only to have them dropped. A plain `Error` is enough: the
 * wizard renders `error.message`.
 */
function refuseOversizedZip(zip: Blob): void {
  if (zip.size > MAX_PROFILE_BYTES) {
    throw new Error(
      `the profile ZIP is ${String(zip.size)} bytes; the daemon accepts at most ` +
        `${String(MAX_PROFILE_BYTES)} (16 MiB). Choose a smaller export.`,
    );
  }
}

/**
 * The commit's choices as a query string.
 *
 * `URLSearchParams` rather than template interpolation, because the host is operator input: an IPv6
 * literal carries `:` and a zone id carries `%`, both of which must be percent-encoded to survive the
 * URL. The value is not otherwise touched: the daemon decides what counts as a host, and a well-meant
 * client-side "fix" is how a working address becomes a broken one.
 *
 * `host` and `port` are omitted when absent rather than sent empty, so the daemon sees `None` and applies
 * its documented fallbacks (the derived host, the transport's default port).
 */
function commitQuery(request: CommitRequest): string {
  const query = new URLSearchParams({ slug: request.slug });
  if (request.host !== null && request.host !== '') {
    query.set('host', request.host);
  }
  if (request.port !== null) {
    query.set('port', String(request.port));
  }
  return query.toString();
}

/** The onboarding routes. Each one carries the credential; each one is caller-initiated. */
export function onboardingRoutes(
  doFetch: FetchLike,
  authHeaders: AuthHeaders,
): Pick<ApiClient, 'previewProfile' | 'commitProfile' | 'deleteDevice'> {
  return {
    previewProfile: async (zip) => {
      refuseOversizedZip(zip);
      const response = await doFetch(apiUrl('/onboarding/preview'), {
        method: 'POST',
        headers: authHeaders('application/zip'),
        body: zip,
      });
      return readJson<ProfilePreview>(response);
    },

    commitProfile: async (zip, request) => {
      refuseOversizedZip(zip);
      const response = await doFetch(apiUrl(`/onboarding/commit?${commitQuery(request)}`), {
        method: 'POST',
        headers: authHeaders('application/zip'),
        body: zip,
      });
      return readJson<OnboardingResult>(response);
    },

    deleteDevice: async (slug) => {
      const response = await doFetch(apiUrl(`/devices/${encodeURIComponent(slug)}`), {
        method: 'DELETE',
        headers: authHeaders(),
      });
      // Success is a bare 200 with no body, which `readJson` would reject. A failure does carry an
      // `ApiError` (a 409 means "that device is connected; restart first"), so its message is kept.
      if (!response.ok) {
        const body = await parseJsonBody(response);
        throw new ApiRequestError(response.status, errorMessage(body, response.status));
      }
    },
  };
}
