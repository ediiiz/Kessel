/**
 * The app's single entry point to the API types, which `ts-rs` generates from the Rust structs in
 * `apps/server`.
 *
 * Import from here, never from `@kessel/api-types` or `src/generated/` directly: this keeps the
 * workspace dependency type-checked by `bun run --filter web check` and lets the generator change
 * its output layout without touching components. Type-only, so it adds nothing to the bundle.
 */

export type * from '@kessel/api-types';
