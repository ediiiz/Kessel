import { describe, expect, it, vi } from 'vitest';
import type { ConfigUpdate, ConfigView } from '$lib/api-types';
import { click, render } from '$lib/dom-testing';
import ConfigForm from './ConfigForm.svelte';

const STORED: ConfigView = {
  mqtt_host: '192.0.2.2',
  mqtt_port: 1884,
  mqtt_username: 'kessel',
  mqtt_password_set: true,
  mqtt_base: 'kessel',
  mqtt_discovery: 'curated',
  log_level: 'debug',
  require_auth: false,
  overridden_by_env: [],
  restart_required: true,
  mqtt: { source: 'stored', host: '192.0.2.2', port: 1884, autodiscovery: true },
};

/** Fires a real submit so the component's `FormData` read is exercised, not bypassed. */
function submitForm(container: HTMLElement) {
  const form = container.querySelector<HTMLFormElement>('[data-testid="config-form"]');
  expect(form).not.toBeNull();
  form?.requestSubmit();
}

describe('ConfigForm rendering', () => {
  it('shows what is stored, and never a password', () => {
    const { container } = render(ConfigForm, {
      view: STORED,
      save: vi.fn(),
      restart: vi.fn(),
      health: vi.fn(),
    });

    expect(container.querySelector<HTMLInputElement>('[data-testid="config-host"]')?.value).toBe(
      '192.0.2.2',
    );
    expect(container.querySelector<HTMLInputElement>('[data-testid="config-port"]')?.value).toBe(
      '1884',
    );

    const password = container.querySelector<HTMLInputElement>('[data-testid="config-password"]');
    // The daemon never returns it, so the box must be empty — a filled one would be a lie about
    // what is stored, and submitting it would be the value that gets saved.
    expect(password?.value).toBe('');
    expect(password?.placeholder).toBe('Unchanged');
    expect(container.innerHTML).not.toContain('s3cret');
  });
});

describe('ConfigForm saving', () => {
  it('saves what was typed, and omits an empty password so the stored one survives', async () => {
    const save = vi.fn((update: ConfigUpdate) =>
      Promise.resolve({ ...STORED, mqtt_host: update.mqtt_host }),
    );
    const { container } = render(ConfigForm, {
      view: STORED,
      save,
      restart: vi.fn(),
      health: vi.fn(),
    });

    const host = container.querySelector<HTMLInputElement>('[data-testid="config-host"]');
    expect(host).not.toBeNull();
    if (host) host.value = '192.0.2.50';

    submitForm(container);
    await vi.waitFor(() => {
      expect(save).toHaveBeenCalledTimes(1);
    });

    const sent = save.mock.calls[0]?.[0];
    expect(sent?.mqtt_host).toBe('192.0.2.50');
    // An untouched password field must be sent as null ("keep what is stored"), never as '',
    // which the daemon reads as "clear it".
    expect(sent?.mqtt_password).toBeNull();
  });

  it('sends the sign-in switch as a boolean, both ways', async () => {
    const save = vi.fn((update: ConfigUpdate) =>
      Promise.resolve({ ...STORED, require_auth: update.require_auth ?? false }),
    );
    const { container } = render(ConfigForm, {
      view: STORED,
      save,
      restart: vi.fn(),
      health: vi.fn(),
    });

    /**
     * Toggled by clicking it, not by assigning `.checked`.
     *
     * It is a `Switch` (a `button role="switch"` over a hidden input), so setting the property
     * directly would let the visible control and the submitted value disagree. Clicking exercises
     * what an operator actually does.
     */
    const box = container.querySelector<HTMLElement>('[data-testid="config-require-auth"]');
    expect(
      box,
      'the switch must exist — it is the only way to turn authentication on',
    ).not.toBeNull();
    expect(
      box?.getAttribute('aria-checked'),
      'and it must show what is stored, which here is off',
    ).toBe('false');

    click(box);
    submitForm(container);
    await vi.waitFor(() => {
      expect(save).toHaveBeenCalledTimes(1);
    });
    // An unchecked box is absent from FormData, so this field is read as a presence. It must still be
    // sent as `false` rather than omitted when it is off: omitting means "keep" to the daemon.
    expect(save.mock.calls[0]?.[0].require_auth).toBe(true);

    click(box);
    submitForm(container);
    await vi.waitFor(() => {
      expect(save).toHaveBeenCalledTimes(2);
    });
    expect(save.mock.calls[1]?.[0].require_auth).toBe(false);
  });
});

