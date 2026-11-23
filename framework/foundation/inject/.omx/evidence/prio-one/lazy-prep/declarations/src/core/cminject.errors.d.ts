import type { DiIssue, ResolutionPathStep } from '@coldmind/inject/src/core/cminject.contracts.ts';
import type { InjectionToken } from '@coldmind/inject/src/core/cminject.token.ts';
/** Base class of cminject errors. `code` is a stable identifier for matching. */
export declare class DiError extends Error {
    /** Stable identifier of the error kind. */
    readonly code: string;
    /** Creates an error with a message and a stable `code`. */
    constructor(message: string, code: string, options?: ErrorOptions);
}
/** Thrown when a token has no registration in the scope chain. */
export declare class NotRegisteredError extends DiError {
    /** The token that was looked up. */
    readonly token: InjectionToken | string;
    /** Readable name of the token. */
    readonly tokenDescription: string;
    /** Creates the error for a token and its readable name. */
    constructor(token: InjectionToken | string, description: string);
}
/** Thrown when a tenant or request service is resolved with no matching scope above. */
export declare class ScopeMissingError extends DiError {
    /** The kind of scope that was missing. */
    readonly scopeKind: 'tenant' | 'request';
    /** Readable name of the token. */
    readonly tokenDescription: string;
    /** Creates the error for the missing scope kind and the token. */
    constructor(scopeKind: 'tenant' | 'request', tokenDescription: string);
}
/** Thrown when a build depends on itself, directly or across concurrent resolutions. `path` lists the steps. */
export declare class CircularDependencyError extends DiError {
    /** The cycle: a cause string and the resolution steps that form it. */
    readonly path: {
        cause: string;
        steps: ResolutionPathStep[];
    };
    /** Creates the error for the token that closed the cycle and the steps leading to it. */
    constructor(name: string, steps: ResolutionPathStep[]);
}
/** Thrown when a registration change conflicts with live instances or with the token's kind. */
export declare class MutationConflictError extends DiError {
    /** Kind of the scope involved, when known. */
    readonly scopeKind?: string;
    /** Creates the error with a message and, optionally, the scope kind. */
    constructor(message: string, scopeKind?: string);
}
/** Thrown when a container or instance is used in a state that does not allow it, such as after disposal. */
export declare class LifecycleError extends DiError {
    /** Creates the error with a message. */
    constructor(message: string);
}
/** Thrown when the declared-dependency graph has problems. `issues` lists them. */
export declare class DiGraphError extends DiError {
    /** The problems found. */
    readonly issues: readonly DiIssue[];
    /** Creates the error from the problems found. One problem becomes the message; several are listed. */
    constructor(issues: readonly DiIssue[]);
}
/** Type guard for `NotRegisteredError`. */
export declare function isNotRegisteredError(e: unknown): e is NotRegisteredError;
/** Type guard for `ScopeMissingError`. */
export declare function isScopeMissingError(e: unknown): e is ScopeMissingError;
