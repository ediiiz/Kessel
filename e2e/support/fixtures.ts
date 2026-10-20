import { readFile } from 'node:fs/promises';
import { test as base, expect } from '@playwright/test';
import mqtt, { type MqttClient } from 'mqtt';
import { FIXTURE_PROFILE } from './stack.ts';

export const SLUG = 'dishwasher';

function required(name: string): string {
  const value = process.env[name];
  if (value === undefined || value === '') {
    throw new Error(`${name} is unset; run the suite through playwright so global setup runs`);
  }
  return value;
}

export interface Stack {
  baseUrl: string;
  mqttPort: number;
  simPort: number;
  stopSim: () => Promise<void>;
  startSim: () => Promise<void>;
}

async function control(action: 'start' | 'stop'): Promise<void> {
  const response = await fetch(`${required('KESSEL_E2E_CONTROL_URL')}/sim/${action}`, {
    method: 'POST',
  });
  if (!response.ok) {
    throw new Error(`sim ${action} failed: ${response.status} ${await response.text()}`);
  }
}

interface DeviceSummary {
  slug: string;
  connection_state: string;
}

export async function connectionState(baseUrl: string, slug: string): Promise<string | null> {
  const response = await fetch(`${baseUrl}/api/devices`);
  const devices = (await response.json()) as DeviceSummary[];
  return devices.find((device) => device.slug === slug)?.connection_state ?? null;
}

export interface Message {
  topic: string;
  payload: string;
  retain: boolean;
}

/** A broker connection that records everything it receives, in order. */
export interface Recorder {
  client: MqttClient;
  messages: Message[];
  subscribe: (filter: string) => Promise<void>;
  publish: (topic: string, payload: string) => Promise<void>;
  /** Messages on `topic`, live or retained, from `from` onwards. */
  on: (topic: string, from?: number) => Message[];
}

async function connectRecorder(port: number): Promise<Recorder> {
  const client = await mqtt.connectAsync(`mqtt://127.0.0.1:${port}`, {
    clientId: `kessel-e2e-${process.pid}-${Date.now()}`,
    reconnectPeriod: 0,
  });
  const messages: Message[] = [];
  client.on('message', (topic, payload, packet) => {
    messages.push({ topic, payload: payload.toString(), retain: packet.retain });
  });
  return {
    client,
    messages,
    subscribe: async (filter) => {
      await client.subscribeAsync(filter, { qos: 1 });
    },
    publish: async (topic, payload) => {
      await client.publishAsync(topic, payload, { qos: 1 });
    },
    on: (topic, from = 0) => messages.slice(from).filter((message) => message.topic === topic),
  };
}

export const test = base.extend<{ device: string; broker: Recorder }, { stack: Stack }>({
  stack: [
    // oxlint-disable-next-line no-empty-pattern -- Playwright requires the destructuring form
    async ({}, use) => {
      await use({
        baseUrl: required('KESSEL_E2E_BASE_URL'),
        mqttPort: Number(required('KESSEL_E2E_MQTT_PORT')),
        simPort: Number(required('KESSEL_E2E_SIM_PORT')),
        stopSim: () => control('stop'),
        startSim: () => control('start'),
      });
    },
    { scope: 'worker' },
  ],

  baseURL: async ({ stack }, use) => {
    await use(stack.baseUrl);
  },

  // The simulator, onboarded through the API and connected. Onboarding through the UI has its own
  // test; everything else only needs the device to exist.
  device: async ({ stack }, use) => {
    if ((await connectionState(stack.baseUrl, SLUG)) === null) {
      const zip = await readFile(FIXTURE_PROFILE);
      const query = new URLSearchParams({
        slug: SLUG,
        host: '127.0.0.1',
        port: String(stack.simPort),
      });
      const response = await fetch(`${stack.baseUrl}/api/onboarding/commit?${query}`, {
        method: 'POST',
        body: zip,
      });
      expect(response.ok, await response.text()).toBe(true);
    }
    await expect
      .poll(() => connectionState(stack.baseUrl, SLUG), { message: 'device connects' })
      .toBe('connected');
    await use(SLUG);
  },

  broker: async ({ stack }, use) => {
    const recorder = await connectRecorder(stack.mqttPort);
    await use(recorder);
    await recorder.client.endAsync();
  },
});

export { expect };
