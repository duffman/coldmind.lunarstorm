import { Button, h, Label, Panel, StatusBar, WindowLayout, type LSNode } from '../../common';
import type { MyDash } from './MyDash.ts';

/** Paired by the environment with MyDash.ts; connections use symbols. */
export function layout(view: MyDash): LSNode {
  return (
    <WindowLayout>
      <Panel className="counter-heading">
        <Label caption="DESKTOP TEMPLATE" className="eyebrow" />
        <Label caption="A simple counter" className="heading" />
        <Label caption="A window, two actions, and an application service." />
      </Panel>
      <Panel className="result-panel">
        <Label caption="CURRENT COUNT" className="eyebrow" />
        <Label outlet={view.countLabel} caption="0" className="count-value" />
      </Panel>
      <Panel className="counter-actions">
        <Button outlet={view.incrementButton} caption="Increase" onClick={view.increment} />
        <Button outlet={view.resetButton} caption="Reset" onClick={view.reset} />
      </Panel>
      <StatusBar outlet={view.status} />
    </WindowLayout>
  );
}
