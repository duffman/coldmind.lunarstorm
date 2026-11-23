import type { DiIssue, ResolutionPathStep } from '@coldmind/inject/contracts';
import type { InjectionToken } from '@coldmind/inject/token';

/** Base class of cminject errors. `code` is a stable identifier for matching. */
export class DiError extends Error {
  /** Stable identifier of the error kind. */
  readonly code: string;
  /** Creates an error with a message and a stable `code`. */
  constructor(message: string, code: string, options?: ErrorOptions) {
    super(message, options); this.code = code; this.name = new.target.name;
  }
}
/** Thrown when a token has no registration in the scope chain. */
export class NotRegisteredError extends DiError {
  /** The token that was looked up. */
  readonly token: InjectionToken | string;
  /** Readable name of the token. */
  readonly tokenDescription: string;
  /** Creates the error for a token and its readable name. */
  constructor(token: InjectionToken | string, description: string) {
    super(`No registration found for token: ${description}`, 'DI_NOT_REGISTERED');
    this.token = token; this.tokenDescription = description;
  }
}
/** Thrown when a tenant or request service is resolved with no matching scope above. */
export class ScopeMissingError extends DiError {
  /** The kind of scope that was missing. */
  readonly scopeKind: 'tenant' | 'request';
  /** Readable name of the token. */
  readonly tokenDescription: string;
  /** Creates the error for the missing scope kind and the token. */
  constructor(scopeKind: 'tenant' | 'request', tokenDescription: string) {
    super(`${scopeKind === 'tenant' ? 'Tenant' : 'Request'} scope is required to resolve ${scopeKind} scoped service: ${tokenDescription}`, 'DI_SCOPE_MISSING');
    this.scopeKind = scopeKind; this.tokenDescription = tokenDescription;
  }
}
/** Thrown when a build depends on itself, directly or across concurrent resolutions. `path` lists the steps. */
export class CircularDependencyError extends DiError {
  /** The cycle: a cause string and the resolution steps that form it. */
  readonly path: { cause: string; steps: ResolutionPathStep[] };
  /** Creates the error for the token that closed the cycle and the steps leading to it. */
  constructor(name: string, steps: ResolutionPathStep[]) {
    super(`Circular dependency detected: ${steps.map(s => `${s.tokenName}@${s.scopeId}`).join(' -> ')}`, 'DI_CIRCULAR_DEPENDENCY');
    this.path = { cause: `circular:${name}`, steps };
  }
}
/** Thrown when a registration change conflicts with live instances or with the token's kind. */
export class MutationConflictError extends DiError {
  /** Kind of the scope involved, when known. */
  readonly scopeKind?: string;
  /** Creates the error with a message and, optionally, the scope kind. */
  constructor(message: string, scopeKind?: string) {
    super(message, 'DI_MUTATION_CONFLICT'); this.scopeKind = scopeKind;
  }
}
/** Thrown when a container or instance is used in a state that does not allow it, such as after disposal. */
export class LifecycleError extends DiError {
  /** Creates the error with a message. */
  constructor(message: string) { super(message, 'DI_LIFECYCLE_ERROR'); }
}
/** Thrown when the declared-dependency graph has problems. `issues` lists them. */
export class DiGraphError extends DiError {
  /** The problems found. */
  readonly issues: readonly DiIssue[];
  /** Creates the error from the problems found. One problem becomes the message; several are listed. */
  constructor(issues: readonly DiIssue[]) {
    super(issues.length === 1 ? issues[0]!.message : `The dependency graph has ${issues.length} problems:\n${issues.map(i => `  - ${i.message}`).join('\n')}`, 'DI_GRAPH_UNSOUND');
    this.issues = issues;
  }
}
/** Type guard for `NotRegisteredError`. */
export function isNotRegisteredError(e: unknown): e is NotRegisteredError { return e instanceof NotRegisteredError; }
/** Type guard for `ScopeMissingError`. */
export function isScopeMissingError(e: unknown): e is ScopeMissingError { return e instanceof ScopeMissingError; }
