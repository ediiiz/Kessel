/**
 * Shared fixtures and drivers for the two `OnboardingWizard` test files.
 *
 * `OnboardingWizard.test.ts` covers phase 1 and the dialog's own rules;
 * `OnboardingWizard.commit.test.ts` covers what phase 2 sends and what comes back.
 *
 * Not named `*.test.ts`, so vitest's `include` (`src/**\/*.test.ts`) does not collect it as a suite. Only
 * imported from tests, so it never reaches the bundle.
 */
import { expect, vi } from 'vitest';
import type { CommitRequest, OnboardingResult, ProfilePreview } from '$lib/api-types';
import { attachFile, click, render, zipFile, type Mounted } from '$lib/dom-testing';
import OnboardingWizard from './OnboardingWizard.svelte';

/** A `ProfilePreview` as `apps/server/src/types.rs` shapes it. Note: **no host field**. */
export function preview(overrides: Partial<ProfilePreview> = {}): ProfilePreview {
  return {
    role: 'dishwasher',
    suggested_slug: 'dishwasher',
    existing_slug: null,
    entity_count: 270,
    skipped_entity_count: 3,
    transport: 'tls_psk',
    default_port: 443,
    ...overrides,
  };
}

/** An `OnboardingResult` as the daemon returns it. */
export function result(overrides: Partial<OnboardingResult> = {}): OnboardingResult {
  return {
    device: {
      slug: 'dishwasher',
      role: 'dishwasher',
      ever_writable: true,
      connection_state: 'connected',
      last_failure: null,
      entity_count: 270,
      brand: 'BOSCH',
      vib: 'SMD8TCX01E',
    },
    skipped_entity_count: 3,
    host_source: 'derived',
    connection: { status: 'connected' },
    ...overrides,
  };
}

/**
 * A failing connection test, in the `ConnectionTest` union's shape.
 *
 * `host_source` is `manual` because the operator supplied the address that failed — which is why a test
 * cannot use that field as the signal that a *correction* landed.
 */
export const failed = result({
  host_source: 'manual',
  connection: {
    status: 'failed',
    reason: 'the appliance did not answer at that address',
    needs_manual_ip: true,
  },
});

export interface Props extends Record<string, unknown> {
  preview: (zip: File) => Promise<ProfilePreview>;
  commit: (zip: File, request: CommitRequest) => Promise<OnboardingResult>;
  setManualHost: (slug: string, host: string) => Promise<OnboardingResult>;
}

/** The wizard with every prop defaulted to a resolving stub, overridable per test. */
export function wizard(overrides: Partial<Props> = {}): Mounted {
  return render(OnboardingWizard, {
    preview: () => Promise.resolve(preview()),
    commit: () => Promise.resolve(result()),
    setManualHost: () => Promise.resolve(result({ host_source: 'manual' })),
    ...overrides,
  } satisfies Props);
}

/** A `commit` spy typed with both parameters, so `mock.calls[0][1]` is readable. */
export function commitSpy(reply: OnboardingResult = result()) {
  return vi.fn((_zip: File, _request: CommitRequest) => Promise.resolve(reply));
}

/**
 * Phase 1: pick a file, click, and wait for the confirmation dialog.
 *
 * The dialog is found with `queryPortal` because `bits-ui` renders it through a portal attached to
 * `document.body`, outside the mount container.
 */
export async function readProfile(view: Mounted): Promise<void> {
  attachFile(view.query('profile-zip'), zipFile());
  click(view.query('upload'));
  await vi.waitFor(() => {
    expect(view.queryPortal('confirm-slug')).not.toBeNull();
  });
  view.flush();
}

/** Phase 1 and 2: read, then confirm, and wait for the result panel. */
export async function onboard(view: Mounted): Promise<void> {
  await readProfile(view);
  click(view.queryPortal('confirm-submit'));
  await vi.waitFor(() => {
    expect(view.query('result-slug')).not.toBeNull();
  });
  view.flush();
}

/** The `value` of a portalled `<input>`, or `null` if it is not one. */
export function valueOf(element: HTMLElement | null): string | null {
  return element instanceof HTMLInputElement ? element.value : null;
}
