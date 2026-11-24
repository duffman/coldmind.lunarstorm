import type { MainForm } from './main-form';

/** The form class this resource connects its events to; a type only. */
declare const form: MainForm;

/**
 * The main form's view resource (spec 2.1): controls, their properties and their
 * event connections. The compiler reads it from the AST and never executes it.
 */
export default (
  <form
    name="MainForm"
    caption="Project Register"
    width={960}
    height={650}
    minWidth={640}
    minHeight={420}
    position="screenCenter"
    menu="MainMenu"
    onCreate={form.formCreated}
    onCloseQuery={form.formClosing}
  >
    <mainmenu name="MainMenu">
      <menuitem name="FileMenu" caption="File">
        <menuitem name="NewItem" caption="New project" shortcut="CmdOrCtrl+N" onClick={form.newProject} />
        <menuitem name="SaveItem" caption="Save" shortcut="CmdOrCtrl+S" enabled={false} onClick={form.saveProject} />
        <menuitem name="FileSeparator" caption="-" />
        <menuitem name="QuitItem" role="quit" />
      </menuitem>
      <menuitem name="EditMenu" caption="Edit">
        <menuitem name="UndoItem" role="undo" />
        <menuitem name="RedoItem" role="redo" />
        <menuitem name="EditSeparator" caption="-" />
        <menuitem name="CutItem" role="cut" />
        <menuitem name="CopyItem" role="copy" />
        <menuitem name="PasteItem" role="paste" />
        <menuitem name="SelectAllItem" role="selectAll" />
      </menuitem>
    </mainmenu>
    <panel name="CommandBar" align="top" layout="row" gap={8} padding={12}>
      <button name="NewButton" caption="New project" onClick={form.newProject} />
      <button name="SaveButton" caption="Save" default enabled={false} onClick={form.saveProject} />
      <button name="RevertButton" caption="Revert" enabled={false} onClick={form.revertProject} />
    </panel>
    <panel name="Workspace" align="client" layout="row" gap={20} padding={20}>
      <panel name="ProjectPane" layout="column" gap={10} width={250}>
        <label name="ProjectsLabel" caption="Projects" />
        <listbox name="ProjectList" align="client" items={[]} itemIndex={-1} onChange={form.selectProject} />
        <label name="CountLabel" caption="" />
      </panel>
      <panel name="EditorPane" align="client" layout="column" gap={10}>
        <label name="RecordLabel" caption="Project details" />
        <label name="NameLabel" caption="Project name" />
        <edit name="NameEdit" text="" onChange={form.projectChanged} />
        <label name="OwnerLabel" caption="Owner" />
        <edit name="OwnerEdit" text="" onChange={form.projectChanged} />
        <label name="StatusLabel" caption="Status" />
        <combobox
          name="StatusEdit"
          items={['Planned', 'In progress', 'Complete']}
          itemIndex={0}
          onChange={form.projectChanged}
        />
        <label name="NotesLabel" caption="Notes" />
        <memo name="NotesMemo" align="client" text="" onChange={form.projectChanged} />
      </panel>
    </panel>
    <statusbar name="StatusBar" caption="Ready" />
  </form>
);
