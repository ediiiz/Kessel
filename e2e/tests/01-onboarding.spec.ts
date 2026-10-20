import { FIXTURE_PROFILE } from '../support/stack.ts';
import { SLUG, connectionState, expect, test } from '../support/fixtures.ts';

test('onboarding the simulator through the wizard connects it and shows live values', async ({
  page,
  stack,
}) => {
  expect(
    await connectionState(stack.baseUrl, SLUG),
    'this test needs a daemon with no devices',
  ).toBeNull();

  await page.goto('/#/');
  await expect(page.getByTestId('no-devices')).toBeVisible();

  await page.getByTestId('add-appliance').click();
  const sheet = page.getByTestId('add-appliance-sheet');
  await expect(sheet).toBeVisible();
  await sheet.getByTestId('profile-zip').setInputFiles(FIXTURE_PROFILE);
  await sheet.getByTestId('upload').click();

  const dialog = page.getByTestId('confirm-dialog');
  await expect(dialog).toBeVisible();
  await expect(dialog.getByTestId('preview-role')).toHaveText('dishwasher');
  await expect(dialog.getByTestId('preview-transport')).toHaveText('TLS-PSK over wss');
  await expect(dialog.getByTestId('confirm-slug')).toHaveValue(SLUG);
  await dialog.getByTestId('confirm-host').fill('127.0.0.1');
  await dialog.getByTestId('confirm-port').fill(String(stack.simPort));
  await dialog.getByTestId('confirm-submit').click();

  await expect(dialog).toBeHidden();
  await expect(sheet.getByTestId('result-slug')).toHaveText(SLUG);
  await expect(sheet.getByTestId('result-connection')).toContainText('connected');

  // The list behind the sheet has already picked the new appliance up.
  await page.keyboard.press('Escape');
  await expect(sheet).toBeHidden();
  const row = page.locator(`[data-testid="device-row"][data-slug="${SLUG}"]`);
  await expect(row.getByTestId('device-connection-state')).toHaveText('connected');
  await row.getByTestId('device-link').click();

  await expect(page).toHaveURL(new RegExp(`#/devices/${SLUG}$`, 'u'));
  await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
  await expect(page.getByText('connected', { exact: true })).toBeVisible();

  // The simulator counts the remaining time down on a timer; the card must follow it without a
  // reload.
  const remaining = page.getByTestId('programme-remaining');
  await expect(remaining).not.toHaveText('');
  const first = await remaining.textContent();
  await expect(remaining).not.toHaveText(first ?? '');
});
