import { describe, expect, test } from 'bun:test';
import { Application, Button, Edit, ListBox, StatusBar, type DesktopHost } from '../../desktop';
import { ProjectStore } from '../models/project-store';
import { MainForm } from './main-form';

function openRegister() {
  const host: DesktopHost = { open() {}, update() {}, close() {}, dispose() {} };
  const application = new Application(host);
  const store = new ProjectStore();
  const form = new MainForm(application, store);
  application.run(form);
  return { application, store, form };
}

describe('Project Register form', () => {
  test('control events edit drafts and Save commits the record', () => {
    const { application, form, store } = openRegister();
    form.find<Edit>('NameEdit').input('Component toolkit');
    expect(store.read('P-001').name).toBe('Desktop component library');
    expect(form.find<Button>('SaveButton').enabled).toBe(true);
    form.find<Button>('SaveButton').click();
    expect(store.read('P-001').name).toBe('Component toolkit');
    expect(form.find<Button>('SaveButton').enabled).toBe(false);
    application.dispose();
  });

  test('unsaved edits block selection and close until Revert', () => {
    const { application, form } = openRegister();
    form.find<Edit>('NameEdit').input('Unsaved draft');
    form.find<ListBox>('ProjectList').select(1);
    expect(form.find<ListBox>('ProjectList').itemIndex).toBe(0);
    expect(form.close()).toBe(false);
    form.find<Button>('RevertButton').click();
    expect(form.find<Edit>('NameEdit').text).toBe('Desktop component library');
    form.find<ListBox>('ProjectList').select(1);
    expect(form.find<Edit>('NameEdit').text).toBe('Customer directory');
    expect(form.close()).toBe(true);
    application.dispose();
  });

  test('invalid Save keeps the draft open with a validation message', () => {
    const { application, form, store } = openRegister();
    form.find<Edit>('NameEdit').input(' ');
    form.find<Button>('SaveButton').click();
    expect(form.find<StatusBar>('StatusBar').caption).toBe('Project name is required.');
    expect(form.find<Button>('SaveButton').enabled).toBe(true);
    expect(store.read('P-001').name).toBe('Desktop component library');
    application.dispose();
  });
});
