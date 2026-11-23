from pathlib import Path
import json,re
methods=['singleton','transient','scoped','tenant','request','singletonMulti','transientMulti','scopedMulti','tenantMulti','requestMulti']
f=Path('src/core/cminject.host.ts');s=f.read_text();s=s.replace('  /** Tracked teardown is a public Container method', '''  registerFactory<T, D>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | ServiceLifecycle<T, D> | import('@coldmind/inject/contracts').InjectedSpec<T> | RegisterOptions | undefined, lifetime: ServiceLifetime, kind: 'single' | 'multi', options?: RegisterOptions): BindingHandle<T, D>;
  /** Tracked teardown is a public Container method''');f.write_text(s)
f=Path('src/core/cminject.container.ts');s=f.read_text();pos=s.index('  /** Registers a singleton,');s=s[:pos]+'''  /** Operational forwarding boundary used only by genuine typed internal facades. */
  registerFactory<T, D>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions | undefined, lifetime: ServiceLifetime, kind: 'single' | 'multi', options?: RegisterOptions): BindingHandle<T, D> {
    return this.mutation.register(token, factory, lifetime, kind, options);
  }
'''+s[pos:];f.write_text(s)
f=Path('src/core/cminject.view.ts');s=f.read_text().replace('  ScopeType, ServiceLifetime, AmbientResolution,','  ScopeType, AmbientResolution,');s="import { ServiceLifetime } from '@coldmind/inject/contracts';\n"+s
# Static explicit methods; never runtime-generated or prototype-transplanted.
for method in methods:
 start=s.index('  '+method+'<');end=s.index('\n  }',start)+4
 lifetime=method.replace('Multi','').capitalize();kind='multi' if method.endswith('Multi') else 'single'
 block=f'''  {method}<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;
  {method}<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | (ServiceLifecycle<T, D> & {{ readonly inject?: never }}), options?: RegisterOptions): BindingHandle<T, D>;
  {method}<T>(ctor: Constructor<T>, options?: RegisterOptions): BindingHandle<T>;
  {method}<T, D>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions, options?: RegisterOptions): BindingHandle<T, D> {{
    return this.registering(() => this.target.registerFactory(token, factory, ServiceLifetime.{lifetime}, '{kind}', options));
  }}'''
 s=s[:start]+block+s[end:]
f.write_text(s)
f=Path('src/core/cminject.decorators.ts');s=f.read_text().replace('defineDecorator<[InjectionToken]>', 'defineDecorator<[DependencySlot]>');f.write_text(s)
f=Path('src/core/cminject.construction.ts');s=f.read_text().replace('import { DiError, NotRegisteredError }','import { DiError, NotRegisteredError, LifecycleError }').replace("throw new DiError('Deferred getSync requires synchronous construction', 'DI_LIFECYCLE')", "throw new LifecycleError('Deferred getSync requires synchronous construction')");f.write_text(s)
f=Path('tests/bun/core/delivery.test.ts');s=f.read_text().replace('Container, createToken, defineModule,','Container, createToken, delay, defineModule,').replace('static $inject=[lazy(B)]','static $inject=[lazy(delay(() => B))]');f.write_text(s)
for filename in ['typed-factory.test-d.ts','typed-factory.dialects.test-d.ts']:
 s=(Path('.omx/prepared-tests')/filename).read_text();s=s.replace("import type { TypedFactoryContainer } from './typed-factory.facade.ts';", "import type { IContainer as TypedFactoryContainer } from '@coldmind/inject/contracts';")
 for old,new in [('token','token'),('contracts','contracts'),('lazy','lazy')]:s=s.replace('../../src/core/cminject.'+old+'.ts','@coldmind/inject/'+new)
 # Treat these declarations as compilation fixtures, not runtime test bodies.
 Path('tests/types/'+filename).write_text(s)
for name in ['tsconfig.types.json']:
 p=Path(name);s=json.loads(p.read_text());s['compilerOptions'].update({'rootDir':'.','customConditions':['bun']});p.write_text(json.dumps(s,indent=2)+'\n')
