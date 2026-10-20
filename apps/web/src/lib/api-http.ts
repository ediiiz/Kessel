/**
 * The HTTP plumbing every Kessel API call shares: URL composition, the token header, the error
 * classes, and JSON body handling.
 *
 * This module knows how to speak HTTP to the daemon; `$lib/api` knows which routes exist and
 * re-exports everything public here, so components import from `$lib/api` only.
 *
 * Every URL is composed from `base` so an ingress prefix survives; see `apiUrl`.
 */
import { base } from '$app/paths';

/** The header every mutating request carries. */
export const TOKEN_HEADER = 'X-Kessel-Token';

/** Just enough of `fetch` for this client, so a test can substitute a recorder. */
export type FetchLike = (url: string, init?: RequestInit) => Promise<Response>;

/**
 * How the client obtains the operator token.
 *
 * A getter, not a value: the token is entered after the app has loaded, and holding a *reference*
 * to it here rather than a copy means it is never captured in more than one place.
 */
export type TokenSource = () => string | null;

/**
 * A mutating call was attempted with no operator token held.
 *
 * Refused here rather than sent: the daemon would answer 401, and a local error that names the
 * missing input is a better diagnostic.
 */
export class MissingTokenError extends Error {
  constructor() {
    super('the API token is required for this action; paste it in the token field first');
    this.name = 'MissingTokenError';
  }
}

/** A non-2xx response whose body was an `ApiError`. Never carries the token. */
export class ApiRequestError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = 'ApiRequestError';
    this.status = status;
  }
}

/**
 * The request never got an answer: the daemon is stopped, the LAN dropped, ingress closed the tunnel.
 *
 * Distinct from {@link ApiRequestError}, which means the daemon answered and said no. The two call
 * for different actions from the operator: "read what it told you" versus "check that the add-on is
 * running".
 *
 * `fetch` signals a transport failure by rejecting with a `TypeError` whose message differs per
 * browser (`Failed to fetch`, `NetworkError when attempting to fetch resource.`, `Load failed`).
 * Sniffing those strings is fragile; the reliable discriminator is where the rejection came from,
 * which only the fetch wrapper knows. See `networkAware`.
 *
 * The engine's own wording is kept as `cause`: it separates a DNS failure from a CORS refusal in
 * devtools.
 */
export class DaemonUnreachableError extends Error {
  constructor(cause: unknown) {
    super('the daemon could not be reached — check that the Kessel add-on is running');
    this.name = 'DaemonUnreachableError';
    this.cause = cause;
  }
}

/**
 * Wraps a `fetch` so a rejection *from the transport* becomes a {@link DaemonUnreachableError}.
 *
 * Applied once, around the innermost fetch in `createApiClient`, so every route is covered without a
 * call site opting in. A non-2xx answer is not a rejection: it resolves, and `readJson` turns it into
 * an `ApiRequestError`.
 *
 * Errors this module already raised pass through unchanged. In production they cannot reach here,
 * but a substituted `fetchImpl` in a test can throw one, and relabelling it as unreachable would be
 * wrong.
 */
export function networkAware(fetchImpl: FetchLike): FetchLike {
  return async (url, init) => {
    try {
      return await fetchImpl(url, init);
    } catch (error) {
      if (error instanceof ApiRequestError || error instanceof MissingTokenError) {
        throw error;
      }
      throw new DaemonUnreachableError(error);
    }
  };
}

/** `${base}/api${path}` — the only place a request URL is built. */
export function apiUrl(path: string): string {
  return `${base}/api${path}`;
}

/**
 * A body that could not be parsed as JSON. A distinct sentinel rather than `null` or `undefined`,
 * both of which a well-formed JSON body can legitimately be.
 */
const NOT_JSON = Symbol('not-json');

/**
 * `response.json()` rejects on a non-JSON body, and the daemon can send one: axum answers an
 * oversized upload with a plain-text `413`, and a reverse proxy such as Home Assistant ingress may
 * answer `502`/`504` in HTML. Returning a sentinel instead of rethrowing preserves the HTTP status,
 * which is the only useful diagnostic left once the body is unusable.
 */
export async function parseJsonBody(response: Response): Promise<unknown> {
  try {
    return (await response.json()) as unknown;
  } catch {
    return NOT_JSON;
  }
}

export async function readJson<T>(response: Response): Promise<T> {
  const body = await parseJsonBody(response);
  if (!response.ok) {
    throw new ApiRequestError(response.status, errorMessage(body, response.status));
  }
  if (body === NOT_JSON) {
    throw new ApiRequestError(
      response.status,
      `the daemon answered ${String(response.status)} but the body was not JSON`,
    );
  }
  return body as T;
}

/**
 * Pulls `ApiError.message` out of a failure body, falling back to the status. `NOT_JSON` is a
 * symbol, so it fails the `typeof body === 'object'` guard and takes the status fallback.
 */
export function errorMessage(body: unknown, status: number): string {
  if (typeof body === 'object' && body !== null && 'message' in body) {
    const { message } = body as { message: unknown };
    if (typeof message === 'string') {
      return message;
    }
  }
  return `the daemon answered ${String(status)}`;
}

/**
 * Builds the headers a mutating request needs.
 *
 * `contentType` is optional because `DELETE /api/devices/{slug}` sends no body, and declaring a content
 * type for a body that does not exist would be misleading.
 */
export type AuthHeaders = (contentType?: string) => Record<string, string>;

/**
 * The token header, or a throw. `token` is read at call time — the value is never cached in this
 * module, never logged, never interpolated into a URL and never handed back to a caller.
 *
 * There are two ways to be authorised, and the order matters. A held token wins. Otherwise, if a
 * session is open, no auth header is sent at all: the credential is the `HttpOnly` cookie the browser
 * attaches itself, which no script can read. Only with neither does this throw `MissingTokenError`,
 * so the operator learns what to do before a request is sent.
 */
export function makeAuthHeaders(token: TokenSource, hasSession: () => boolean): AuthHeaders {
  return (contentType) => {
    const headers: Record<string, string> =
      contentType === undefined ? {} : { 'content-type': contentType };
    const value = token();
    if (value !== null && value !== '') {
      headers[TOKEN_HEADER] = value;
      return headers;
    }
    if (hasSession()) {
      return headers;
    }
    throw new MissingTokenError();
  };
}
