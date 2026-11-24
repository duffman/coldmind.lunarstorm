import type { DependencyEdge, DiIssue, DiNode } from '@coldmind/inject/contracts';
import { ServiceLifetime } from '@coldmind/inject/contracts';
import { DiGraphError } from '@coldmind/inject/errors';
import { tokenName, type InjectionToken } from '@coldmind/inject/token';

/** How long an instance lives, widest first. Transient and Scoped are absent: they adopt the container that resolves them. */
const BREADTH: ReadonlyMap<ServiceLifetime, number> = new Map([
  [ServiceLifetime.Singleton, 3],
  [ServiceLifetime.Tenant, 2],
  [ServiceLifetime.Request, 1],
]);

/** Lower-case name of a lifetime, for messages. */
const lifetimeName = (value: ServiceLifetime) => ServiceLifetime[value]!.toLowerCase();
/** The identity a node is stored under: its own identity, or its key. */
const identityOf = (node: DiNode): symbol => node.identity ?? node.key;

/** A node together with the graph (this one or an ancestor) that holds it. */
interface Vertex {
  /** The graph that holds the node. */
  readonly graph: DiGraph;
  /** The node. */
  readonly node: DiNode;
}

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
export class DiGraph {
  /** The parent scope's graph. A child does not copy it: parent registrations made later are visible here too. */
  private readonly parent?: DiGraph;
  /** Identity → node. For singles, identity is the token key. */
  private readonly local = new Map<symbol, DiNode>();
  /** Token key → identities that provide it. Multi-inject has many. */
  private readonly byToken = new Map<symbol, Set<symbol>>();
  /** Token key → identities of the local nodes that depend on it. */
  private readonly reverse = new Map<symbol, Set<symbol>>();
  /** How a token is named in messages. */
  private readonly describe: (token: InjectionToken) => string;
  /** Called when a mutation introduces a problem. */
  private readonly onIssue: (issue: DiIssue) => void;
  /** Optional owner callback for raw-key query results. */
  private readonly onExpose?: (key: symbol) => void;

  /** Creates a graph, optionally layered over a parent. */
  constructor(options: DiGraphOptions = {}) {
    this.parent = options.parent;
    this.describe = options.describe ?? tokenName;
    this.onExpose = options.onExpose;
    this.onIssue = options.onIssue ?? (issue => { this.exposeIssue(issue); throw new DiGraphError([issue]); });
  }

  /** Number of nodes registered in this scope. */
  get size(): number { return this.local.size; }
  /** Whether this scope has a registration for the key. */
  has(key: symbol): boolean { return this.local.has(key) || (this.byToken.get(key)?.size ?? 0) > 0; }
  /** Whether this scope or an ancestor has a registration for the key. */
  resolves(key: symbol): boolean { return this.has(key) || (this.parent?.resolves(key) ?? false); }
  /** The nearest node for a key, in this scope or an ancestor. */
  get(key: symbol): DiNode | undefined { const node = this.locate(key)?.node; if (node) this.exposeNode(node); return node; }
  /** Nodes registered in this scope. */
  nodes(): DiNode[] { const nodes = [...this.local.values()]; for (const node of nodes) this.exposeNode(node); return nodes; }

  /** Local nodes that depend on this token. */
  dependents(key: symbol): DiNode[] {
    const nodes = this.localDependents(key);
    for (const node of nodes) this.exposeNode(node);
    return nodes;
  }

  /** The declared dependencies of a key's node. */
  dependencies(key: symbol): DependencyEdge[] {
    const edges = [...(this.locate(key)?.node.deps ?? [])];
    for (const edge of edges) this.onExpose?.(edge.key);
    return edges;
  }

  /** Single-edge dependencies that nothing provides, here or in an ancestor. */
  unresolved(): DependencyEdge[] {
    const edges = [...this.local.values()].flatMap(node => node.deps.filter(edge => !edge.multi && !this.resolves(edge.key)));
    for (const edge of edges) this.onExpose?.(edge.key);
    return edges;
  }

  /** Nodes whose dependencies are unknown, so the graph cannot check them. */
  undeclared(): DiNode[] { const nodes = [...this.local.values()].filter(node => !node.declared); for (const node of nodes) this.exposeNode(node); return nodes; }

