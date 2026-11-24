import { Container, createToken, DiError, DiGraphError } from '@coldmind/inject';

/** Matches stable error codes and keeps graph issue kinds distinct from codes. */
export async function runExample(): Promise<string> {
  const Missing = createToken<string>('Missing');
  const Consumer = createToken<string>('Consumer');
  const root = new Container();
  try {
    let missingCode = '';
    try { await root.resolve(Missing); }
    catch (error) {
      if (!(error instanceof DiError) || error.code !== 'DI_NOT_REGISTERED') throw error;
      missingCode = error.code;
    }
    if (!missingCode) throw new Error('Missing dependency unexpectedly resolved');
    root.singleton(Consumer, c => c.resolve(Missing), { deps: [Missing] });
    try { root.validate(); }
    catch (error) {
      if (!(error instanceof DiGraphError) || !error.issues.some(issue => issue.kind === 'missing')) throw error;
      return `${missingCode}|${error.code}:missing`;
    }
    throw new Error('Validation accepted a missing dependency');
  } finally {
    await root.dispose();
  }
}
