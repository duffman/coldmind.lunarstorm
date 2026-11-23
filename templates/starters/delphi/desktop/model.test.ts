import { describe, expect, test } from 'bun:test';
import { Application } from './application';
import { Form } from './form';
import { Button, Edit } from './controls';
import { h } from './jsx-runtime';
import type { DesktopHost } from './host';

function setup() {
  const log: string[] = [];
  const host: DesktopHost = {
    open: form => { log.push(`open:${form.name}`); },
    close: form => { log.push(`close:${form.name}`); },
    update: control => { log.push(`update:${control.name}`); },
    dispose: () => { log.push('host-dispose'); },
  };
  const app = new Application(host);
  const form = new Form(app, { name: 'Main', caption: 'Main' });
  return { app, form, log };
}

describe('retained desktop model', () => {
  test('loads once, retains control identity and runs lifecycle in host order', () => {
    const { app, form, log } = setup();
    form.load(h('panel', { name: 'Client' }, h('edit', { name: 'Title', text: 'Initial' })));
    const edit = form.find<Edit>('Title');
    form.onCreate.subscribe(() => { log.push('create'); });
    form.onShow.subscribe(() => { log.push('show'); });
    app.run(form); edit.text = 'Changed'; form.show();
    expect(form.find('Title')).toBe(edit);
    expect(log).toEqual(['create', 'open:Main', 'show', 'update:Title']);
    expect(() => form.load(h('label', { name: 'Other' }))).toThrow('once');
  });
  test('close query can veto; successful close destroys children before owner', () => {
    const { app, form, log } = setup();
    form.load(h('button', { name: 'Save', caption: 'Save' }));
    const button = form.find<Button>('Save');
    const removeVeto = form.onCloseQuery.subscribe(query => { query.cancel = true; });
    button.onDestroy.subscribe(() => { log.push('destroy:button'); });
    form.onDestroy.subscribe(() => { log.push('destroy:form'); });
    app.run(form);
    expect(form.close()).toBe(false); expect(button.disposed).toBe(false);
    removeVeto(); expect(form.close()).toBe(true);
    expect(log).toEqual(['open:Main', 'close:Main', 'destroy:button', 'destroy:form']);
    expect(app.components).toHaveLength(0);
  });
  test('nested events queue after current handler and disposal prevents delivery', () => {
    const { app, form } = setup(); const seen: string[] = [];
    form.load(h('panel', { name: 'Client' }, h('button', { name: 'First' }), h('button', { name: 'Second' })));
    const first = form.find<Button>('First'); const second = form.find<Button>('Second');
    first.onClick.subscribe(() => { seen.push('first:start'); second.click(); seen.push('first:end'); });
    second.onClick.subscribe(() => { seen.push('second'); });
    first.click(); expect(seen).toEqual(['first:start', 'first:end', 'second']);
    app.dispose(); second.click(); expect(seen).toHaveLength(3);
  });
  test('duplicate form control names reject the tree and release partial components', () => {
    const { form } = setup();
    expect(() => form.load(h('panel', { name: 'Client' }, h('edit', { name: 'Same' }), h('button', { name: 'Same' })))).toThrow('unique');
    expect(form.components).toHaveLength(0);
  });
  test('disabled and readonly input cannot mutate state or emit an event', () => {
    const { form } = setup(); let changes = 0;
    form.load(h('edit', { name: 'Title', text: 'Locked', readOnly: true, onChange: () => { changes++; } }));
    const edit = form.find<Edit>('Title'); edit.input('Changed');
    expect(edit.text).toBe('Locked'); expect(changes).toBe(0);
  });
  test('a failed handler clears queued work and later events can still run', () => {
    const { app } = setup(); const seen: string[] = [];
    expect(() => app.dispatch(() => {
      app.dispatch(() => { seen.push('stale'); });
      throw new Error('handler failed');
    })).toThrow('handler failed');
    app.dispatch(() => { seen.push('fresh'); });
    expect(seen).toEqual(['fresh']);
  });
  test('teardown releases siblings and host even when a destroy handler fails', () => {
    const { app, form, log } = setup();
    form.load(h('panel', { name: 'Client' }, h('button', { name: 'First' }), h('button', { name: 'Second' })));
    const first = form.find<Button>('First'); const second = form.find<Button>('Second');
    second.onDestroy.subscribe(() => { throw new Error('destroy failed'); });
    expect(() => app.dispose()).toThrow('Application teardown failed');
    expect(first.disposed).toBe(true); expect(second.disposed).toBe(true);
    expect(form.disposed).toBe(true); expect(app.components).toHaveLength(0);
    expect(log.at(-1)).toBe('host-dispose');
  });

});
