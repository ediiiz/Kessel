/**
 * Re-runs the `ts-rs` generator whenever an API type changes; `just types` has no watch mode.
 *
 * A Bun script rather than `cargo-watch`, `watchexec` or `bacon`, because Bun is already required
 * and none of those is installed by this repository.
 *
 * `#[derive(TS)]` appears in exactly two files, `apps/server/src/types.rs` and
 * `apps/server/src/config.rs`, so only those are watched. Watching the whole crate would rebuild on
 * every unrelated edit, and watching `packages/api-types/` would watch the generator's own output.
 * If a third file gains a derive this script will not notice, so it prints what it covers.
 *
 * The generator (`cargo run -p server --example export-types`) shares the `target/` lock with a
 * running `cargo run -p server`, so a regeneration and a daemon rebuild serialise. That is
 * deliberate: a private `CARGO_TARGET_DIR` would cost a second cold build and a duplicate multi-GB
 * `target/`, and editing an API type triggers a daemon rebuild anyway.
 */
import { existsSync, watch } from 'node:fs';

/** The only two files carrying `#[derive(TS)]`. Kept as basenames — the watch is on the directory. */
const WATCHED = ['types.rs', 'config.rs'];

/** The directory holding them. Watched non-recursively: editors replace files by rename, which a
 *  per-file watch loses track of, and a directory watch survives. */
const WATCH_DIR = 'apps/server/src';

/** Collapses the burst of events a single save produces (write, rename, chmod) into one run. */
const DEBOUNCE_MS = 150;

let timer: ReturnType<typeof setTimeout> | null = null;
let running = false;
/** A change that arrived while the generator was mid-run. Re-runs once at the end rather than queueing
 *  a run per event — the generator is not incremental, so N queued runs and one final run agree. */
let dirty = false;

async function regenerate(): Promise<void> {
  if (running) {
    dirty = true;
    return;
  }
  running = true;
  const started = Date.now();
  const proc = Bun.spawn(['cargo', 'run', '-p', 'server', '--example', 'export-types'], {
    stdout: 'inherit',
    stderr: 'inherit',
  });
  const code = await proc.exited;
  running = false;
  const took = `${((Date.now() - started) / 1000).toFixed(1)}s`;
  // A non-zero exit is a compile error in the crate the daemon pane is also building, so it is already
  // on screen there. Say which pane failed and keep watching — exiting would make the loop need a
  // manual restart on every typo.
  console.log(
    code === 0 ? `types: regenerated (${took})` : `types: generator failed, exit ${code} (${took})`,
  );
  if (dirty) {
    dirty = false;
    void regenerate();
  }
}

for (const file of WATCHED) {
  const path = `${WATCH_DIR}/${file}`;
  if (!existsSync(path)) {
    console.error(
      `types: ${path} does not exist — the watch list is stale, fix scripts/watch-types.ts`,
    );
    process.exit(1);
  }
}

console.log(`types: watching ${WATCHED.map((f) => `${WATCH_DIR}/${f}`).join(' ')}`);

watch(WATCH_DIR, (_event, filename) => {
  if (filename === null || !WATCHED.includes(filename)) return;
  if (timer !== null) clearTimeout(timer);
  timer = setTimeout(() => void regenerate(), DEBOUNCE_MS);
});

// One run at startup, so the generated types are current even if they drifted while this was not
// running.
void regenerate();
