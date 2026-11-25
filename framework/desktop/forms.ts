/**
 * Mock backend runtime (`@desktop/forms`): the exports form classes and the
 * generated entry import, with no behaviour, so the pipeline builds and its
 * tests run while no real runtime exists.
 */

/** The owner name of the application element. */
export const APPLICATION = '@application';

/** A placeholder control. */
export class Control {}

/** A placeholder form; project form classes extend it. */
export class Form {}

/**
 * Starts the application. Does nothing.
 * @param {unknown} _project the generated project description
 */
export function start(_project: unknown): void {}
