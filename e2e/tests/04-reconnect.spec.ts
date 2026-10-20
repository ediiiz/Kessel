import { expect, test } from '../support/fixtures.ts';

const AVAILABILITY = 'kessel/dishwasher/availability';

test('the UI follows the appliance going away and coming back', async ({
  page,
  device,
  broker,
  stack,
}) => {
  await broker.subscribe(AVAILABILITY);
  await page.goto(`/#/devices/${device}`);
  const state = page.getByText(/^(connecting|handshake|connected|reconnecting|offline)$/u);
  await expect(state).toHaveText('connected');

  const before = broker.messages.length;
  await stack.stopSim();
  try {
    await expect(state).toHaveText(/^(reconnecting|offline)$/u);
    await expect(page.getByTestId('device-connection-detail')).toBeVisible();
    await expect
      .poll(() => broker.on(AVAILABILITY, before).map((message) => message.payload))
      .toContain('offline');
  } finally {
    await stack.startSim();
  }

  const restarted = broker.messages.length;
  await expect(state).toHaveText('connected', { timeout: 30_000 });
  await expect
    .poll(() => broker.on(AVAILABILITY, restarted).map((message) => message.payload))
    .toContain('online');
});