  /** Add a node and check what is already knowable: captives on its edges, cycles through it, and captives of its dependents. */
  add(node: DiNode): void {
    const id = identityOf(node);
    if (this.local.has(id)) this.detach(id);

    this.local.set(id, node);
    this.index(this.byToken, node.key).add(id);
    for (const edge of node.deps) this.index(this.reverse, edge.key).add(id);

    for (const issue of this.captivesOf(node)) this.onIssue(issue);
    // A cycle through the new node needs an edge out of it.
    if (node.deps.length) for (const issue of this.cycleIssues([{ graph: this, node }], node)) this.onIssue(issue);

    // The new node can be the narrower provider a dependent was already pointing at.
    if (!this.reverse.has(node.key)) return;
    for (const dependent of this.localDependents(node.key)) {
      if (identityOf(dependent) === id) continue;
      for (const issue of this.captivesOf(dependent, node)) this.onIssue(issue);
    }
  }

  /** Remove every local node for this token. Dependents left without a provider are reported unless `silent`. */
  remove(key: symbol, silent = false): void {
    const ids = this.local.has(key) ? [key] : [...(this.byToken.get(key) ?? [])];
    for (const id of ids) this.removeOne(id, silent);
  }

  /** Every problem in the graph as it stands, including unresolved edges and cycles. */
  issues(): DiIssue[] {
    const found: DiIssue[] = [];
    for (const node of this.local.values()) for (const edge of node.deps) {
      if (edge.multi || edge.optional || this.resolves(edge.key)) continue;
      found.push({
        kind: 'missing', at: node.key, edge,
        message: `${this.describe(node.token)}${node.module ? ` [${node.module}]` : ''} depends on ${this.describe(edge.token)} (${edge.property === undefined ? `parameter ${edge.index}` : `member ${String(edge.property)}`}), which nothing registers.`,
      });
    }
    for (const identities of this.byToken.values()) {
      const unconditional = [...identities].map(id => this.local.get(id)!).filter(node => !node.conditional && !node.multi);
      if (unconditional.length > 1) found.push({ kind: 'ambiguous', at: unconditional[0]!.key, message: `Ambiguous unconditional providers for ${this.describe(unconditional[0]!.token)}` });
    }
    for (const node of this.local.values()) found.push(...this.captivesOf(node));
    found.push(...this.cycleIssues([...this.local.values()].map(node => ({ graph: this, node }))));
    for (const issue of found) this.exposeIssue(issue);
    return found;
  }

  /** Throws `DiGraphError` when `issues()` is not empty. */
  assertSound(): void {
    const issues = this.issues();
    if (issues.length) throw new DiGraphError(issues);
  }

  /** Dependencies before consumers, for local registrations. Recomputed per call. */
  order(): DiNode[] {
    const result: DiNode[] = [];
    const done = new Set<DiNode>();
    const visiting = new Set<DiNode>();
    const visit = (node: DiNode): void => {
      if (done.has(node)) return;
      if (visiting.has(node)) {
        this.onExpose?.(node.key);
        throw new DiGraphError([{ kind: 'cycle', at: node.key, message: `order() hit a cycle at ${this.describe(node.token)}` }]);
      }
      visiting.add(node);
      for (const edge of node.deps.filter(edge => !edge.lazy)) for (const target of this.localProviders(edge.key)) visit(target);
      visiting.delete(node);
      done.add(node);
      result.push(node);
    };
    for (const node of this.local.values()) visit(node);
    for (const node of result) this.exposeNode(node);
    return result;
  }

  /** Reports only the keys actually returned as part of a public node. */
  private exposeNode(node: DiNode): void {
    this.onExpose?.(node.key);
    if (node.identity) this.onExpose?.(node.identity);
    for (const edge of node.deps) this.onExpose?.(edge.key);
  }
  /** Reports raw keys contained in a returned or thrown diagnostic. */
  private exposeIssue(issue: DiIssue): void {
    this.onExpose?.(issue.at);
    if (issue.edge) this.onExpose?.(issue.edge.key);
    for (const key of issue.path ?? []) this.onExpose?.(key);
  }
  /** Reads reverse-index nodes without exposing keys during internal mutation or traversal. */
  private localDependents(key: symbol): DiNode[] {
    return [...(this.reverse.get(key) ?? [])].map(id => this.local.get(id)).filter((node): node is DiNode => node !== undefined);
  }

