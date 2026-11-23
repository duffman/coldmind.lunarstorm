import { composeModules, defineModule, type ModuleDefinition } from '@coldmind/inject/src/core/cminject.module.ts';
declare function infer<const T extends readonly ModuleDefinition[]>(...modules: T): T;
const nested = infer(defineModule({ name: 'nested', register: () => {} }));
const direct = composeModules({ name: 'literal', register: () => {} });
const original = defineModule({ name: 'stored', register: () => {} });
const stored = composeModules(original);
const nestedComposed = composeModules(defineModule({ name: 'nestedComposed', register: () => {} }));
