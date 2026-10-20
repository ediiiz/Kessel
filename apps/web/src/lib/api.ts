/**
 * The one place the SPA talks to the Kessel daemon.
 *
 *   1. Every URL is composed from `base` in `$app/paths`. Under Home Assistant ingress `base` is
 *      `/api/hassio_ingress/<token>`, computed at load time from the document URL (see
 *      `svelte.config.js`). A root-absolute `fetch('/api/devices')` works on loopback and silently
 *      leaves the add-on under ingress.
 *   2. Types come from `$lib/api-types`, never from `@kessel/api-types` or `src/generated/`.
 *
 * The HTTP plumbing lives in `$lib/api-http`, and the session and onboarding routes in
 * `$lib/api-session` and `$lib/api-onboarding`. Everything public from `api-http` is re-exported
 * below, so components import from this module only.
 */

import type {
  CommitRequest,
  ConfigUpdate,
  ConfigView,
  DeviceSummary,
  EntityDescriptor,
  Health,
  ManualHostRequest,
  OnboardingResult,
  ProfilePreview,
  ProgramDescriptor,
  RestartOutcome,
  SessionView,
  WriteOutcome,
  WriteRequest,
} from '$lib/api-types';
import {
  type AuthHeaders,
  apiUrl,
  type FetchLike,
  makeAuthHeaders,
  networkAware,
  readJson,
  parseJsonBody,
  errorMessage,
  ApiRequestError,
  type TokenSource,
} from '$lib/api-http';
import { onboardingRoutes } from '$lib/api-onboarding';
import { sessionRoutes } from '$lib/api-session';

// Re-exported so components and tests import from `$lib/api` alone.
export {
  ApiRequestError,
  apiUrl,
  type AuthHeaders,
  DaemonUnreachableError,
  type FetchLike,
  MissingTokenError,
  TOKEN_HEADER,
  type TokenSource,
} from '$lib/api-http';

/** The largest profile ZIP the daemon accepts (16 MiB). */
export const MAX_PROFILE_BYTES = 16 * 1024 * 1024;

export interface ApiClientOptions {
  /** Defaults to the global `fetch`. */
  fetchImpl?: FetchLike;
  /** Defaults to "no token", which makes every mutating call refuse locally. */
  token?: TokenSource;
  /**
   * Whether a browser session is open.
   *
   * When it is, a mutating call needs no token: the daemon accepts the `HttpOnly` session cookie the
   * browser attaches on its own, and this module cannot see that cookie, hence a predicate rather than a
   * value. Defaults to "no session".
   */
  hasSession?: () => boolean;
}

/** The daemon's HTTP surface, as the UI uses it. */
export interface ApiClient {
  health(): Promise<Health>;
  devices(): Promise<DeviceSummary[]>;
  entities(slug: string): Promise<EntityDescriptor[]>;
  /**
   * The appliance's programmes, named.
   *
   * A read, and the only programme route. There is deliberately no `startProgram`: the daemon exposes no
   * handler for one, and starting a programme on a real cooktop is not a UI affordance.
   */
  programs(slug: string): Promise<ProgramDescriptor[]>;
  /**
   * Phase 1: parse a profile ZIP and report what it says. **Dials nothing, stores nothing.**
   *
   * Mutating by method only: it is a `POST` because it carries a body, and the daemon gates on method,
   * so it needs the credential like any other `POST`.
   */
  previewProfile(zip: Blob): Promise<ProfilePreview>;
  /**
   * Phase 2: onboard the appliance on the operator's confirmed terms.
   *
   * The **same ZIP** is posted again, with the choices in the query string. It is not held server-side
   * between the phases: a staging area would need an expiry, and an expiry is a way for a confirmation
   * dialog left open over lunch to fail.
   *
   * Resolves even when the appliance did not answer — `result.connection` carries that verdict, and the
   * device is stored either way so a powered-off appliance does not cost the upload.
   */
  commitProfile(zip: Blob, request: CommitRequest): Promise<OnboardingResult>;
  setManualHost(slug: string, host: string): Promise<OnboardingResult>;
  /** Forgets a device, on the daemon's disk as well as in its registry. */
  deleteDevice(slug: string): Promise<void>;
  write(slug: string, request: WriteRequest): Promise<WriteOutcome>;
  /** The stored configuration. Never carries the broker password — see `ConfigView`. */
  config(): Promise<ConfigView>;
  /**
   * Replaces the stored configuration and returns the new view.
   *
   * Omitting `mqtt_password` **keeps** the stored one; sending `''` clears it. That is why the form does
   * not send an empty password field by default: a UI cannot read the password back, so sending what it has
   * would wipe it on every save.
   */
  saveConfig(update: ConfigUpdate): Promise<ConfigView>;
  /**
   * Restarts the daemon so a saved setting takes effect.
   *
   * Resolves as soon as the daemon has accepted the request (202), before anything happens. `mechanism` says what is about to
   * happen: `supervisor` means Home Assistant restarts the add-on, `process_exit` means the process ends
   * and whatever started it has to start it again. Only the first promises Kessel comes back, so a caller
   * must not render the two the same way.
   */
  restart(): Promise<RestartOutcome>;
  /** Whether this browser's session cookie currently authorises a mutating call. */
  session(): Promise<SessionView>;
  /**
   * Exchanges the operator token for a session cookie. On success the caller should **discard its copy of
   * the token**: the browser holds an `HttpOnly` credential from then on, which no script can read.
   */
  signIn(token: string): Promise<void>;
  /** Ends this browser's session, server-side and in the browser. */
  signOut(): Promise<void>;
}

