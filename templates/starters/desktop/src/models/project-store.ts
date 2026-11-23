export const projectStatuses = ['Planned', 'In progress', 'Complete'] as const;
export type ProjectStatus = typeof projectStatuses[number];

export interface Project {
  id: string;
  name: string;
  owner: string;
  status: ProjectStatus;
  notes: string;
}

const initialProjects: readonly Project[] = [
  {
    id: 'P-001',
    name: 'Desktop component library',
    owner: 'Alex Morgan',
    status: 'In progress',
    notes: 'Define the form lifecycle, control ownership, and typed event contracts.',
  },
  {
    id: 'P-002',
    name: 'Customer directory',
    owner: 'Sam Chen',
    status: 'Planned',
    notes: 'A small data-entry application using the shared desktop controls.',
  },
  {
    id: 'P-003',
    name: 'Release checklist',
    owner: 'Jordan Lee',
    status: 'Complete',
    notes: 'Review packaging, keyboard navigation, and application shutdown.',
  },
];

/** Session-local storage. Callers edit detached records and explicitly save. */
export class ProjectStore {
  private readonly records = new Map(initialProjects.map(project => [project.id, { ...project }]));
  private nextNumber = initialProjects.length + 1;

  list(): Project[] {
    return [...this.records.values()].map(project => ({ ...project }));
  }

  read(id: string): Project {
    const project = this.records.get(id);
    if (!project) throw new Error(`Project ${id} does not exist.`);
    return { ...project };
  }

  save(project: Project): void {
    const name = project.name.trim();
    if (!name) throw new Error('Project name is required.');
    if (!this.records.has(project.id)) throw new Error(`Project ${project.id} does not exist.`);
    this.records.set(project.id, { ...project, name, owner: project.owner.trim() });
  }

  create(): Project {
    const project: Project = {
      id: `P-${String(this.nextNumber++).padStart(3, '0')}`,
      name: 'Untitled project',
      owner: '',
      status: 'Planned',
      notes: '',
    };
    this.records.set(project.id, project);
    return { ...project };
  }
}
