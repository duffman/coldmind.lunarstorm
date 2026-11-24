// Generated form tree of MainForm (stands in for 4b output): literal props only.
export default {
  name: 'MainForm',
  tree: ['panel', { name: 'Root', align: 'client', layout: 'column' }, [
    ['panel', { name: 'CommandBar', align: 'top', layout: 'row', gap: 8, padding: 10 }, [
      ['button', { name: 'SaveButton', caption: 'Save', enabled: false }],
      ['button', { name: 'RevertButton', caption: 'Revert', enabled: false }],
      ['label', { name: 'ClockLabel', caption: '', align: 'right' }],
    ]],
    ['panel', { name: 'Workspace', align: 'client', layout: 'row', gap: 16, padding: 16 }, [
      ['listbox', { name: 'ProjectList', width: 260, items: [], itemIndex: -1 }],
      ['panel', { name: 'Editor', align: 'client', layout: 'column', gap: 8 }, [
        ['label', { name: 'NameLabel', caption: 'Project name' }],
        ['edit', { name: 'NameEdit', text: '', placeholder: 'Name' }],
      ]],
    ]],
    ['statusbar', { name: 'StatusBar', caption: 'Ready', align: 'bottom' }],
  ]],
};