describe('ConfigForm feedback', () => {
  it('clears the password only when the operator asks for it explicitly', async () => {
    // Typed through the parameter so `save.mock.calls[0][0]` is a ConfigUpdate rather than `never`.
    const save = vi.fn((_update: ConfigUpdate) =>
      Promise.resolve({ ...STORED, mqtt_password_set: false }),
    );
    const { container } = render(ConfigForm, {
      view: STORED,
      save,
      restart: vi.fn(),
      health: vi.fn(),
    });

    container.querySelector<HTMLButtonElement>('[data-testid="config-clear-password"]')?.click();

    await vi.waitFor(() => {
      expect(save).toHaveBeenCalledTimes(1);
    });
    expect(save.mock.calls[0]?.[0]?.mqtt_password).toBe('');
  });

  it('says a restart is required rather than implying the change is live', async () => {
    const save = vi.fn(() => Promise.resolve(STORED));
    const { container } = render(ConfigForm, {
      view: STORED,
      save,
      restart: vi.fn(),
      health: vi.fn(),
    });

    submitForm(container);
    await vi.waitFor(() => {
      expect(container.querySelector('[data-testid="config-saved"]')?.textContent ?? '').toContain(
        'Restart',
      );
    });
  });

  it('names the environment variables that override what is stored', () => {
    const { container } = render(ConfigForm, {
      view: { ...STORED, overridden_by_env: ['KESSEL_MQTT_HOST'] },
      save: vi.fn(),
      restart: vi.fn(),
      health: vi.fn(),
    });
    // Without this the operator sees a stored host while the daemon uses another, with nothing to
    // explain the difference.
    expect(
      container.querySelector('[data-testid="config-env-override"]')?.textContent ?? '',
    ).toContain('KESSEL_MQTT_HOST');
  });
});

describe('ConfigForm and the broker it did not have to be told about', () => {
  const DISCOVERED: ConfigView = {
    ...STORED,
    mqtt_host: null,
    mqtt_port: null,
    mqtt_username: null,
    mqtt_password_set: false,
    mqtt: { source: 'supervisor', host: 'core-mosquitto', port: 1883, autodiscovery: true },
  };

  /**
   * On a fresh add-on install the stored host is empty, so an empty host box reads as "MQTT is off",
   * and a host the operator then types replaces a working broker, because a typed host wins over
   * autodiscovery.
   */
  it('says which broker the daemon actually found, and where it came from', () => {
    const { container } = render(ConfigForm, {
      view: DISCOVERED,
      save: vi.fn(),
      restart: vi.fn(),
      health: vi.fn(),
    });

    const note = container.querySelector('[data-testid="config-broker-source"]')?.textContent ?? '';
    expect(note).toContain('core-mosquitto');
    expect(note).toMatch(/home assistant/iu);
    // And the box is still empty, because nothing is stored — the note is what explains it.
    expect(container.querySelector<HTMLInputElement>('[data-testid="config-host"]')?.value).toBe(
      '',
    );
  });

  it('does not claim a discovered broker when there is none', () => {
    const { container } = render(ConfigForm, {
      view: { ...STORED, mqtt: { ...STORED.mqtt, source: 'stored' } },
      save: vi.fn(),
      restart: vi.fn(),
      health: vi.fn(),
    });

    const note = container.querySelector('[data-testid="config-broker-source"]')?.textContent ?? '';
    expect(note).not.toMatch(/home assistant/iu);
  });

  /** Nothing here is applied live, so the button that applies it belongs beside the message saying so. */
  it('offers the restart beside the saved message', async () => {
    const save = vi.fn((update: ConfigUpdate) =>
      Promise.resolve({ ...STORED, mqtt_host: update.mqtt_host }),
    );
    const { container } = render(ConfigForm, {
      view: STORED,
      save,
      restart: vi.fn(),
      health: vi.fn(),
    });

    expect(container.querySelector('[data-testid="config-saved"]')).toBeNull();

    submitForm(container);
    await vi.waitFor(() => {
      expect(container.querySelector('[data-testid="config-saved"]')).not.toBeNull();
    });

    expect(
      container.querySelectorAll('[data-testid="restart-button"]').length,
      'the saved message must carry a restart, and the Server section keeps its own',
    ).toBe(2);
  });
});