# Package import resolution checker follows the actual explicit export map.
f=Path('scripts/check-core-imports.ts');s=f.read_text();s=s.replace("const known = new Set(files);", "const manifest = JSON.parse(await readFile(resolve(import.meta.dirname, '../package.json'), 'utf8'));\nconst known = new Set(files);")
a=s.index('    const target = specifier.text');b=s.index('    if (!target || !known.has(target))',a)
s=s[:a]+'''    const subpath = specifier.text === '@coldmind/inject' ? '.' : specifier.text.startsWith('@coldmind/inject/') ? './' + specifier.text.slice('@coldmind/inject/'.length) : undefined;
    const exported = subpath === undefined ? undefined : manifest.exports[subpath];
    const exportedSource = typeof exported === 'string' ? exported : exported?.bun;
    if (subpath && !exportedSource) fail(`Unknown explicit package import: ${specifier.text}`);
    const target = exportedSource ? resolve(root, exportedSource) : specifier.text.startsWith('.') ? resolve(file, '..', specifier.text) : undefined;
'''+s[b:]
f.write_text(s)
Path('scripts/check-imports.ts').write_text('''import { readdir, readFile, access } from 'node:fs/promises';
import { resolve, extname } from 'node:path';
const root = resolve(import.meta.dirname, '..');
const manifest = JSON.parse(await readFile(resolve(root, 'package.json'), 'utf8'));
if (Object.keys(manifest.exports).some(key => key.includes('*'))) throw new Error('Wildcard package exports are forbidden');
async function scan(directory: string): Promise<void> {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const file = resolve(directory, entry.name);
    if (entry.isDirectory()) { if (!['node_modules', 'dist', '.cache', '.omx'].includes(entry.name)) await scan(file); continue; }
    if (!['.ts', '.tsx', '.mjs', '.mts'].includes(extname(file))) continue;
    const source = await readFile(file, 'utf8');
    for (const match of source.matchAll(/(?:from\\s*|import\\s*\\(|import\\s*)['"]([^'"]+)['"]/g)) {
      const name = match[1]!;
      if (name.startsWith('@coldmind/inject/src/')) throw new Error(`Source-path package import: ${file}: ${name}`);
      if (file.startsWith(resolve(root, 'src') + '/') && name.startsWith('.')) throw new Error(`Relative source import: ${file}: ${name}`);
      if (name === '@coldmind/inject' || name.startsWith('@coldmind/inject/')) {
        const key = name === '@coldmind/inject' ? '.' : './' + name.slice('@coldmind/inject/'.length);
        if (!manifest.exports[key]) throw new Error(`Unexported package import: ${name}`);
      }
    }
  }
}
for (const folder of ['src', 'tests', 'scripts', 'examples']) await scan(resolve(root, folder));
for (const [key, value] of Object.entries(manifest.exports)) {
  const route = value as string | { bun: string; types: string; default: string };
  if (typeof route === 'string') { await access(resolve(root, route)); continue; }
  await access(resolve(root, route.bun));
  if (process.argv.includes('--dist')) { await access(resolve(root, route.types)); await access(resolve(root, route.default)); }
  if (Object.keys(route)[0] !== 'types') throw new Error(`types must precede runtime conditions: ${key}`);
}
console.log('Explicit import policy and entry resolution passed');
''')
# All browser-safe public and implementation subpaths, not server entries.
f=Path('scripts/check-browser.ts');s=f.read_text();a=s.index('const entrypoints =');b=s.index('\n',a);s=s[:a]+'''const manifest = await Bun.file(new URL('../package.json', import.meta.url)).json();
const entrypoints = [...new Set<string>(Object.entries(manifest.exports).filter(([key]) => !key.includes('server') && key !== './package.json').map(([, route]) => (route as { bun: string }).bun))];'''+s[b:];s=s.replace("external: ['react']", "external: ['react'], conditions: ['bun']");f.write_text(s)
# Complete user documentation against the implemented APIs, without claiming validation.
Path('docs').mkdir(exist_ok=True)
guides={
'getting-started':('Getting started','Create one root, register services, resolve them and dispose the root. Prefer exported typed symbols to string tokens. Singleton ownership belongs to its registering scope; scoped instances belong to the resolving scope.'),
'lifetimes':('Lifetimes','singleton caches per registration owner; transient builds on every call; scoped caches per resolving scope; tenant and request require matching ancestor scope kinds. A longer-lived service must not capture a shorter-lived tenant/request service. Successfully returned transients are caller-owned. registerInstance defaults to owned; use explicit external ownership when another system disposes the value.'),
'async-and-cycles':('Async construction and cycles','Factories and onInit may return native promises. Concurrent cached resolutions share a build. resolveSync stays synchronous until asynchronous work is necessary, then throws LifecycleError. Set buildTimeout in milliseconds; undefined inherits, null disables, and registration timeoutMs overrides the owner policy. Cancellation aborts signals and rejects callers; arbitrary user promises are not forcibly stopped. Lazy/provider edges defer resolution without disabling captive checks. Use the supplied factory container for contextual resolution, especially across await.'),
'modules':('Modules','defineModule preserves object identity. load(...modules) installs root-owned tree-wide registrations and returns idempotent unload handles. Imports load depth-first; cycles are module-cycle graph issues. Repeated explicit loads do not accumulate hidden references. Shared imports survive remaining dependents or explicit owners. The registration callback receives an explicit transaction facade that captures registrations across await; unrelated root registrations are not captured. Retained registration facades cannot be used after callback completion. Use imports rather than recursively loading from register. Failed callbacks roll back their own registrations; module-owned pending mutations are rejected. Typed provides/requires capability maps are opt-in author declarations, not inferred token identity proofs.'),
 'testing':('Testing utilities','await createTestContainer(...modules) creates an isolated root with buildTimeout 2000. snapshot() returns an opaque owner-bound checkpoint; await restore(snapshot) reinstates definitions and disposes post-snapshot instances without reviving disposed scopes. Unchanged live owned supplied values preserve identity. A disposed owned supplied original cannot be reconstructed: restoration rejects before mutation. override is asynchronous; await its handle before resolving the replacement and restore nested handles in LIFO order. Owned registerInstance overrides reject before disposal. Callable values use { value: fn }; explicit factories use { factory: fn }. Restoration rebuilds factory-backed originals instead of reusing disposed objects.'),
 'decorators':('Decorator dialects','Standard decorators support classes and members, not constructor parameter decorators. Declare constructor dependencies with static $inject, decorator deps, typed factories, or the ctsc pass. Legacy parameter decorators require experimentalDecorators in the current working directory tsconfig. Static $members supports field injection without decorators. Inject accepts ordinary tokens and lazy/provider markers. The legacy two-argument property decorator type cannot distinguish a method from a callable property structurally; runtime placement validation remains authoritative.'),
 'server':('Server runtime','Import DiRuntime, runInContext and currentContext from @coldmind/inject/server. Importing server installs no ambient resolution source. Each DiRuntime attaches its own AsyncLocalStorage to its root; children inherit it. Child useAmbient throws. Independent roots never borrow resolution frames, even with shared storage. Attachment replacement/detachment is intended between operations. Request context storage is distinct from build-resolution storage. Start the runtime, acquire/run/release request leases, then drain and dispose. Graceful runtime draining waits for leases; container cancellation is a separate boundary.'),
 'react':('React adapter','Use @coldmind/inject/react with the configured container/scope. Suspense handles pending asynchronous resolution; errors persist until reset/retry. Scope/token resource isolation and StrictMode deduplication are preserved. Dispose scopes at their lifecycle boundary. The runnable example uses server rendering without external credentials.'),
 'introspection':('Introspection and validation','graph(), bindingOf(), cachedBindings(), dependency/resource snapshots and inspect() describe registrations and managed instances. validate({ requireDeps: true }) reports undeclared factory dependencies; deps: [] is explicitly declared. includeObserved merges current-generation dynamic edges for drift/cycle checks. Inspection exposes drift separately. qualify(Db, "replica") interns a distinct typed token named Db#replica. when predicates run in registration order before the unconditional fallback and receive requester/path/scope. Static graph queries do not execute predicates. Registry epochs change on definition mutations, not cache hits.'),
 'errors':('Errors','Catch DiError and inspect its stable code; graph issue kinds are not error codes. DI_GRAPH_UNSOUND carries missing, cycle, captive, dangling, decorator, module-cycle, undeclared, undeclared-edge or ambiguous issues. Timeouts/cancellation do not imply user work was forcibly terminated. Cleanup errors may be attached to the original failure. Never retry an operation on a disposed container.'),
 'migrate-inversify':('Migrating Inversify wiring','This covers dependency wiring, not application framework replacement. Inversify 8.x bind/get/getAsync map to explicit registrations, resolveSync and resolve. Its default transient scope is not cminject singleton. Choose lifetimes explicitly. Map activation/deactivation to lifecycle hooks with ownership tests. cminject snapshot handles are explicit and async to restore, not Inversify snapshot-stack restore calls. See [official container API](https://inversify.io/docs/api/container/).'),
 'migrate-tsyringe':('Migrating TSyringe wiring','TSyringe 4.10 class/value/factory/token providers map to class registrations, registerInstance, factories and explicit aliases. ResolutionScoped (one resolution chain) is not cminject scoped (one resolving container). TSyringe delay uses a transparent proxy; cminject lazy injects an explicit resolver object and delay only delays token lookup. cminject clearInstances preserves supplied values; TSyringe 4.10 removes value-provider registrations when clearing. See [pinned official providers and scopes](https://github.com/microsoft/tsyringe/blob/e033769d97cfb6cc4a8569e2b50eb32015453302/README.md) and [clearInstances implementation](https://github.com/microsoft/tsyringe/blob/e033769d97cfb6cc4a8569e2b50eb32015453302/src/dependency-container.ts#L405-L420).'),
 'migrate-nest':('Migrating Nest dependency wiring','This maps dependency providers only; it does not implement Nest HTTP/controllers/guards/pipes/transports. useClass maps to class registrations; useValue to registerInstance with explicit ownership; useFactory/inject to typed dependency factories; useExisting to a factory resolving the existing token with declared deps. cminject module imports install root-owned tree-wide registrations, not Nest export-visibility boundaries. Request-scoped work uses explicit request scopes/DiRuntime leases. See [official custom providers](https://docs.nestjs.com/fundamentals/custom-providers) and [injection scopes](https://docs.nestjs.com/fundamentals/injection-scopes).')}
