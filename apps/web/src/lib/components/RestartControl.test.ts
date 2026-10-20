import { describe, expect, it, vi } from 'vitest';
import type { RestartOutcome } from '$lib/api-types';
import { click, render } from '$lib/dom-testing';
import RestartControl from './RestartControl.svelte';

const SUPERVISOR: RestartOutcome = {
  mechanism: 'supervisor',
  message: 'Home Assistant is restarting the add-on.',
};

const PROCESS_EXIT: RestartOutcome = {
  mechanism: 'process_exit',
  message: 'Kessel is exiting. It comes back only if whatever started it restarts it.',
};

function button(container: HTMLElement) {
  return container.querySelector<HTMLButtonElement>('[data-testid="restart-button"]');
}

function status(container: HTMLElement) {
  return container.querySelector('[data-testid="restart-status"]')?.textContent ?? '';
}

/** Clicks through the confirmation, which is the only way to reach the call. */
async function confirmAndRestart(container: HTMLElement) {
  click(button(container));
  await vi.waitFor(() => {
    expect(status(container)).toMatch(/reconnects/iu);
  });
  click(button(container));
}

describe('RestartControl', () => {
  /**
   * A restart drops every appliance connection this daemon holds open, and under ingress the page it was
   * clicked from goes with it. That is worth one confirmation — and not worth a modal.
   */
  it('does not restart on the first click', async () => {
    const restart = vi.fn(() => Promise.resolve(SUPERVISOR));
    const { container } = render(RestartControl, { restart, health: () => Promise.resolve() });

    click(button(container));
    await vi.waitFor(() => {
      expect(status(container)).toMatch(/reconnects/iu);
    });

    expect(restart).not.toHaveBeenCalled();
  });

  it('restarts on the second click', async () => {
    const restart = vi.fn(() => Promise.resolve(SUPERVISOR));
    const { container } = render(RestartControl, { restart, health: () => Promise.resolve() });

    await confirmAndRestart(container);

    await vi.waitFor(() => {
      expect(restart).toHaveBeenCalledTimes(1);
    });
  });

  /**
   * The two mechanisms promise different things and the copy must not flatten them: only the Supervisor
   * one says Kessel comes back. The daemon's own sentence is what is shown, rather than a second copy of
   * that judgement in the browser.
   */
  it("repeats the daemon's own words about what is about to happen", async () => {
    const restart = vi.fn(() => Promise.resolve(PROCESS_EXIT));
    const { container } = render(RestartControl, {
      restart,
      // A watch that stays open: the message under test belongs to the `restarting` phase, and a watch
      // that resolved immediately would race the assertion rather than test it.
      health: () => Promise.reject(new Error('gone')),
      pollMs: 5,
      timeoutMs: 5000,
    });

    await confirmAndRestart(container);

    await vi.waitFor(() => {
      expect(status(container)).toContain('comes back only if');
    });
  });

  /**
   * The daemon going away and coming back is the only *evidence* a restart happened — the 202 is a promise,
   * not a fact — so the control watches for it rather than declaring success on the response.
   */
  it('says Kessel is back once it answers again', async () => {
    let calls = 0;
    const health = () => {
      calls += 1;
      return calls > 2 ? Promise.resolve() : Promise.reject(new Error('gone'));
    };
    const { container } = render(RestartControl, {
      restart: () => Promise.resolve(SUPERVISOR),
      health,
      pollMs: 5,
      timeoutMs: 2000,
    });

    await confirmAndRestart(container);

    await vi.waitFor(
      () => {
        expect(status(container)).toMatch(/back/iu);
      },
      { timeout: 2000 },
    );
  });

  /**
   * A daemon that never went away did not restart. The Supervisor refusing for want of
   * `hassio_api: true` looks exactly like success from the 202 alone.
   */
  it('says so when the daemon never went away', async () => {
    const { container } = render(RestartControl, {
      restart: () => Promise.resolve(SUPERVISOR),
      health: () => Promise.resolve(),
      pollMs: 5,
      timeoutMs: 30,
    });

    await confirmAndRestart(container);

    await vi.waitFor(() => {
      expect(status(container)).toMatch(/did not|still running/iu);
    });
  });

  it('reports a refusal rather than pretending the restart happened', async () => {
    const restart = vi.fn(() => Promise.reject(new Error('the API token is missing')));
    const { container } = render(RestartControl, { restart, health: () => Promise.resolve() });

    await confirmAndRestart(container);

    await vi.waitFor(() => {
      expect(status(container)).toContain('the API token is missing');
    });
  });
});
