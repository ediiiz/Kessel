/**
 * Programme actions as writes to the programme pointers, so the confirmation step and the toasts
 * describe exactly what changes:
 *
 * - `SelectedProgram = <key>` selects a programme (on a cooktop or an oven that is also the start);
 * - `ActiveProgram = <key>` starts that programme;
 * - `ActiveProgram = 0` stops whatever runs, a delayed start included.
 *
 * {@link programmeWriter} routes each to the daemon route that carries it. The daemon sends what the
 * appliance's own description calls for, and the appliance's answer is the outcome.
 */
import type { StartProgramRequest, WriteOutcome, WriteRequest } from '$lib/api-types';
import type { Writer } from '$lib/write.svelte';

export const SELECTED_PROGRAM = 'BSH.Common.Root.SelectedProgram';
export const ACTIVE_PROGRAM = 'BSH.Common.Root.ActiveProgram';
/** The active-programme value that means "nothing runs". */
export const NO_PROGRAMME = '0';

/** The operation states in which a programme runs or waits to, so Stop has something to stop. */
const RUNNING_STATES = new Set(['Run', 'DelayedStart', 'Pause', 'ActionRequired']);

/**
 * Whether a programme runs, by the appliance's operation state where it reports one.
 *
 * The active programme alone is not enough: a dishwasher keeps reporting its last programme as
 * active after an abort while it is `Ready`. Without an operation state, a non-zero active programme
 * is all there is to go on. The state may arrive as its short name or as its full enum key.
 */
export function programmeRuns(
  operation: string | null | undefined,
  active: string | null | undefined,
): boolean {
  if (operation !== null && operation !== undefined && operation !== '') {
    return RUNNING_STATES.has(operation.slice(operation.lastIndexOf('.') + 1));
  }
  return active !== null && active !== undefined && active !== '' && active !== NO_PROGRAMME;
}

/** The routes {@link programmeWriter} needs. */
export interface ProgrammeRoutes {
  write(slug: string, request: WriteRequest): Promise<WriteOutcome>;
  startProgram(slug: string, request: StartProgramRequest): Promise<WriteOutcome>;
  stopProgram(slug: string): Promise<WriteOutcome>;
}

/** A {@link Writer} that sends a start or a stop to its own route and everything else as a write. */
export function programmeWriter(api: ProgrammeRoutes): Writer {
  return (slug, request) => {
    if (request.feature_key !== ACTIVE_PROGRAM) {
      return api.write(slug, request);
    }
    return request.value === NO_PROGRAMME
      ? api.stopProgram(slug)
      : api.startProgram(slug, { program: request.value });
  };
}
