// Backend artifact of the example app. It stands in for what 4c generates: control
// proxies and the dispatch table (top half) bound to the developer's form class (bottom).
import { serve, setProperty, restored } from '@desktop/bridge';
import { native } from '@desktop/api';

// ---------------------------------------------------------------- generated (4c)

/** A control as the form class sees it; writes go to the form's WebView. */
class ControlProxy {
  /**
   * @param {Form} form owning form
   * @param {string} name control name
   * @param {Record<string, unknown>} values initial values from the view resource
   */
  constructor(form, name, values) {
    this.form = form;
    this.name = name;
    this.values = values;
  }
  /** @param {Record<string, unknown>} props values a WebView event changed */
  receive(props) {
    Object.assign(this.values, props);
  }
}

/**
 * Defines typed accessors on a proxy class.
 * @param {Function} cls proxy class
 * @param {string[]} props property names
 */
const accessors = (cls, props) => {
  for (const prop of props) {
    Object.defineProperty(cls.prototype, prop, {
      get() { return this.values[prop]; },
      set(value) {
        this.values[prop] = value;
        setProperty(this.form.name, this.name, prop, value);
      },
    });
  }
};
class Panel extends ControlProxy {}
class Button extends ControlProxy {}
class Edit extends ControlProxy {}
class ListBox extends ControlProxy {}
class Label extends ControlProxy {}
class StatusBar extends ControlProxy {}
class MenuItem extends ControlProxy {}
class Timer extends ControlProxy {}
accessors(Panel, ['visible']);
accessors(Button, ['caption', 'enabled', 'visible']);
accessors(Edit, ['text', 'placeholder', 'enabled', 'visible']);
accessors(ListBox, ['items', 'itemIndex', 'enabled', 'visible']);
accessors(Label, ['caption', 'visible']);
accessors(StatusBar, ['caption', 'visible']);
accessors(MenuItem, ['caption', 'enabled', 'checked']);
accessors(Timer, ['interval', 'enabled']);

/** Base of every form class. */
class Form {
  /**
   * @param {string} name form name
   * @param {Record<string, [Function, Record<string, unknown>]>} controls proxies with view values
   */
  constructor(name, controls) {
    this.name = name;
    this.controls = new Map();
    const carried = restored(name);
    for (const [control, [Proxy, values]] of Object.entries(controls)) {
      this.controls.set(control, new Proxy(this, control, { ...values, ...(carried[control] ?? {}) }));
    }
  }
  /**
   * @param {string} name control name
   * @returns {ControlProxy}
   */
  find(name) {
    const control = this.controls.get(name);
    if (!control) throw new Error(`Unknown control: ${name}`);
    return control;
  }
}

const mainFormControls = {
  SaveButton: [Button, { caption: 'Save', enabled: false, visible: true }],
  RevertButton: [Button, { caption: 'Revert', enabled: false, visible: true }],
  ProjectList: [ListBox, { items: [], itemIndex: -1, enabled: true, visible: true }],
  NameLabel: [Label, { caption: 'Project name', visible: true }],
  NameEdit: [Edit, { text: '', placeholder: 'Name', enabled: true, visible: true }],
  ClockLabel: [Label, { caption: '', visible: true }],
  StatusBar: [StatusBar, { caption: 'Ready', visible: true }],
  SaveItem: [MenuItem, { caption: 'Save', enabled: false, checked: false }],
  ClockTimer: [Timer, { interval: 1000, enabled: true }],
};

const mainFormConnections = {
  'MainForm.onCreate': 'formCreated',
  'MainForm.onCloseQuery': 'formClosing',
  'MainForm.onResize': 'formResized',
  'SaveButton.onClick': 'saveProject',
  'SaveItem.onClick': 'saveProject',
  'RevertButton.onClick': 'revertProject',
  'NameEdit.onChange': 'nameChanged',
  'ProjectList.onChange': 'selectProject',
  'ClockTimer.onTimer': 'tick',
};

// ---------------------------------------------------------------- form class (developer code)

class MainForm extends Form {
  constructor() {
    super('MainForm', mainFormControls);
    this.records = ['Desktop component library', 'Customer directory', 'Release checklist'];
    this.current = 0;
  }

  async formCreated() {
    this.showRecords();
    this.open(this.current);
    const info = await native.os.info();
    this.find('StatusBar').caption = `Ready · ${info.platform}/${info.arch}`;
  }

  formClosing() {
    if (!this.dirty) return { cancel: false };
    this.find('StatusBar').caption = 'Save or revert before closing.';
    return { cancel: true };
  }

  formResized(size) {
    this.find('ClockLabel').visible = size.width > 520;
  }

  nameChanged() {
    this.setDirty(true);
    this.find('StatusBar').caption = 'Modified';
  }

  selectProject() {
    const index = this.find('ProjectList').itemIndex;
    if (this.dirty) {
      this.find('ProjectList').itemIndex = this.current;
      this.find('StatusBar').caption = 'Save or revert before switching projects.';
      return;
    }
    this.open(index);
  }

  saveProject() {
    const name = String(this.find('NameEdit').text).trim();
    if (!name) {
      this.find('StatusBar').caption = 'Project name is required.';
      return;
    }
    this.records[this.current] = name;
    this.showRecords();
    this.setDirty(false);
    this.find('StatusBar').caption = `Saved “${name}”.`;
  }

  revertProject() {
    this.open(this.current);
  }

  tick() {
    const now = new Date();
    const pad = (n) => String(n).padStart(2, '0');
    this.find('ClockLabel').caption = `${pad(now.getHours())}:${pad(now.getMinutes())}:${pad(now.getSeconds())}`;
  }

  showRecords() {
    const list = this.find('ProjectList');
    list.items = this.records.map((name, i) => `P-00${i + 1}   ${name}`);
    list.itemIndex = this.current;
  }

  open(index) {
    this.current = index;
    this.find('ProjectList').itemIndex = index;
    this.find('NameEdit').text = this.records[index];
    this.setDirty(false);
    this.find('StatusBar').caption = 'Ready';
  }

  setDirty(value) {
    this.dirty = value;
    this.find('SaveButton').enabled = value;
    this.find('RevertButton').enabled = value;
    this.find('SaveItem').enabled = value;
  }
}

// ---------------------------------------------------------------- generated dispatch (4c)

const forms = { MainForm: new MainForm() };
const connections = { MainForm: mainFormConnections };

serve((message) => {
  const form = forms[message.form];
  if (!form) return undefined;
  const props = message.payload && message.payload.props;
  if (props && message.target !== message.form) form.find(message.target).receive(props);
  const handler = connections[message.form][`${message.target}.${message.event}`];
  if (!handler) return undefined;
  return form[handler](message.payload && message.payload.args !== undefined ? message.payload.args : message.payload);
});