/** The read routes. These carry no token: the daemon only gates mutating methods. */
function readRoutes(
  doFetch: FetchLike,
): Pick<ApiClient, 'health' | 'devices' | 'entities' | 'programs' | 'config'> {
  const get = async <T>(path: string): Promise<T> => readJson<T>(await doFetch(apiUrl(path)));
  return {
    health: () => get<Health>('/health'),
    devices: () => get<DeviceSummary[]>('/devices'),
    entities: (slug) => get<EntityDescriptor[]>(`/devices/${encodeURIComponent(slug)}/entities`),
    programs: (slug) => get<ProgramDescriptor[]>(`/devices/${encodeURIComponent(slug)}/programs`),
    // The response never includes the broker password (`ConfigView`).
    config: () => get<ConfigView>('/config'),
  };
}

/** The two routes that address an existing device. */
function deviceRoutes(
  doFetch: FetchLike,
  authHeaders: AuthHeaders,
): Pick<ApiClient, 'setManualHost' | 'write'> {
  return {
    setManualHost: async (slug, host) => {
      const body: ManualHostRequest = { host };
      const response = await doFetch(
        apiUrl(`/onboarding/${encodeURIComponent(slug)}/host`),
        jsonPost(body, authHeaders('application/json')),
      );
      return readJson<OnboardingResult>(response);
    },

    write: async (slug, request) => {
      const response = await doFetch(
        apiUrl(`/devices/${encodeURIComponent(slug)}/write`),
        jsonPost(request, authHeaders('application/json')),
      );
      return readWriteOutcome(response);
    },
  };
}

/** The settings write and the restart; separate to keep each route group within its length limit. */
function configRoute(
  doFetch: FetchLike,
  authHeaders: AuthHeaders,
): Pick<ApiClient, 'saveConfig' | 'restart'> {
  return {
    restart: async () => {
      const response = await doFetch(
        apiUrl('/restart'),
        // An empty body. It is a POST because it mutates and so needs the credential: an ungated
        // restart would let anything that can reach the port drop every appliance connection.
        jsonPost({}, authHeaders('application/json')),
      );
      return readJson<RestartOutcome>(response);
    },

    saveConfig: async (update) => {
      const response = await doFetch(apiUrl('/config'), {
        ...jsonPost(update, authHeaders('application/json')),
        method: 'PUT',
      });
      return readJson<ConfigView>(response);
    },
  };
}

export function createApiClient(options: ApiClientOptions = {}): ApiClient {
  // Not named `base`, to avoid confusion with the ingress prefix from `$app/paths` that `apiUrl` uses.
  const underlying = options.fetchImpl ?? ((url, init) => globalThis.fetch(url, init));
  // `same-origin` is `fetch`'s default but set explicitly, because the session cookie carries
  // authorisation and a changed default would fail as a silent 401. Applied to reads too, since
  // `GET /api/session` needs the cookie.
  // `networkAware` is the outer layer so it sees a transport rejection from every route; see
  // `DaemonUnreachableError`.
  const doFetch: FetchLike = networkAware((url, init) =>
    underlying(url, { ...init, credentials: 'same-origin' }),
  );
  const authHeaders = makeAuthHeaders(
    options.token ?? (() => null),
    options.hasSession ?? (() => false),
  );
  return {
    ...readRoutes(doFetch),
    ...onboardingRoutes(doFetch, authHeaders),
    ...deviceRoutes(doFetch, authHeaders),
    ...configRoute(doFetch, authHeaders),
    ...sessionRoutes(doFetch),
  };
}

function jsonPost(body: unknown, headers: Record<string, string>): RequestInit {
  return {
    method: 'POST',
    headers,
    body: JSON.stringify(body),
  };
}

/**
 * A write's 403 and 503 both carry a `WriteOutcome::Refused` body, not an `ApiError`. A 400 or 404
 * is still an `ApiError`, so the discriminator is the body, not the status.
 */
async function readWriteOutcome(response: Response): Promise<WriteOutcome> {
  const body = await parseJsonBody(response);
  if (typeof body === 'object' && body !== null && 'outcome' in body) {
    return body as WriteOutcome;
  }
  throw new ApiRequestError(response.status, errorMessage(body, response.status));
}