  /** Returns the set stored under a key, creating it when missing. */
  private index(map: Map<symbol, Set<symbol>>, key: symbol): Set<symbol> {
    let set = map.get(key);
    if (!set) { set = new Set(); map.set(key, set); }
    return set;
  }

  /** Local nodes that provide a token key. */
  private localProviders(key: symbol): DiNode[] {
    return [...(this.byToken.get(key) ?? [])].map(id => this.local.get(id)!);
  }

  /** Drop a local node from every index. */
  private detach(id: symbol): void {
    const node = this.local.get(id);
    if (!node) return;
    for (const edge of node.deps) {
      const set = this.reverse.get(edge.key);
      set?.delete(id);
      if (set?.size === 0) this.reverse.delete(edge.key);
    }
    const tokenSet = this.byToken.get(node.key);
    tokenSet?.delete(id);
    if (tokenSet?.size === 0) this.byToken.delete(node.key);
    this.local.delete(id);
  }

  /** Removes one local node and reports its dependents as dangling. */
  private removeOne(id: symbol, silent: boolean): void {
    const node = this.local.get(id);
    if (!node) return;
    const orphaned = this.localDependents(node.key).filter(dependent => identityOf(dependent) !== id);
    this.detach(id);
    if (silent || this.resolves(node.key)) return;

    for (const dependent of orphaned) {
      const edge = dependent.deps.find(candidate => candidate.key === node.key);
      if (!edge || edge.optional || edge.multi) continue;
      this.onIssue({
        kind: 'dangling', at: dependent.key, edge,
        message: `${this.describe(dependent.token)} depends on ${this.describe(node.token)}, which was just unregistered. Nothing provides it now.`,
      });
    }
  }

  /** Nearest provider for a single edge; every provider for a multi edge. */
  private locate(key: symbol): Vertex | undefined {
    const direct = this.local.get(key);
    if (direct) return { graph: this, node: direct };
    for (const id of this.byToken.get(key) ?? []) {
      const node = this.local.get(id);
      if (node) return { graph: this, node };
    }
    return this.parent?.locate(key);
  }

  private locateCandidates(key: symbol): Vertex[] {
    const providers = this.localProviders(key);
    return providers.length ? providers.map(node => ({ graph: this, node })) : this.parent?.locateCandidates(key) ?? [];
  }
  /** Every provider of a key, here and in ancestors, without duplicates. */
  private locateAll(key: symbol): Vertex[] {
    const found: Vertex[] = [];
    const seen = new Set<DiNode>();
    const add = (vertex: Vertex) => {
      if (seen.has(vertex.node)) return;
      seen.add(vertex.node);
      found.push(vertex);
    };
    const direct = this.local.get(key);
    if (direct) add({ graph: this, node: direct });
    for (const id of this.byToken.get(key) ?? []) {
      const node = this.local.get(id);
      if (node) add({ graph: this, node });
    }
    for (const vertex of this.parent?.locateAll(key) ?? []) add(vertex);
    return found;
  }

  /** Captive-lifetime problems on a node's edges. A singleton holding a shorter-lived provider is captive. */
  private captivesOf(node: DiNode, onlyProvider?: DiNode): DiIssue[] {
    // Scoped and transient adopt the container that resolves them.
    if (!node.deps.length || node.lifetime === ServiceLifetime.Transient || node.lifetime === ServiceLifetime.Scoped) return [];
    const consumer = BREADTH.get(node.lifetime);
    if (consumer === undefined) return [];

    const out: DiIssue[] = [];
    for (const edge of node.deps) {
      const providers = edge.multi ? this.locateAll(edge.key) : this.locateCandidates(edge.key);
      for (const { node: dependency } of providers) {
        if (onlyProvider && dependency !== onlyProvider) continue;
        // A singleton factory runs on its owner and would cache a scoped instance there. A tenant factory must not either.
        if (dependency.lifetime === ServiceLifetime.Scoped) {
          if (node.lifetime !== ServiceLifetime.Singleton && node.lifetime !== ServiceLifetime.Tenant) continue;
          out.push(this.captiveIssue(node, edge, dependency));
          continue;
        }
        const held = BREADTH.get(dependency.lifetime);
        if (held === undefined || held >= consumer) continue;
        out.push(this.captiveIssue(node, edge, dependency));
      }
    }
    return out;
  }

