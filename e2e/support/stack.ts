import { spawn, type ChildProcess } from 'node:child_process';
import { createWriteStream, existsSync, mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { Socket, createServer, type Server } from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { Aedes } from 'aedes';

export const REPO_ROOT = path.resolve(import.meta.dirname, '../..');
export const FIXTURE_PROFILE = path.join(
  REPO_ROOT,
  'crates/hc-profile/tests/fixtures/dishwasher-tls.zip',
);

// `just e2e` copies the binaries to `e2e/.bin`, out of reach of other builds sharing the target
// directory. KESSEL_BIN and KESSEL_SIM_BIN override either location.
const COPIED = path.join(REPO_ROOT, 'e2e/.bin');
const BINARY_DIR = existsSync(COPIED) ? COPIED : path.join(REPO_ROOT, '.cache/target/debug');
export const KESSEL_BIN = process.env.KESSEL_BIN ?? path.join(BINARY_DIR, 'kessel');
export const KESSEL_SIM_BIN = process.env.KESSEL_SIM_BIN ?? path.join(BINARY_DIR, 'kessel-sim');

export const STARTUP_TIMEOUT_MS = 30_000;

export function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const probe = createServer();
    probe.once('error', reject);
    probe.listen(0, '127.0.0.1', () => {
      const address = probe.address();
      const port = typeof address === 'object' && address !== null ? address.port : 0;
      probe.close(() => resolve(port));
    });
  });
}

export function scratchDir(prefix: string): string {
  const base = process.env.KESSEL_E2E_TMP ?? tmpdir();
  mkdirSync(base, { recursive: true });
  return mkdtempSync(path.join(base, prefix));
}

export async function waitFor(
  what: string,
  probe: () => Promise<boolean>,
  timeoutMs = STARTUP_TIMEOUT_MS,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    // oxlint-disable-next-line no-await-in-loop -- polling is sequential by design
    if (await probe()) {
      return;
    }
    // oxlint-disable-next-line no-await-in-loop -- polling is sequential by design
    await new Promise((resolve) => {
      setTimeout(resolve, 100);
    });
  }
  throw new Error(`timed out waiting for ${what}`);
}

function portAcceptsConnections(port: number): Promise<boolean> {
  return new Promise((resolve) => {
    const socket = new Socket();
    const settle = (open: boolean): void => {
      socket.destroy();
      resolve(open);
    };
    socket.once('connect', () => settle(true));
    socket.once('error', () => settle(false));
    socket.connect(port, '127.0.0.1');
  });
}

function exited(child: ChildProcess, name: string): void {
  if (child.exitCode !== null || child.signalCode !== null) {
    throw new Error(`${name} exited early (${child.exitCode ?? child.signalCode}); see its log`);
  }
}

export interface Broker {
  port: number;
  close: () => Promise<void>;
}

export async function startBroker(): Promise<Broker> {
  const aedes = await Aedes.createBroker();
  const server: Server = createServer(aedes.handle);
  const port = await new Promise<number>((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      resolve(typeof address === 'object' && address !== null ? address.port : 0);
    });
  });
  return {
    port,
    close: () =>
      new Promise<void>((resolve) => {
        aedes.close(() => {
          server.close(() => resolve());
        });
      }),
  };
}

export interface Managed {
  stop: () => Promise<void>;
}

const running = new Set<ChildProcess>();

// A crashed or interrupted runner must not leave a daemon or simulator behind.
process.once('exit', () => {
  for (const child of running) {
    child.kill('SIGKILL');
  }
});

function launch(
  command: string,
  args: string[],
  env: NodeJS.ProcessEnv,
  log: string,
): ChildProcess {
  const out = createWriteStream(log, { flags: 'a' });
  const child = spawn(command, args, { env, stdio: ['ignore', 'pipe', 'pipe'] });
  running.add(child);
  child.once('exit', () => running.delete(child));
  child.stdout?.pipe(out);
  child.stderr?.pipe(out);
  return child;
}

function stopper(child: ChildProcess): () => Promise<void> {
  return async () => {
    if (child.exitCode !== null || child.signalCode !== null) {
      return;
    }
    const gone = new Promise<void>((resolve) => {
      child.once('exit', () => resolve());
    });
    const kill = setTimeout(() => child.kill('SIGKILL'), 5000);
    child.kill('SIGTERM');
    await gone;
    clearTimeout(kill);
  };
}

export async function startSim(port: number, logDir: string): Promise<Managed> {
  const child = launch(
    KESSEL_SIM_BIN,
    ['--profile', FIXTURE_PROFILE, '--bind', '127.0.0.1', '--port', String(port), '--seed', '7'],
    { PATH: process.env.PATH, KESSEL_SIM_LOG: 'info', NO_COLOR: '1' },
    path.join(logDir, 'sim.log'),
  );
  await waitFor(`kessel-sim on port ${port}`, () => {
    exited(child, 'kessel-sim');
    return portAcceptsConnections(port);
  });
  return { stop: stopper(child) };
}

export interface Daemon extends Managed {
  baseUrl: string;
  stateDir: string;
}

export async function startDaemon(port: number, mqttPort: number, logDir: string): Promise<Daemon> {
  const stateDir = scratchDir('kessel-state-');
  // Only what the daemon needs, so nothing from the caller's shell (a real broker, a Supervisor
  // token, real data paths) leaks into the run.
  const env: NodeJS.ProcessEnv = {
    PATH: process.env.PATH,
    HOME: stateDir,
    KESSEL_PROFILE_DIR: path.join(stateDir, 'profiles'),
    KESSEL_CONFIG_FILE: path.join(stateDir, 'config.json'),
    KESSEL_TOKEN_FILE: path.join(stateDir, 'kessel.token'),
    KESSEL_REQUIRE_AUTH: 'false',
    KESSEL_PORT: String(port),
    KESSEL_MQTT_HOST: '127.0.0.1',
    KESSEL_MQTT_PORT: String(mqttPort),
    KESSEL_LOG: 'info',
    NO_COLOR: '1',
  };
  const child = launch(KESSEL_BIN, [], env, path.join(logDir, 'kessel.log'));
  const baseUrl = `http://127.0.0.1:${port}`;
  await waitFor('the daemon health endpoint', async () => {
    exited(child, 'kessel');
    const response = await fetch(`${baseUrl}/api/health`).catch(() => null);
    return response?.ok ?? false;
  });
  const stop = stopper(child);
  return {
    baseUrl,
    stateDir,
    stop: async () => {
      await stop();
      rmSync(stateDir, { recursive: true, force: true });
    },
  };
}
