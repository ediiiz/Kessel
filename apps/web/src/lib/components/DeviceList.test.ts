import { describe, expect, it } from 'vitest';
import type { DeviceSummary } from '$lib/api-types';
import { render } from '$lib/dom-testing';
import DeviceList from './DeviceList.svelte';

/**
 * The fleet as `GET /api/devices` reports it.
 *
 * The mixed `ever_writable` values are a fixture, not a claim about the real fleet: the component has
 * to render both kinds of device, and an all-writable fixture would not exercise the read-only marker.
 */
const FLEET: DeviceSummary[] = [
  {
    slug: 'cooktop',
    role: 'cooktop',
    ever_writable: false,
    connection_state: 'connected',
    last_failure: null,
    entity_count: 112,
    brand: 'BOSCH',
    vib: 'SMD8TCX01E',
  },
  {
    slug: 'oven',
    role: 'oven',
    ever_writable: false,
    connection_state: 'reconnecting',
    last_failure: null,
    entity_count: 305,
    brand: 'BOSCH',
    vib: 'SMD8TCX01E',
  },
  {
    slug: 'dishwasher',
    role: 'dishwasher',
    ever_writable: true,
    connection_state: 'handshake',
    last_failure: null,
    entity_count: 270,
    brand: 'BOSCH',
    vib: 'SMD8TCX01E',
  },
  {
    slug: 'fridgefreezer',
    role: 'fridgefreezer',
    ever_writable: true,
    connection_state: 'offline',
    last_failure: null,
    entity_count: 88,
    brand: 'BOSCH',
    vib: 'SMD8TCX01E',
  },
];

function rowFor(view: ReturnType<typeof render>, slug: string): HTMLElement {
  const row = view.queryAll('device-row').find((candidate) => candidate.dataset.slug === slug);
  if (row === undefined) {
    throw new Error(`no row for ${slug}`);
  }
  return row;
}

describe('dashboard_lists_devices_with_connection_state', () => {
  it('renders one card per appliance, each with its connection state', () => {
    const view = render(DeviceList, { devices: FLEET });

    const rows = view.queryAll('device-row');
    expect(rows).toHaveLength(4);
    expect(rows.map((row) => row.dataset.slug)).toStrictEqual([
      'cooktop',
      'oven',
      'dishwasher',
      'fridgefreezer',
    ]);
    // The daemon's own word for the state, verbatim, in its own element. The card also renders a
    // colour and a prose hint, but neither replaces the raw state, so the state is never reduced to
    // a colour an operator has to decode.
    expect(
      view.queryAll('device-connection-state').map((cell) => cell.textContent?.trim()),
    ).toStrictEqual(['connected', 'reconnecting', 'handshake', 'offline']);
    expect(
      view.queryAll('device-entity-count').map((cell) => cell.textContent?.trim()),
    ).toStrictEqual(['112', '305', '270', '88']);
    view.destroy();
  });

  it("links each card to that device's entity list, through `resolve()`", () => {
    const view = render(DeviceList, { devices: FLEET });

    const link = rowFor(view, 'dishwasher').querySelector<HTMLAnchorElement>(
      '[data-testid="device-link"]',
    );
    // Hash-routed, and the href must be composed by `resolve()` so it stays inside an ingress
    // prefix. At the test base of '' that is exactly `#/devices/…`.
    expect(link?.getAttribute('href')).toBe('#/devices/dishwasher');
    view.destroy();
  });

  it('says so, rather than rendering an empty grid, when no device is onboarded', () => {
    const view = render(DeviceList, { devices: [] });

    expect(view.queryAll('device-row')).toHaveLength(0);
    expect(view.query('no-devices')).not.toBeNull();
    view.destroy();
  });
});

/**
 * The fleet screen carries no write path at all: nothing operable anywhere in it, for any device.
 *
 * Writes live on the device page (see `DeviceList.svelte`'s header); the click-only, send-once,
 * refusal-rendered-verbatim behaviour is covered in `WriteControl.test.ts`.
 */
describe('fleet_screen_renders_no_write_path', () => {
  it('renders no operable control for any device, writable or not', () => {
    const view = render(DeviceList, { devices: FLEET });

    // Against the whole component, not per row: no device is expected to have a control.
    expect(
      view.container.querySelectorAll('button, input, select, textarea, [contenteditable]'),
    ).toHaveLength(0);
    expect(view.queryAll('write-control')).toHaveLength(0);
    view.destroy();
  });

  it('marks a never-writable device as read-only, and does not mark a writable one', () => {
    const view = render(DeviceList, { devices: FLEET });

    for (const slug of ['cooktop', 'oven']) {
      expect(rowFor(view, slug).querySelector('[data-testid="never-writable"]')).not.toBeNull();
    }
    for (const slug of ['dishwasher', 'fridgefreezer']) {
      expect(rowFor(view, slug).querySelector('[data-testid="never-writable"]')).toBeNull();
    }
    view.destroy();
  });
});
