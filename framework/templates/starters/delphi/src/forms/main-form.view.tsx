import { h } from '../../desktop';
import type { MainForm } from './main-form';

/** The form resource: layout, component properties, and event connections. */
export function mainFormView(form: MainForm) {
  return (
    <panel name="MainLayout" align="client" layout="column" gap={0}>
      <panel name="CommandBar" align="top" layout="row" gap={8} padding={12}>
        <button name="NewButton" caption="New project" onClick={form.newProject} />
        <button name="SaveButton" caption="Save" onClick={form.saveProject} />
        <button name="RevertButton" caption="Revert" onClick={form.revertProject} />
      </panel>
      <panel name="Workspace" align="client" layout="row" gap={20} padding={20}>
        <panel name="ProjectPane" layout="column" gap={10} width={250}>
          <label name="ProjectsLabel" caption="Projects" />
          <listbox name="ProjectList" caption="Projects" align="client" items={[]} onChange={form.selectProject} />
          <label name="CountLabel" caption="" />
        </panel>
        <panel name="EditorPane" align="client" layout="column" gap={10}>
          <label name="RecordLabel" caption="Project details" />
          <label name="NameLabel" caption="Project name" />
          <edit name="NameEdit" caption="Project name" text="" onChange={form.projectChanged} />
          <label name="OwnerLabel" caption="Owner" />
          <edit name="OwnerEdit" caption="Owner" text="" onChange={form.projectChanged} />
          <label name="StatusLabel" caption="Status" />
          <combobox name="StatusEdit" caption="Status" items={['Planned', 'In progress', 'Complete']} itemIndex={0} onChange={form.projectChanged} />
          <label name="NotesLabel" caption="Notes" />
          <memo name="NotesMemo" caption="Notes" align="client" text="" onChange={form.projectChanged} />
        </panel>
      </panel>
      <statusbar name="StatusBar" caption="Ready" />
    </panel>
  );
}