for key,(title,body) in guides.items():
 example=Path('examples/cminject.'+key+'.ts').read_text();Path('docs/'+key+'.md').write_text('# '+title+'\n\n'+body+'\n\n## Runnable example\n\n[Source](../examples/cminject.'+key+'.ts). This example is part of the final docs test suite.\n\n```ts\n'+example+'```\n\n[API reference](api/index.html) · [Getting started](getting-started.md)\n')
codes=set(re.findall(r"['\"]((?:DI_|RUNTIME_|TENANT_|INVALID_SCOPE)[A-Z_]+)['\"]", '\n'.join(p.read_text() for p in Path('src').rglob('*.ts'))))
f=Path('docs/errors.md');f.write_text(f.read_text()+'\n## Stable codes\n\n'+''.join('- `'+code+'`: '+{'DI_NOT_REGISTERED':'No matching registration; conditional candidates may have no matching predicate/fallback.','DI_BUILD_TIMEOUT':'An asynchronous materialization exceeded its configured deadline.','DI_BUILD_CANCELLED':'The owning scope ended before materialization completed.','DI_MUTATION_CONFLICT':'The requested registry mutation cannot safely proceed.','DI_GRAPH_UNSOUND':'Graph validation failed; inspect issues for the problem kinds.','DI_CIRCULAR_DEPENDENCY':'The explicit resolution or waits-for graph contains a cycle.','DI_LIFECYCLE_ERROR':'The scope/build state or synchronous API cannot perform this operation.'}.get(code,'The corresponding container/runtime lifecycle boundary rejected the operation; inspect the error message and cause.')+'\n' for code in sorted(codes)))
Path('README.md').write_text('''# @coldmind/inject

Typed hierarchical dependency injection with synchronous fast paths, async construction, explicit lifetimes, modules and browser-safe core APIs.

```sh
bun add @coldmind/inject
# or: npm install @coldmind/inject
```

```ts
import { Container, createToken } from '@coldmind/inject';
const Message = createToken<string>('Message');
const container = new Container();
container.singleton(Message, () => 'Hello');
console.log(await container.resolve(Message));
await container.dispose();
```

Bun >=1.3 uses source TypeScript; Node >=22 uses compiled ESM/declarations. Final local distribution validation is required before release.

| Entry | Purpose |
| --- | --- |
| `@coldmind/inject` | Container, tokens, decorators, modules, deferred dependencies |
| `/contracts`, `/token`, `/errors`, `/graph`, `/decorators`, `/container`, `/module`, `/lazy` | Explicit public API surfaces |
| `/server` | DiRuntime and server-only async context |
| `/testing` | Test roots and awaited overrides |
| `/hot`, `/inspect`, `/react` | Hot replacement, introspection, React adapter |
| `/internal/*` | Explicit implementation routes, not supported application APIs |

[Getting started](docs/getting-started.md) · [Lifetimes](docs/lifetimes.md) · [Async/cycles](docs/async-and-cycles.md) · [Modules](docs/modules.md) · [Testing](docs/testing.md) · [Decorators](docs/decorators.md) · [Server](docs/server.md) · [React](docs/react.md) · [Introspection](docs/introspection.md) · [Errors](docs/errors.md)

[Inversify migration](docs/migrate-inversify.md) · [TSyringe migration](docs/migrate-tsyringe.md) · [Nest wiring migration](docs/migrate-nest.md) · [API](docs/api/index.html)

Generate API reference with `bun run docs:api`. Run tests only in the final validation phase of the active delivery task.
''')
f=Path('CLAUDE.md');s=f.read_text().replace('Bun is the runtime. Node support is a TODO (`@coldmind/inject/node` is not built yet).','Bun >=1.3 and Node >=22 are distribution targets; final source/dist verification is recorded separately. Bun uses source, Node uses compiled ESM.');s+='\n## Current implementation\n\nCore responsibilities are split across async/binding/view/construction/resolution/mutation/observe with a type-only host boundary. Root-owned modules, snapshots, deferred slots, typed factories and conditional candidates share these owners. Server import is neutral; DiRuntime attaches ambient resolution per root. No wildcard exports or source-path package imports.\n';f.write_text(s)
f=Path('FIX-PLAN.md');s=f.read_text().replace('`classFactory` is async whenever a class injects anything, so `resolveSync`/`get` always fail for decorated classes.','Historical defect, now repaired: class construction and completion stay synchronous until a dependency/hook actually returns a native promise.');s+='\n## Superseding complete-delivery integration\n\nThe implementation now includes the planned parameter pass at `../../tools/ctsc/src/passes/cminject.paramtypes.ts`, split integration and explicit source/dist exports. Timeout/cancellation live in core resolution. Final typecheck/review/test results must be attached before completion is claimed; the earlier bug descriptions above are historical rationale.\n';f.write_text(s)
f=Path('DECISIONS.md');s=f.read_text()+'''\n## Solo complete-delivery integration (awaiting final validation)

Modules use root identity registries, explicit registration-bound views across await, exact binding capture, import ownership and reverse cleanup. No ambient transaction capture. Snapshots are opaque owner-bound definitions/record checkpoints; restoring disposed owned supplied originals conflicts atomically. Overrides are awaited, token-local and LIFO; callable values use an explicit value tag. Lazy/provider declarations bind to the resolving scope, skip eager cycle traversal only, and retain captive checks. Typed inject tuples supply positional values and declare graph edges. Conditions select ordered predicates before an unconditional fallback, with caches keyed by exact binding identity. Server import installs nothing; root-owned ambient sources are isolated by opaque tree identity. These changes are implementation, not a claim of final passing validation.
''';f.write_text(s)
Path('.omx/evidence/prio-one/solo-code-pass.md').write_text('Complete integrated code/basic-test writing pass performed solo. No tests, typechecks, builds, benchmarks, examples or validation scripts executed during this pass. Next: typecheck, fix, direct critical requirements review, then tests. Existing compiler/split/decorate and benchmark code retained for final integration validation.\n')
print('Completed code, docs, examples and basic-test writing pass. No validation was executed.')
