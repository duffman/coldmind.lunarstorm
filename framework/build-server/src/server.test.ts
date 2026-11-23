import { describe, expect, test } from 'bun:test';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createRequestHandler, startServer } from './server';
import type { extractProject } from './model';

type Model = ReturnType<typeof extractProject>;
const model = (errors = false) => ({
  schemaVersion: 1,
  root: '/project',
  config: {},
  views: [],
  services: [],
  diagnostics: errors ? [{ severity: 'error', code: 'TEST_ERROR', message: 'Invalid view connection.' }] : [],
}) as unknown as Model;
const post = (body: unknown, headers: HeadersInit = { 'Content-Type': 'application/json' }) => new Request('http://127.0.0.1/analyze', {
  method: 'POST', headers, body: JSON.stringify(body),
});

async function withDirectory(run: (path: string) => Promise<void>) {
  const directory = await mkdtemp(join(tmpdir(), 'ls-server-'));
  try { await run(directory); }
  finally { await rm(directory, { recursive: true, force: true }); }
}

describe('build server HTTP interface', () => {
  test('health and endpoint method contracts', async () => {
    const handle = createRequestHandler();
    const health = await handle(new Request('http://127.0.0.1/health'));
    expect(health.status).toBe(200);
    expect(await health.json()).toEqual({ status: 'ok' });
    const method = await handle(new Request('http://127.0.0.1/analyze'));
    expect(method.status).toBe(405);
    expect(method.headers.get('allow')).toBe('POST');
    expect((await handle(new Request('http://127.0.0.1/missing'))).status).toBe(404);
  });

  test('rejects origin requests and unsupported content types before analysis', async () => {
    let called = false;
    const handle = createRequestHandler({ analyze: () => { called = true; return model(); } });
    const origin = await handle(post({ projectPath: '/tmp' }, { 'Content-Type': 'application/json', Origin: 'https://example.com' }));
    expect(origin.status).toBe(403);
    expect(origin.headers.get('access-control-allow-origin')).toBeNull();
    expect((await handle(post({ projectPath: '/tmp' }, { 'Content-Type': 'text/plain' }))).status).toBe(415);
    expect(called).toBe(false);
  });

  test('validates JSON, path shape, directory existence and request size', async () => {
    const handle = createRequestHandler({ analyze: () => { throw new Error('Must not analyze invalid requests'); } });
    const invalid = new Request('http://127.0.0.1/analyze', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{' });
    expect((await handle(invalid)).status).toBe(400);
    for (const body of [null, [], {}, { projectPath: 4 }, { projectPath: 'relative' }, { projectPath: '/tmp', extra: true }]) {
      expect((await handle(post(body))).status).toBe(400);
    }
    expect((await handle(post({ projectPath: '/nonexistent-ls-project-directory' }))).status).toBe(404);
    expect((await handle(post({ projectPath: '/' + 'a'.repeat(17 * 1024) }))).status).toBe(413);
    await withDirectory(async directory => {
      const file = join(directory, 'file');
      await writeFile(file, '');
      expect((await handle(post({ projectPath: file }))).status).toBe(400);
    });
  });

  test('returns extracted model and preserves diagnostic failure status', async () => {
    await withDirectory(async directory => {
      const validModel = model();
      let analyzedPath: string | undefined;
      const handle = createRequestHandler({ analyze: path => { analyzedPath = path; return validModel; } });
      const response = await handle(post({ projectPath: directory }));
      expect(response.status).toBe(200);
      expect(await response.json()).toEqual(validModel);
      expect(analyzedPath).toBe(directory);
      const invalidModel = model(true);
      const invalid = await createRequestHandler({ analyze: () => invalidModel })(post({ projectPath: directory }));
      expect(invalid.status).toBe(422);
      expect(await invalid.json()).toEqual(invalidModel);
    });
  });

  test('returns a bounded internal failure without leaking exception details', async () => {
    await withDirectory(async directory => {
      const handle = createRequestHandler({ analyze: () => { throw new Error('secret stack details'); } });
      const response = await handle(post({ projectPath: directory }));
      expect(response.status).toBe(500);
      expect(await response.json()).toEqual({ error: { code: 'ANALYSIS_FAILED', message: 'Project analysis failed unexpectedly.' } });
    });
  });

  test('validates port before opening a listener', () => {
    for (const port of [-1, 65536, 1.5, Number.NaN]) expect(() => startServer({ port })).toThrow('Port must be an integer');
  });
});
