import { describe, expect, test } from 'bun:test';
import { Application, type DesktopHost } from '../../common';
import { CounterService } from '../../services/CounterService';
import { MyDash } from './MyDash.ts';
import { layout } from './MyDash.tsx';

const host: DesktopHost = { open() {}, close() {}, dispose() {} };

describe('MyDash view', () => {
  test('button events call the injected service and update persistent outlets', () => {
    const application = new Application(host);
    const view = application.run(MyDash, layout);
    const button = view.incrementButton;
    expect(view.countLabel.caption).toBe('0');
    expect(view.resetButton.enabled).toBe(false);
    button.click();
    button.click();
    expect(view.countLabel.caption).toBe('2');
    expect(application.services.get(CounterService).value).toBe(2);
    expect(view.incrementButton).toBe(button);
    expect(view.resetButton.enabled).toBe(true);
    application.dispose();
  });

  test('reset returns the service and controls to the initial state', () => {
    const application = new Application(host);
    const view = application.run(MyDash, layout);
    view.incrementButton.click();
    view.resetButton.click();
    expect(view.countLabel.caption).toBe('0');
    expect(view.status.text).toBe('Counter reset');
    expect(view.resetButton.enabled).toBe(false);
    application.dispose();
  });

  test('destroyed controls cannot invoke actions', () => {
    const application = new Application(host);
    const view = application.run(MyDash, layout);
    const button = view.incrementButton;
    const counter = application.services.get(CounterService);
    view.close();
    button.click();
    expect(counter.value).toBe(0);
    expect(view.disposed).toBe(true);
    application.dispose();
  });
});
