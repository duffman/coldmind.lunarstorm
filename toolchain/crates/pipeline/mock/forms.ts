/**
 * Dummy backend runtime (`@desktop/forms`): the exports form classes and the
 * generated entry import, with no behaviour. No real runtime exists yet.
 */

/** The owner name of the application element. */
export const APPLICATION = '@application';

/** A placeholder control. */
export class Control {}

/** A placeholder form; project form classes extend it. */
export class Form {}

/**
 * Starts the application: only keeps the project description.
 *
 * Decision: the description is stored on `globalThis` so the bundler keeps the
 * form classes; an empty `start` let it drop them as unused.
 * @param {unknown} project the generated project description
 */
export function start(project: unknown): void {
  (globalThis as { project?: unknown }).project = project;
}
