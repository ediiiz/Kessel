/**
 * Whether this browser holds a Kessel session.
 *
 * A shared store rather than local `$state` per page, because several pages need to know whether a
 * mutating call will be authorised. Under Home Assistant ingress every in-app navigation is a full
 * page load, so this module is re-evaluated often and `checked` starts `false` each time; that is
 * why `refresh` exists. Nothing is persisted here: the `HttpOnly` cookie is the durable state.
 *
 * `authenticated` is the daemon's answer to `GET /api/session`, since scripts cannot read the
 * cookie. A stale `true` grants nothing: the daemon re-checks the cookie on every mutating request.
 */
import type { ApiClient } from '$lib/api';

/**
 * The session flag, and whether it has been established yet.
 *
 * `checked` distinguishes "asked, and there is no session" from "not asked yet", which is what stops the
 * token field flashing on screen for every operator who *is* signed in.
 */
export const session = $state({ authenticated: false, authRequired: true, checked: false });

/**
 * Asks the daemon whether this browser's cookie is live, and records the answer.
 *
 * It also records whether the daemon authenticates at all. That is off by default, since Home
 * Assistant ingress has already authenticated the browser; the daemon then answers
 * `authenticated: true`. `authRequired` lets the sign-in control render nothing in that case.
 */
export async function refresh(api: ApiClient): Promise<void> {
  try {
    const view = await api.session();
    session.authenticated = view.authenticated;
    session.authRequired = view.auth_required;
  } catch {
    // An unreachable daemon is not an authenticated one. The page's own load path reports the
    // connection failure, so it is not reported twice here.
    session.authenticated = false;
  } finally {
    session.checked = true;
  }
}

/**
 * Exchanges `token` for a session cookie and, on success, tells the caller to forget the token.
 *
 * Throws whatever the exchange threw — a wrong token is a `401` the caller renders — and leaves
 * `authenticated` alone in that case, so a failed sign-in does not look like a successful one.
 */
export async function signIn(api: ApiClient, token: string): Promise<void> {
  await api.signIn(token);
  session.authenticated = true;
  session.checked = true;
}

/** Ends the session. `authenticated` goes false even if the daemon had already forgotten it. */
export async function signOut(api: ApiClient): Promise<void> {
  try {
    await api.signOut();
  } finally {
    session.authenticated = false;
    session.checked = true;
  }
}
