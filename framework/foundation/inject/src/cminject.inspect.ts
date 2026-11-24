import type { DiIssue, IContainer, RuntimeDependencyGraph, RuntimeResourceGraph } from '@coldmind/inject/contracts';

/** A point-in-time view of a container: declared and observed dependencies, live resources, and graph problems. */
export interface InspectSnapshot {
  /** Bindings and the declared and observed dependencies between them. */
  dependencies: RuntimeDependencyGraph;
  /** Scopes, live instances and the runtime edges between them. */
  resources: RuntimeResourceGraph;
  /** Problems in the declared-dependency graph: missing, cycle, captive, dangling or decorator. */
  issues: DiIssue[];
  drift: DiIssue[];
}

/** Captures the inspection snapshot of a container. Pure read: it changes no registrations or instances. */
export function inspect(container: IContainer): InspectSnapshot {
  return {
    dependencies: container.getDependencyGraphSnapshot(),
    resources: container.getResourceGraphSnapshot(),
    issues: container.graph().issues(),
    drift: container.dependencyDrift(),
  };
}
