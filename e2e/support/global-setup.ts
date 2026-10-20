import { mkdirSync } from 'node:fs';
import { createServer } from 'node:http';
import path from 'node:path';
import type { FullConfig } from '@playwright/test';
import { type Managed, freePort, scratchDir, startBroker, startDaemon, startSim } from './stack.ts';

// Everything here runs in the Playwright runner process, which outlives the workers. Tests reach the
// stack through `process.env`, which the workers inherit, and restart the simulator through a small
// HTTP control endpoint so the runner keeps ownership of every child process.
export default async function globalSetup(_config: FullConfig): Promise<() => Promise<void>> {
  const logDir = process.env.KESSEL_E2E_LOGS ?? scratchDir('kessel-e2e-logs-');
  mkdirSync(logDir, { recursive: true });

  const broker = await startBroker();
  const simPort = await freePort();
  let sim: Managed | null = await startSim(simPort, logDir);
  const daemon = await startDaemon(await freePort(), broker.port, logDir);

  const control = createServer((request, response) => {
    const act = async (): Promise<void> => {
      if (request.method === 'POST' && request.url === '/sim/stop') {
        await sim?.stop();
        sim = null;
      } else if (request.method === 'POST' && request.url === '/sim/start') {
        sim ??= await startSim(simPort, logDir);
      } else {
        response.writeHead(404).end();
        return;
      }
      response.writeHead(204).end();
    };
    act().catch((error: unknown) => {
      response.writeHead(500).end(String(error));
    });
  });
  const controlPort = await freePort();
  await new Promise<void>((resolve) => {
    control.listen(controlPort, '127.0.0.1', resolve);
  });

  process.env.KESSEL_E2E_BASE_URL = daemon.baseUrl;
  process.env.KESSEL_E2E_MQTT_PORT = String(broker.port);
  process.env.KESSEL_E2E_SIM_PORT = String(simPort);
  process.env.KESSEL_E2E_CONTROL_URL = `http://127.0.0.1:${controlPort}`;
  process.env.KESSEL_E2E_LOGS = logDir;
  process.stdout.write(`e2e: daemon ${daemon.baseUrl}, logs in ${path.resolve(logDir)}\n`);

  return async () => {
    await new Promise<void>((resolve) => {
      control.close(() => resolve());
    });
    await daemon.stop();
    await sim?.stop();
    await broker.close();
  };
}
