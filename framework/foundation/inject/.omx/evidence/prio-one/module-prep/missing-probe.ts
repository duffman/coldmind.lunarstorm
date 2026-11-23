import { composeModules, defineModule } from '@coldmind/inject/src/core/cminject.module.ts';
const needsDb = defineModule<{}, { Db: { query(): string } }>()({ name: 'needs-db', register: () => {} });
composeModules(needsDb);
