// Compiler-backed negative evidence without modifying the passing fixture.
import ts from '../../../../node_modules/typescript/lib/typescript.js';
const configPath = '.omx/prepared-tests/typed-factory.tsconfig.json';
const config = ts.readConfigFile(configPath, ts.sys.readFile);
if (config.error) throw new Error(ts.flattenDiagnosticMessageText(config.error.messageText, '\n'));
const parsed = ts.parseJsonConfigFileContent(config.config, ts.sys, '.omx/prepared-tests');
const host = ts.createCompilerHost(parsed.options);
const originalRead = host.readFile;
let removed = 0;
host.readFile = path => {
  const source = originalRead(path);
  if (source === undefined || !path.includes('.omx/prepared-tests/typed-factory') || !path.endsWith('.test-d.ts')) return source;
  return source.replace(/\/\/ @ts-expect-error[^\n]*/g, () => { removed++; return '// Negative assertion deliberately unsuppressed for diagnostic evidence.'; });
};
const program = ts.createProgram(parsed.fileNames, parsed.options, host);
const diagnostics = ts.getPreEmitDiagnostics(program);
process.stdout.write(ts.formatDiagnosticsWithColorAndContext(diagnostics, { getCurrentDirectory: ts.sys.getCurrentDirectory, getCanonicalFileName: f => f, getNewLine: () => '\n' }));
process.stdout.write(`\nTS ${ts.version}; stripped ${removed} negative directives; produced ${diagnostics.length} diagnostics.\n`);
if (removed < 15 || diagnostics.length < removed || diagnostics.some(d => d.code === 2578)) process.exitCode = 1;
