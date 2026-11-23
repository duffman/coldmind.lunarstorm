import type { DependencyEdge, DiIssue, DiNode } from '@coldmind/inject/src/core/cminject.contracts.ts';
import { type InjectionToken } from '@coldmind/inject/src/core/cminject.token.ts';
/** Options for `DiGraph`. */
export interface DiGraphOptions {
    /** The parent scope's graph. A child does not copy it: a parent registration made later is visible here too. */
    parent?: DiGraph;
    /** How a token is named in messages. */
    describe?: (token: InjectionToken) => string;
    /** Called when a mutation introduces a problem. The default throws. */
    onIssue?: (issue: DiIssue) => void;
    /** Retains a raw key only when a public query or thrown diagnostic exposes it. */
    onExpose?: (key: symbol) => void;
}
/**
 * Mutable declared-dependency graph. Forward edges, reverse edges and the token index are
 * maintained on every mutation, so queries are lookups. Cycle detection is Tarjan SCC,
 * linear in the reachable graph, so diamonds do not blow up.
 */
export declare class DiGraph {
    /** The parent scope's graph. A child does not copy it: parent registrations made later are visible here too. */
    private readonly parent?;
    /** Identity → node. For singles, identity is the token key. */
    private readonly local;
    /** Token key → identities that provide it. Multi-inject has many. */
    private readonly byToken;
    /** Token key → identities of the local nodes that depend on it. */
    private readonly reverse;
    /** How a token is named in messages. */
    private readonly describe;
    /** Called when a mutation introduces a problem. */
    private readonly onIssue;
    /** Optional owner callback for raw-key query results. */
    private readonly onExpose?;
    /** Creates a graph, optionally layered over a parent. */
    constructor(options?: DiGraphOptions);
    /** Number of nodes registered in this scope. */
    get size(): number;
    /** Whether this scope has a registration for the key. */
    has(key: symbol): boolean;
    /** Whether this scope or an ancestor has a registration for the key. */
    resolves(key: symbol): boolean;
    /** The nearest node for a key, in this scope or an ancestor. */
    get(key: symbol): DiNode | undefined;
    /** Nodes registered in this scope. */
    nodes(): DiNode[];
    /** Local nodes that depend on this token. */
    dependents(key: symbol): DiNode[];
    /** The declared dependencies of a key's node. */
    dependencies(key: symbol): DependencyEdge[];
    /** Single-edge dependencies that nothing provides, here or in an ancestor. */
    unresolved(): DependencyEdge[];
    /** Nodes whose dependencies are unknown, so the graph cannot check them. */
    undeclared(): DiNode[];
    /** Add a node and check what is already knowable: captives on its edges, cycles through it, and captives of its dependents. */
    add(node: DiNode): void;
    /** Remove every local node for this token. Dependents left without a provider are reported unless `silent`. */
    remove(key: symbol, silent?: boolean): void;
    /** Every problem in the graph as it stands, including unresolved edges and cycles. */
    issues(): DiIssue[];
    /** Throws `DiGraphError` when `issues()` is not empty. */
    assertSound(): void;
    /** Dependencies before consumers, for local registrations. Recomputed per call. */
    order(): DiNode[];
    /** Reports only the keys actually returned as part of a public node. */
    private exposeNode;
    /** Reports raw keys contained in a returned or thrown diagnostic. */
    private exposeIssue;
    /** Reads reverse-index nodes without exposing keys during internal mutation or traversal. */
    private localDependents;
    /** Returns the set stored under a key, creating it when missing. */
    private index;
    /** Local nodes that provide a token key. */
    private localProviders;
    /** Drop a local node from every index. */
    private detach;
    /** Removes one local node and reports its dependents as dangling. */
    private removeOne;
    /** Nearest provider for a single edge; every provider for a multi edge. */
    private locate;
    /** Every provider of a key, here and in ancestors, without duplicates. */
    private locateAll;
    /** Captive-lifetime problems on a node's edges. A singleton holding a shorter-lived provider is captive. */
    private captivesOf;
    /** Builds a captive-lifetime issue for one edge. */
    private captiveIssue;
    /** Successors of a vertex. A singleton resolves its edges in the scope that owns it; everything else resolves in this scope. */
    private successors;
    /** Tarjan's strongly connected components over everything reachable from `starts`. */
    private components;
    /**
     * Cycle issues, one per strongly connected component. Only components that hold a local node are reported:
     * a cycle wholly inside an ancestor belongs to that ancestor's issues.
     * With `mustInclude`, only components containing that node are reported, and the path starts there.
     */
    private cycleIssues;
    /** Shortest closed path from `origin` back to itself, staying inside the component. */
    private cyclePath;
    /** Builds a cycle issue for a path that returns to its start. */
    private cycleIssue;
}
