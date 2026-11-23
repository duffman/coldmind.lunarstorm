import { resolve } from 'node:path';
import { extractProject, hasErrors } from './model';
import { startServer } from './server';

const [command, argument] = process.argv.slice(2);

try {
  if (command === 'analyze') {
    if (!argument) throw new Error('Usage: bun run src/cli.ts analyze <project-directory>');
    const model = await extractProject(resolve(argument));
    console.log(JSON.stringify(model, null, 2));
    if (hasErrors(model)) process.exitCode = 1;
  } else if (command === 'serve') {
    const port = argument === undefined ? 4321 : Number(argument);
    if (!Number.isInteger(port) || port < 0 || port > 65535) throw new Error('Port must be an integer between 0 and 65535.');
    const server = startServer({ port });
    console.log(`LS build server listening on ${server.url}`);
  } else {
    throw new Error('Usage: bun run src/cli.ts <analyze <project-directory> | serve [port]>');
  }
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
