import { Application, Button, ComboBox, Edit, Form, Label, ListBox, Memo, StatusBar, type CloseQuery } from '../../desktop';
import { ProjectStore, projectStatuses, type Project } from '../models/project-store';
import { mainFormView } from './main-form.view';

/** Owns the form's interaction state; the view resource contains no business logic. */
export class MainForm extends Form {
  private readonly projectList: ListBox;
  private readonly nameEdit: Edit;
  private readonly ownerEdit: Edit;
  private readonly statusEdit: ComboBox;
  private readonly notesMemo: Memo;
  private readonly statusBar: StatusBar;
  private readonly saveButton: Button;
  private readonly revertButton: Button;
  private records: Project[] = [];
  private currentId = '';
  private dirty = false;

  constructor(application: Application, private readonly store: ProjectStore) {
    super(application, { name: 'MainForm', caption: 'Project Register', width: 960, height: 650 });
    this.load(mainFormView(this));
    this.projectList = this.find<ListBox>('ProjectList');
    this.nameEdit = this.find<Edit>('NameEdit');
    this.ownerEdit = this.find<Edit>('OwnerEdit');
    this.statusEdit = this.find<ComboBox>('StatusEdit');
    this.notesMemo = this.find<Memo>('NotesMemo');
    this.statusBar = this.find<StatusBar>('StatusBar');
    this.saveButton = this.find<Button>('SaveButton');
    this.revertButton = this.find<Button>('RevertButton');
    this.onCreate.subscribe(this.formCreated);
    this.onCloseQuery.subscribe(this.formClosing);
  }

  private readonly formCreated = (): void => {
    this.refreshProjects();
    this.openProject(this.records[0]!.id);
  };

  private readonly formClosing = (query: CloseQuery): void => {
    query.cancel = !this.allowNavigation();
  };

  readonly selectProject = (): void => {
    const project = this.records[this.projectList.itemIndex];
    if (!project || project.id === this.currentId) return;
    if (!this.allowNavigation()) return;
    this.openProject(project.id);
  };

  readonly projectChanged = (): void => {
    this.setDirty(true);
    this.statusBar.caption = 'Modified — save or revert before switching projects.';
  };

  readonly newProject = (): void => {
    if (!this.allowNavigation()) return;
    const project = this.store.create();
    this.refreshProjects();
    this.openProject(project.id);
    this.statusBar.caption = `${project.id} created. Enter the project details.`;
  };

  readonly saveProject = (): void => {
    try {
      this.store.save({
        id: this.currentId,
        name: this.nameEdit.text,
        owner: this.ownerEdit.text,
        status: projectStatuses[this.statusEdit.itemIndex] ?? 'Planned',
        notes: this.notesMemo.text,
      });
      this.refreshProjects();
      this.openProject(this.currentId);
      this.statusBar.caption = `${this.currentId} saved for this session.`;
    } catch (error) {
      this.statusBar.caption = error instanceof Error ? error.message : 'Unable to save the project.';
    }
  };

  readonly revertProject = (): void => {
    this.openProject(this.currentId);
    this.statusBar.caption = 'Unsaved changes reverted.';
  };

  private allowNavigation(): boolean {
    if (!this.dirty) return true;
    this.projectList.itemIndex = this.records.findIndex(project => project.id === this.currentId);
    this.statusBar.caption = 'Save or revert the current project before continuing.';
    return false;
  }

  private refreshProjects(): void {
    this.records = this.store.list();
    this.projectList.items = this.records.map(project => `${project.id}   ${project.name}`);
    this.find<Label>('CountLabel').caption = `${this.records.length} projects · session storage`;
  }

  private openProject(id: string): void {
    const project = this.store.read(id);
    this.currentId = id;
    this.projectList.itemIndex = this.records.findIndex(record => record.id === id);
    this.nameEdit.text = project.name;
    this.ownerEdit.text = project.owner;
    this.statusEdit.itemIndex = projectStatuses.indexOf(project.status);
    this.notesMemo.text = project.notes;
    this.find<Label>('RecordLabel').caption = `${project.id} · Project details`;
    this.setDirty(false);
    this.statusBar.caption = 'Ready';
  }

  private setDirty(value: boolean): void {
    this.dirty = value;
    this.saveButton.enabled = value;
    this.revertButton.enabled = value;
  }
}
