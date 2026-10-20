import { type Recorder, expect, test } from '../support/fixtures.ts';

const HOT_WATER = 'Dishcare.Dishwasher.Setting.HotWater';
const DISCOVERY = 'homeassistant/device/kessel_dishwasher/config';
const COMPONENT = 'Dishcare_Dishwasher_Setting_HotWater';
const TICKING_TOPIC = 'kessel/dishwasher/BSH/Common/Option/RemainingProgramTime/state';

interface SelectConfig {
  platform: string;
  command_topic: string;
  state_topic: string;
  options: string[];
}

interface DeviceMessage {
  origin: { name: string };
  components: Record<string, SelectConfig>;
}

function retainedConfigs(recorder: Recorder): Map<string, string> {
  return new Map(
    recorder.messages
      .filter((message) => message.retain && message.topic.startsWith('homeassistant/'))
      .map((message) => [message.topic, message.payload]),
  );
}

test('a Home Assistant command updates the UI live and leaves discovery alone', async ({
  page,
  device,
  broker,
}) => {
  await broker.subscribe(DISCOVERY);
  await expect.poll(() => broker.on(DISCOVERY).length).toBeGreaterThan(0);
  const announcement = JSON.parse(broker.on(DISCOVERY)[0]?.payload ?? '{}') as DeviceMessage;
  expect(announcement.origin.name).toBe('Kessel');
  const config = announcement.components[COMPONENT];
  if (config === undefined) {
    throw new Error(`${COMPONENT} is not announced`);
  }
  expect(config.platform).toBe('select');
  expect(config.options).toEqual(['ColdWater', 'HotWater']);
  const announced = retainedConfigs(broker);

  await page.goto(`/#/devices/${device}`);
  // A tile shows the current member and carries the one a tap would send.
  const tile = page.locator(`[data-testid="quick-action-toggle"][title="${HOT_WATER}"]`);
  await expect(tile.getByTestId('quick-action-value')).toHaveText('Cold water');
  await expect(tile).toHaveAttribute('data-value', 'HotWater');

  await broker.subscribe(config.state_topic);
  await broker.subscribe(TICKING_TOPIC);
  const before = broker.messages.length;
  await broker.publish(config.command_topic, 'HotWater');

  await expect(tile.getByTestId('quick-action-value')).toHaveText('Hot water');
  await expect(tile).toHaveAttribute('data-value', 'ColdWater');
  await expect
    .poll(() => broker.on(config.state_topic, before).map((message) => message.payload))
    .toContain('HotWater');

  // The daemon publishes values and discovery on one connection, in order, so a value published
  // after the command's echo means any discovery traffic the command caused has arrived too.
  const echoed = broker.messages.findIndex(
    (message, index) =>
      index >= before && message.topic === config.state_topic && message.payload === 'HotWater',
  );
  await expect.poll(() => broker.on(TICKING_TOPIC, echoed + 1).length).toBeGreaterThan(0);
  const discoveryTraffic = broker.messages
    .slice(before)
    .filter((message) => message.topic.startsWith('homeassistant/'));
  expect(discoveryTraffic).toEqual([]);
  expect(retainedConfigs(broker)).toEqual(announced);
});
