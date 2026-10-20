import { expect, test } from '../support/fixtures.ts';

const TIME_LIGHT = 'Dishcare.Dishwasher.Setting.TimeLight';
const STATE_TOPIC = 'kessel/dishwasher/Dishcare/Dishwasher/Setting/TimeLight/state';

test('a quick action on the dishwasher applies at once and reaches MQTT', async ({
  page,
  device,
  broker,
}) => {
  await broker.subscribe(STATE_TOPIC);
  await expect.poll(() => broker.on(STATE_TOPIC).length).toBeGreaterThan(0);

  await page.goto(`/#/devices/${device}`);
  const tile = page.locator(`[data-testid="quick-action-toggle"][title="${TIME_LIGHT}"]`);
  await expect(tile.getByTestId('quick-action-value')).toHaveText('Off');
  await expect(tile).toHaveAttribute('aria-pressed', 'false');

  const before = broker.messages.length;
  await tile.click();

  // A dishwasher produces no heat, so there is nothing to confirm.
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByText('Applied.')).toBeVisible();
  await expect(tile.getByTestId('quick-action-value')).toHaveText('On');
  await expect(tile).toHaveAttribute('aria-pressed', 'true');

  await expect
    .poll(() => broker.on(STATE_TOPIC, before).map((message) => message.payload))
    .toContain('On');
});
