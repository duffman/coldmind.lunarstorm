import { stat } from 'node:fs/promises';
import { isAbsolute } from 'node:path';
import { extractProject, hasErrors } from './model';

const MAX_REQUEST_BYTES = 16 * 1024;
type Analyzer = typeof extractProject;

export interface HandlerOptions {
  analyze?: Analyzer;
}

function json(value: unknown, status = 200, headers?: HeadersInit): Response {
  return Response.json(value, {
    status,
    headers: { 'Cache-Control': 'no-store', ...headers },
  });
}

function failure(status: number, code: string, message: string, headers?: HeadersInit): Response {
  return json({ error: { code, message } }, status, headers);
}

/** Read-only project analysis. Project modules are parsed, never imported. */
export function createRequestHandler({ analyze = extractProject }: HandlerOptions = {}) {
  return async (request: Request): Promise<Response> => {
    if (request.headers.has('origin')) return failure(403, 'ORIGIN_NOT_ALLOWED', 'Browser-origin requests are not supported.');
    const path = new URL(request.url).pathname;
    if (path === '/health') {
      return request.method === 'GET'
        ? json({ status: 'ok' })
        : failure(405, 'METHOD_NOT_ALLOWED', 'Use GET for /health.', { Allow: 'GET' });
    }
    if (path !== '/analyze') return failure(404, 'NOT_FOUND', 'Unknown endpoint.');
    if (request.method !== 'POST') return failure(405, 'METHOD_NOT_ALLOWED', 'Use POST for /analyze.', { Allow: 'POST' });

    const contentType = request.headers.get('content-type')?.split(';')[0]?.trim().toLowerCase();
    if (contentType !== 'application/json') return failure(415, 'UNSUPPORTED_MEDIA_TYPE', 'Send an application/json request.');
    const contentLength = request.headers.get('content-length');
    if (contentLength !== null && Number(contentLength) > MAX_REQUEST_BYTES) {
      return failure(413, 'REQUEST_TOO_LARGE', 'Request exceeds 16 KiB.');
    }

    let body: unknown;
    try {
      // Limit streamed requests too; a missing or inaccurate Content-Length is not trusted.
      const reader = request.body?.getReader();
      const chunks: Uint8Array[] = [];
      let length = 0;
      if (reader) {
        try {
          while (true) {
            const { done, value } = await reader.read();
            if (done) break;
            length += value.byteLength;
            if (length > MAX_REQUEST_BYTES) {
              await reader.cancel();
              return failure(413, 'REQUEST_TOO_LARGE', 'Request exceeds 16 KiB.');
            }
            chunks.push(value);
          }
        } finally { reader.releaseLock(); }
      }
      const bytes = new Uint8Array(length);
      let offset = 0;
      for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
      body = JSON.parse(new TextDecoder().decode(bytes));
    } catch {
      return failure(400, 'INVALID_JSON', 'Request body must contain valid JSON.');
    }

    if (body === null || typeof body !== 'object' || Array.isArray(body)
      || Object.keys(body).some(key => key !== 'projectPath')
      || !('projectPath' in body) || typeof body.projectPath !== 'string'
      || !body.projectPath.trim() || body.projectPath.includes('\0') || !isAbsolute(body.projectPath)) {
      return failure(400, 'INVALID_PROJECT_PATH', 'Provide one absolute projectPath string.');
    }
    const projectPath = body.projectPath;
    try {
      if (!(await stat(projectPath)).isDirectory()) {
        return failure(400, 'INVALID_PROJECT_PATH', 'projectPath must identify a directory.');
      }
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code;
      if (code === 'ENOENT' || code === 'ENOTDIR') return failure(404, 'PROJECT_NOT_FOUND', 'Project directory does not exist.');
      return failure(403, 'PROJECT_UNREADABLE', 'Project directory cannot be read.');
    }

    try {
      const model = await analyze(projectPath);
      return json(model, hasErrors(model) ? 422 : 200);
    } catch {
      return failure(500, 'ANALYSIS_FAILED', 'Project analysis failed unexpectedly.');
    }
  };
}

export interface ServerOptions extends HandlerOptions {
  port?: number;
}

/** This development service is intentionally bound only to loopback. */
export function startServer({ port = 5180, ...options }: ServerOptions = {}) {
  if (!Number.isInteger(port) || port < 0 || port > 65535) throw new Error('Port must be an integer from 0 to 65535.');
  return Bun.serve({
    hostname: '127.0.0.1',
    port,
    maxRequestBodySize: MAX_REQUEST_BYTES,
    fetch: createRequestHandler(options),
  });
}
