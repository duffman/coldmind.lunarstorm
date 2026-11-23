import {
  Form,
  type Button,
  type CloseQuery,
  type ComboBox,
  type Edit,
  type Label,
  type ListBox,
  type Memo,
  type MenuItem,
  type StatusBar,
} from '@desktop/forms';
import { ProjectStore, projectStatuses, type Project } from '../models/project-store';

/**
 * The main form's class (spec 2.5): the handlers its view resource names, and the
 * form's logic. It runs on the backend; writing a control property updates the
 * screen.
 */
export class MainForm extends Form {
  /** The list of projects on the left. */
  private readonly projectList = this.find<ListBox>('ProjectList');
  /** The project name field. */
  private readonly nameEdit = this.find<Edit>('NameEdit');
  /** The owner field. */
  private readonly ownerEdit = this.find<Edit>('OwnerEdit');
  /** The status selector. */
  private readonly statusEdit = this.find<ComboBox>('StatusEdit');
  /** The notes field. */
  private readonly notesMemo = this.find<Memo>('NotesMemo');
  /** The status line at the bottom. */
  private readonly statusBar = this.find<StatusBar>('StatusBar');
  /** The Save button; enabled only while there are unsaved changes. */
  private readonly saveButton = this.find<Button>('SaveButton');
  /** The Revert button; enabled only while there are unsaved changes. */
  private readonly revertButton = this.find<Button>('RevertButton');
  /** The Save menu item; follows the Save button. */
  private readonly saveItem = this.find<MenuItem>('SaveItem');
  /** The session's project records. */
  private readonly store = new ProjectStore();
  /** The records as last listed, in list order. */
  private records: Project[] = [];
  /** The id of the project being edited. */
  private currentId = '';
  /** Whether the editor holds unsaved changes. */
  private dirty = false;

  /** `onCreate`: fills the list and opens the first project. */
  readonly formCreated = (): void => {
    this.refreshProjects();
    this.openProject(this.records[0]!.id);
  };

  /**
   * `onCloseQuery`: refuses to close while there are unsaved changes.
   * @param query set `cancel` to keep the form open
   */
  readonly formClosing = (query: CloseQuery): void => {
    query.cancel = !this.allowNavigation();
  };

  /** `ProjectList.onChange`: opens the selected project unless the editor is dirty. */
  readonly selectProject = (): void => {
    const project = this.records[this.projectList.itemIndex];
    if (!project || project.id === this.currentId) return;
    if (!this.allowNavigation()) return;
    this.openProject(project.id);
  };

  /** `onChange` of every editor field: marks the editor dirty. */
  readonly projectChanged = (): void => {
    this.setDirty(true);
    this.statusBar.caption = 'Modified — save or revert before switching projects.';
  };

  /** New project: creates a record and opens it. */
  readonly newProject = (): void => {
    if (!this.allowNavigation()) return;
    const project = this.store.create();
    this.refreshProjects();
    this.openProject(project.id);
    this.statusBar.caption = `${project.id} created. Enter the project details.`;
  };

  /** Save: writes the editor's fields to the store. */
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

  /** Revert: reloads the current project from the store. */
  readonly revertProject = (): void => {
    this.openProject(this.currentId);
    this.statusBar.caption = 'Unsaved changes reverted.';
  };

  /**
   * Decides whether the editor may leave the current project.
   * @return `false` while there are unsaved changes; the list selection is put back
   */
  private allowNavigation(): boolean {
    if (!this.dirty) return true;
    this.projectList.itemIndex = this.records.findIndex(project => project.id === this.currentId);
    this.statusBar.caption = 'Save or revert the current project before continuing.';
    return false;
  }

  /** Reloads the list from the store. */
  private refreshProjects(): void {
    this.records = this.store.list();
    this.projectList.items = this.records.map(project => `${project.id}   ${project.name}`);
    this.find<Label>('CountLabel').caption = `${this.records.length} projects · session storage`;
  }

  /**
   * Shows a project in the editor.
   * @param id the project's id
   */
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

  /**
   * Records whether the editor is dirty and enables Save and Revert to match.
   * @param value the new state
   */
  private setDirty(value: boolean): void {
    this.dirty = value;
    this.saveButton.enabled = value;
    this.revertButton.enabled = value;
    this.saveItem.enabled = value;
  }
}
