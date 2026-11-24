# @coldmind/core

Shared contracts and type helpers used by the data/contract side of Shirayuki.

## Owns

- validation path/error/result types
- error formatting helpers
- selected reusable TypeScript utility types

## Does not own

Models, persistence, SQL, transports, ActionWire, SignalBus or application runtime behavior.

## Public API

```ts
import {
  formatErrors, formatPath,
  type Path, type ValidationError, type ValidationLevel, type ValidationResult,
  type Exact, type JsonValue, type Merge, type Promisable,
  type ReadonlyDeep, type Simplify, type Tagged, type UnknownRecord,
} from '@coldmind/core'
```

Subpaths:

- `@coldmind/core/validation`
- `@coldmind/core/type-utils`

## Dependency position

`@coldmind/core` depends on no other Shirayuki package. `@coldmind/model`, `@coldmind/data`, and `@coldmind/rpc` build on its contracts.

## Verify

```bash
bun --filter @coldmind/core typecheck
bun --filter @coldmind/core test
```
