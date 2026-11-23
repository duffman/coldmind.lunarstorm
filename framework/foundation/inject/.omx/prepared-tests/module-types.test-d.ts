import { expectTypeOf, test } from 'vitest';
import { composeModules, defineModule, type Module, type ModuleDefinition, type ModuleMissingCapabilities, type ModuleProvides, type ModuleRequires } from '@coldmind/inject/src/core/cminject.module.ts';

/** Database capability used without nominal token-identity claims. */
interface Db { query(): string; }
/** A deliberately incompatible provider. */
interface WrongDb { write(value: number): void; }
/** Service provided by a module with a named requirement. */
interface Users { find(): string; }

const database = defineModule<{ Db: Db }>()({ name: 'database', register: () => {} });
const users = defineModule<{ Users: Users }, { Db: Db }>()({ name: 'users', register: () => {} });
const incompatible = defineModule<{ Db: WrongDb }>()({ name: 'incompatible', register: () => {} });

test('capability declarations and complete composition retain the exact provided map', () => {
  const complete = composeModules(database, users);
  expectTypeOf<ModuleProvides<typeof database>>().toEqualTypeOf<{ Db: Db }>();
  expectTypeOf<ModuleRequires<typeof users>>().toEqualTypeOf<{ Db: Db }>();
  expectTypeOf<ModuleProvides<typeof complete>>().toEqualTypeOf<{ Db: Db; Users: Users }>();
  expectTypeOf<ModuleRequires<typeof complete>>().toEqualTypeOf<{}>();
  expectTypeOf(complete.imports).toEqualTypeOf<readonly [typeof database, typeof users] | undefined>();
});

test('missing and incompatible named providers are compiler errors', () => {
  type Diagnostic = ModuleMissingCapabilities<readonly [typeof users]>;
  expectTypeOf<Diagnostic>().toEqualTypeOf<{ Db: Db }>();
  // @ts-expect-error Db is required but no Db capability is provided.
  composeModules(users);
  // @ts-expect-error A provider named Db must also satisfy the required Db type.
  composeModules(incompatible, users);
});

test('statically declared imports participate in composition completeness', () => {
  const imported = defineModule<{}, { Db: Db }>()({ name: 'imported', imports: [database], register: () => {} });
  const complete = composeModules(imported);
  expectTypeOf<ModuleProvides<typeof complete>>().toEqualTypeOf<{ Db: Db }>();
  const incomplete = defineModule({ name: 'incomplete', imports: [users], register: () => {} });
  // @ts-expect-error Transitive imported Users requires a missing Db provider.
  composeModules(incomplete);
  const transitive = defineModule({ name: 'transitive', imports: [imported], register: () => {} });
  expectTypeOf<ModuleProvides<ReturnType<typeof composeModules<readonly [typeof transitive]>>>>().toEqualTypeOf<{ Db: Db }>();
});

test('duplicate providers preserve a union rather than hiding incompatibility in never', () => {
  const another = defineModule<{ Db: Db }>()({ name: 'another', register: () => {} });
  expectTypeOf<ModuleProvides<ReturnType<typeof composeModules<readonly [typeof database, typeof another, typeof users]>>>>().toEqualTypeOf<{ Db: Db; Users: Users }>();
  // @ts-expect-error One possible Db provider is incompatible, even though another is compatible.
  composeModules(database, incompatible, users);
  expectTypeOf<ModuleProvides<ReturnType<typeof composeModules<readonly [typeof database, typeof incompatible]>>>>().toEqualTypeOf<{ Db: Db | WrongDb }>();
  const impossible = defineModule<{ Db: never }>()({ name: 'impossible', register: () => {} });
  // @ts-expect-error An uninhabitable provider cannot satisfy a real requirement.
  composeModules(impossible, users);
});

test('empty and untyped modules make no implicit capability claims', () => {
  const empty = composeModules();
  const definition = defineModule({ name: 'plain', register: () => {} });
  const plain = composeModules(definition);
  expectTypeOf<ModuleProvides<typeof empty>>().toEqualTypeOf<{}>();
  expectTypeOf<ModuleProvides<typeof plain>>().toEqualTypeOf<{}>();
  const annotation: Module<{ Db: Db }> = database;
  expectTypeOf<ModuleProvides<typeof annotation>>().toEqualTypeOf<{ Db: Db }>();
});

test('dynamic import graphs and widened module arrays are not falsely certified', () => {
  const dynamicImports: ModuleDefinition[] = [database];
  const dynamic = defineModule<{}, { Db: Db }>()({ name: 'dynamic', imports: dynamicImports, register: () => {} });
  // @ts-expect-error A dynamic import array cannot prove the complete transitive capability graph.
  composeModules(dynamic);
  const widened: ModuleDefinition[] = [database, users];
  // @ts-expect-error Static composition requires a finite inferred module tuple.
  composeModules(...widened);
});
