import { describe, expect, it, vi } from 'vitest';
import { click, type } from '$lib/dom-testing';
import { commitSpy, preview, readProfile, valueOf, wizard } from './onboarding-fixtures';

/**
 * Phase 1 and the confirmation dialog's own rules. What phase 2 *sends*, and what comes back
 * from it, is `OnboardingWizard.commit.test.ts`.
 *
 * `$app/paths` is not mocked here: nothing in this file goes through the API client, so `base` never
 * matters. The commit file mocks it, because it does.
 */

describe('reading_a_profile_dials_nothing', () => {
  it('shows what was parsed and prefills the name and port', async () => {
    const read = vi.fn(() => Promise.resolve(preview()));
    const commit = commitSpy();
    const view = wizard({ preview: read, commit });

    await readProfile(view);

    expect(read).toHaveBeenCalledTimes(1);
    // Phase 1 commits nothing, so nothing was dialled or stored.
    expect(commit).not.toHaveBeenCalled();

    expect(view.queryPortal('preview-role')?.textContent).toContain('dishwasher');
    expect(view.queryPortal('preview-entity-count')?.textContent).toContain('270');
    expect(view.queryPortal('preview-skipped')?.textContent).toContain('3');
    expect(view.queryPortal('preview-transport')?.textContent).toContain('TLS-PSK');
    expect(valueOf(view.queryPortal('confirm-slug'))).toBe('dishwasher');
    expect(valueOf(view.queryPortal('confirm-port'))).toBe('443');
    view.destroy();
  });

  it('leaves the host field empty, because there is nothing safe to prefill it with', async () => {
    const view = wizard();
    await readProfile(view);

    // The profile-derived host contains the haId, so the daemon does not return it (`ProfilePreview`),
    // and it would not resolve anyway. The wizard must not invent a substitute.
    expect(valueOf(view.queryPortal('confirm-host'))).toBe('');
    view.destroy();
  });

  it('reads nothing until the operator clicks, and never on mount', () => {
    const read = vi.fn(() => Promise.resolve(preview()));
    const view = wizard({ preview: read });

    expect(read).not.toHaveBeenCalled();
    view.destroy();
  });
});

describe('an_unreadable_archive_stops_before_the_dialog', () => {
  it('shows the reason and opens no dialog', async () => {
    const view = wizard({
      preview: () => Promise.reject(new Error('the profile could not be parsed: no manifest')),
    });
    const { attachFile, zipFile } = await import('$lib/dom-testing');

    attachFile(view.query('profile-zip'), zipFile());
    click(view.query('upload'));
    await vi.waitFor(() => {
      expect(view.query('upload-error')).not.toBeNull();
    });
    view.flush();

    expect(view.query('upload-error')?.textContent).toContain('no manifest');
    // No dialog: there is nothing to confirm.
    expect(view.queryPortal('confirm-slug')).toBeNull();
    view.destroy();
  });
});

describe('a_known_appliance_is_offered_as_an_update', () => {
  it('names the device it would update and defaults to keeping it', async () => {
    const view = wizard({
      preview: () => Promise.resolve(preview({ existing_slug: 'kitchen-dishwasher' })),
    });
    await readProfile(view);

    expect(view.queryPortal('confirm-submit')?.textContent).toContain('Update');
    expect(view.queryPortal('confirm-dialog')?.textContent).toContain('kitchen-dishwasher');
    view.destroy();
  });

  it('offers to add, not update, when the appliance is new', async () => {
    const view = wizard();
    await readProfile(view);

    // Both directions, so the wording cannot pass by being unconditional.
    expect(view.queryPortal('confirm-submit')?.textContent).toContain('Add');
    expect(view.queryPortal('confirm-submit')?.textContent).not.toContain('Update');
    view.destroy();
  });
});

describe('the_dialog_enforces_exactly_two_client_side_rules', () => {
  it('refuses an empty name and sends nothing', async () => {
    const commit = commitSpy();
    const view = wizard({ commit });
    await readProfile(view);

    const submit = view.queryPortal('confirm-submit');
    type(view.queryPortal('confirm-slug'), '   ');
    expect(submit instanceof HTMLButtonElement && submit.disabled).toBe(true);
    click(submit);
    expect(commit).not.toHaveBeenCalled();
    view.destroy();
  });

  it('refuses an out-of-range port and sends nothing', async () => {
    const commit = commitSpy();
    const view = wizard({ commit });
    await readProfile(view);

    const submit = view.queryPortal('confirm-submit');
    type(view.queryPortal('confirm-port'), '70000');
    expect(submit instanceof HTMLButtonElement && submit.disabled).toBe(true);
    click(submit);
    expect(commit).not.toHaveBeenCalled();
    view.destroy();
  });

  it('accepts everything else, because the rest is the daemon judgement', async () => {
    const view = wizard();
    await readProfile(view);

    const submit = view.queryPortal('confirm-submit');
    // A name with characters the daemon will refuse. The dialog does *not* pre-judge it: the daemon
    // answers with prose naming the allowed characters, which is more useful than a disabled button.
    type(view.queryPortal('confirm-slug'), 'not a valid slug');
    expect(submit instanceof HTMLButtonElement && submit.disabled).toBe(false);
    // An empty port is valid: it means the transport's default.
    type(view.queryPortal('confirm-port'), '');
    expect(submit instanceof HTMLButtonElement && submit.disabled).toBe(false);
    view.destroy();
  });
});

describe('the_zip_help_points_at_the_downloader', () => {
  it('opens on request and links the downloader in a new tab', () => {
    const view = wizard();
    const help = view.query('zip-help');
    const trigger = help?.querySelector('button');
    expect(trigger?.getAttribute('aria-expanded')).toBe('false');

    click(trigger ?? null);

    expect(trigger?.getAttribute('aria-expanded')).toBe('true');
    const link = help?.querySelector('a');
    expect(link?.getAttribute('href')).toBe(
      'https://github.com/bruestel/homeconnect-profile-downloader/releases/latest',
    );
    expect(link?.getAttribute('target')).toBe('_blank');
    view.destroy();
  });
});