  /** Builds a captive-lifetime issue for one edge. */
  private captiveIssue(node: DiNode, edge: DependencyEdge, dependency: DiNode): DiIssue {
    return {
      kind: 'captive', at: node.key, edge,
      message: `${this.describe(node.token)} is ${lifetimeName(node.lifetime)} but depends on ${this.describe(dependency.token)}, which is ${lifetimeName(dependency.lifetime)}. The ${lifetimeName(dependency.lifetime)} instance would be captured by the ${lifetimeName(node.lifetime)} and outlive its scope.`,
    };
  }

  /** Successors of a vertex. A singleton resolves its edges in the scope that owns it; everything else resolves in this scope. */
  private successors(vertex: Vertex): Vertex[] {
    const resolver = vertex.node.lifetime === ServiceLifetime.Singleton ? vertex.graph : this;
    const out: Vertex[] = [];
    for (const edge of vertex.node.deps) {
      if (edge.lazy) continue;
      if (edge.multi) { out.push(...resolver.locateAll(edge.key)); continue; }
      out.push(...resolver.locateCandidates(edge.key));
    }
    return out;
  }

  /** Tarjan's strongly connected components over everything reachable from `starts`. */
  private components(starts: readonly Vertex[]): Vertex[][] {
    const index = new Map<DiNode, number>();
    const low = new Map<DiNode, number>();
    const onStack = new Set<DiNode>();
    const stack: Vertex[] = [];
    const out: Vertex[][] = [];
    let counter = 0;

    const connect = (vertex: Vertex): void => {
      const v = vertex.node;
      index.set(v, counter);
      low.set(v, counter);
      counter++;
      stack.push(vertex);
      onStack.add(v);

      for (const next of this.successors(vertex)) {
        const w = next.node;
        if (!index.has(w)) {
          connect(next);
          low.set(v, Math.min(low.get(v)!, low.get(w)!));
        } else if (onStack.has(w)) {
          low.set(v, Math.min(low.get(v)!, index.get(w)!));
        }
      }

      if (low.get(v) === index.get(v)) {
        const group: Vertex[] = [];
        let top: Vertex;
        do {
          top = stack.pop()!;
          onStack.delete(top.node);
          group.push(top);
        } while (top.node !== v);
        out.push(group);
      }
    };

    for (const start of starts) if (!index.has(start.node)) connect(start);
    return out;
  }

  /**
   * Cycle issues, one per strongly connected component. Only components that hold a local node are reported:
   * a cycle wholly inside an ancestor belongs to that ancestor's issues.
   * With `mustInclude`, only components containing that node are reported, and the path starts there.
   */
  private cycleIssues(starts: readonly Vertex[], mustInclude?: DiNode): DiIssue[] {
    const issues: DiIssue[] = [];
    for (const group of this.components(starts)) {
      const origin = mustInclude
        ? group.find(v => v.node === mustInclude)
        : group.find(v => v.graph === this);
      if (!origin) continue;
      const path = this.cyclePath(origin, new Set(group.map(v => v.node)));
      if (path) issues.push(this.cycleIssue(origin.node.key, path));
    }
    return issues;
  }

  /** Shortest closed path from `origin` back to itself, staying inside the component. */
  private cyclePath(origin: Vertex, members: ReadonlySet<DiNode>): Vertex[] | undefined {
    const parent = new Map<DiNode, Vertex>();
    const seen = new Set<DiNode>([origin.node]);
    const queue: Vertex[] = [origin];
    for (let i = 0; i < queue.length; i++) {
      const current = queue[i]!;
      for (const next of this.successors(current)) {
        if (!members.has(next.node)) continue;
        if (next.node === origin.node) {
          const chain: Vertex[] = [current];
          while (chain[0]!.node !== origin.node) chain.unshift(parent.get(chain[0]!.node)!);
          return [...chain, origin];
        }
        if (seen.has(next.node)) continue;
        seen.add(next.node);
        parent.set(next.node, current);
        queue.push(next);
      }
    }
    return undefined;
  }

  /** Builds a cycle issue for a path that returns to its start. */
  private cycleIssue(at: symbol, path: readonly Vertex[]): DiIssue {
    return {
      kind: 'cycle', at, path: path.map(v => identityOf(v.node)),
      message: `Circular dependency: ${path.map(v => this.describe(v.node.token)).join(' → ')}`,
    };
  }
}
