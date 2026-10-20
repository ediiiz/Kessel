/**
 * The session routes: exchange the operator token for an `HttpOnly` cookie, ask whether one is held,
 * and end it. Kept out of `$lib/api` to stay within its line limit.
 *
 * Nothing here can read or return the session value. The cookie is `HttpOnly`, so the browser attaches
 * it and no script can see it; asking the daemon via `session()` is the only way the SPA can know
 * whether it is authenticated.
 */
import type { SessionView } from '$lib/api-types';
import type { ApiClient } from '$lib/api';
import {
  ApiRequestError,
  apiUrl,
  errorMessage,
  type FetchLike,
  parseJsonBody,
  readJson,
  TOKEN_HEADER,
} from '$lib/api-http';

export function sessionRoutes(
  doFetch: FetchLike,
): Pick<ApiClient, 'session' | 'signIn' | 'signOut'> {
  return {
    session: async () => readJson<SessionView>(await doFetch(apiUrl('/session'))),

    // The token is passed as an argument rather than read from `TokenSource` on purpose: this is the one
    // call that exchanges it, and the caller clears its stored copy immediately afterwards.
    signIn: async (token) => {
      const response = await doFetch(apiUrl('/session'), {
        method: 'POST',
        headers: { [TOKEN_HEADER]: token },
      });
      if (!response.ok) {
        throw new ApiRequestError(
          response.status,
          errorMessage(await parseJsonBody(response), response.status),
        );
      }
    },

    signOut: async () => {
      const response = await doFetch(apiUrl('/session'), { method: 'DELETE' });
      // A 401 here means the session was already gone, which is the state the caller wanted. Anything
      // else is a real failure worth surfacing.
      if (!response.ok && response.status !== 401) {
        throw new ApiRequestError(
          response.status,
          errorMessage(await parseJsonBody(response), response.status),
        );
      }
    },
  };
}
