/**
 * Doubles a number; exists only to be transformed and cached by vitest.
 * @param value the number
 * @return twice the number
 */
export function double(value: number): number {
  return value * 2;
}
