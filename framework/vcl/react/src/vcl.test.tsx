/**
 * Tests of the React VCL: the property mirror, and what each control renders
 * from its literal props and from mirrored values.
 *
 * Rendering is checked through `react-dom/server`, which needs no DOM. What a
 * user event does is checked by calling the rendered element's handler through
 * the mirror-facing seam (`Mirror.emit`), and end to end in the WebView.
 */
import { describe, expect, test } from 'bun:test';
import type { ReactElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import {
  Button, CheckBox, COMPONENTS, Column, ComboBox, Edit, Form, GroupBox, Image, Label, ListBox, ListView, Memo, Mirror,
  MirrorContext, PageControl, Panel, ProgressBar, RadioGroup, ScrollBox, Shell, ShellDebug, Splitter, StatusBar, TabSheet,
  ToolBar, TrackBar, TreeView, type Bridge,
} from './index';

/** A bridge that records what the VCL sends and lets a test push updates. */
interface FakeBridge extends Bridge {
  /** Every event sent, as `[control, event, props, args]`. */
  sent: unknown[][];
  /**
   * Delivers a property update, as the host would.
   * @param controls control name to changed values
   */
  push(controls: Record<string, Record<string, unknown>>): void;
}

/**
 * Creates a fake bridge.
 * @param state the values the host holds when the form mounts
 * @param windowId the form this WebView shows
 * @return the bridge
 */
function bridge(state: Record<string, Record<string, unknown>> = {}, windowId = 'Main'): FakeBridge {
  const listeners = new Set<(controls: Record<string, Record<string, unknown>>) => void>();
  const sent: unknown[][] = [];
  return {
    windowId,
    mode: 'debug',
    state,
    sent,
    emit: (...message) => void sent.push(message),
    onUpdate: listener => (listeners.add(listener), () => void listeners.delete(listener)),
    onRefresh: () => () => {},
    push: controls => listeners.forEach(listener => listener(controls)),
  };
}

/**
 * Renders controls inside a mirror.
 * @param element the controls
 * @param state mirrored values, by control name
 * @return the markup
 */
function render(element: ReactElement, state: Record<string, Record<string, unknown>> = {}): string {
  return renderToStaticMarkup(<MirrorContext.Provider value={new Mirror(bridge(state))}>{element}</MirrorContext.Provider>);
}

describe('mirror', () => {
  test('starts from the host state and applies updates per control', () => {
    const host = bridge({ Save: { caption: 'Save', enabled: false } });
    const mirror = new Mirror(host);
    const before = mirror.values('Save');
    expect(before).toEqual({ caption: 'Save', enabled: false });
    expect(mirror.values('Other')).toEqual({});

    const calls: string[] = [];
    mirror.subscribe('Save', () => calls.push('Save'));
    mirror.subscribe('Other', () => calls.push('Other'));
    host.push({ Save: { enabled: true } });
    expect(mirror.values('Save')).toEqual({ caption: 'Save', enabled: true });
    expect(mirror.values('Save')).not.toBe(before);
    expect(calls).toEqual(['Save']);
  });

  test('an update that changes nothing keeps the values and notifies nobody', () => {
    const host = bridge({ Name: { text: 'abc' } });
    const mirror = new Mirror(host);
    const before = mirror.values('Name');
    let calls = 0;
    mirror.subscribe('Name', () => calls++);
    host.push({ Name: { text: 'abc' } });
    expect(mirror.values('Name')).toBe(before);
    expect(calls).toBe(0);
  });

  test('an event mirrors its changed values first, then sends them', () => {
    const host = bridge();
    const mirror = new Mirror(host);
    const seen: unknown[] = [];
    mirror.subscribe('Name', () => seen.push(mirror.values('Name')));
    mirror.emit('Name', 'onChange', { text: 'x' });
    mirror.emit('Save', 'onClick');
    mirror.emit('Tree', 'onExpanded', undefined, { id: 'a', expanded: true });
    expect(seen).toEqual([{ text: 'x' }]);
    expect(host.sent).toEqual([
      ['Name', 'onChange', { text: 'x' }, undefined],
      ['Save', 'onClick', undefined, undefined],
      ['Tree', 'onExpanded', undefined, { id: 'a', expanded: true }],
    ]);
  });

  test('unsubscribe and dispose stop notifications', () => {
    const host = bridge();
    const mirror = new Mirror(host);
    let calls = 0;
    const off = mirror.subscribe('A', () => calls++);
    host.push({ A: { caption: '1' } });
    off();
    host.push({ A: { caption: '2' } });
    expect(calls).toBe(1);
    mirror.subscribe('A', () => calls++);
    mirror.dispose();
    host.push({ A: { caption: '3' } });
    expect(calls).toBe(1);
    expect(mirror.values('A')).toEqual({ caption: '2' });
  });
});

describe('common props', () => {
  test('align, size, hint, visibility and enabled become attributes', () => {
    const html = render(<Label name="L" caption="Hi" align="top" width={120} height={20} hint="tip" />);
    expect(html).toBe('<span class="vcl-label vcl-align-top" style="width:120px;height:20px" data-name="L" title="tip">Hi</span>');
    expect(render(<Label name="L" caption="Hi" visible={false} />)).toContain('hidden=""');
    expect(render(<Label name="L" caption="Hi" enabled={false} />)).toContain('class="vcl-label vcl-align-none vcl-disabled"');
  });

  test('mirrored values override the literal props', () => {
    const html = render(<Label name="L" caption="Literal" />, { L: { caption: 'Live', visible: false } });
    expect(html).toContain('>Live</span>');
    expect(html).toContain('hidden=""');
  });

  test('a control outside a form is an error, not a blank', () => {
    expect(() => renderToStaticMarkup(<Label name="L" />)).toThrow('<L> is rendered outside a form');
  });
});

describe('containers', () => {
  test('form is the client area and takes no mirror values', () => {
    expect(render(<Form name="Main" cursor="wait"><Label name="L" caption="x" /></Form>)).toBe(
      '<div class="vcl-form" data-name="Main" data-layout="column" style="cursor:wait"><span class="vcl-label vcl-align-none" data-name="L">x</span></div>',
    );
  });

  test('panel, scrollbox and groupbox lay their children out', () => {
    expect(render(<Panel name="P" layout="row" gap={8} padding={12} align="client" />)).toBe(
      '<div class="vcl-panel vcl-align-client" style="gap:8px;padding:12px" data-name="P" data-layout="row"></div>',
    );
    expect(render(<ScrollBox name="S" />)).toContain('class="vcl-scrollbox vcl-align-none"');
    const group = render(<GroupBox name="G" caption="Options" enabled={false}><Label name="L" caption="x" /></GroupBox>);
    expect(group).toContain('<fieldset class="vcl-groupbox vcl-align-none vcl-disabled"');
    expect(group).toContain('disabled=""');
    expect(group).toContain('<legend>Options</legend>');
    expect(group).toContain('<div class="vcl-groupbox-body" data-layout="column"');
  });

  test('pagecontrol shows the active page and one tab per sheet', () => {
    const pages = (
      <PageControl name="Pages" activePage="Two">
        <TabSheet name="One" caption="First"><Label name="A" caption="a" /></TabSheet>
        <TabSheet name="Two" caption="Second"><Label name="B" caption="b" /></TabSheet>
      </PageControl>
    );
    const html = render(pages);
    expect(html).toContain('<button type="button" role="tab" class="vcl-tab" aria-selected="false">First</button>');
    expect(html).toContain('<button type="button" role="tab" class="vcl-tab vcl-tab-active" aria-selected="true">Second</button>');
    expect(html).toContain('data-name="One" hidden="" role="tabpanel"');
    expect(html).not.toContain('data-name="Two" hidden=""');

    const live = render(pages, { Pages: { activePage: 'One' }, Two: { caption: 'Renamed' } });
    expect(live).toContain('aria-selected="true">First</button>');
    expect(live).toContain('>Renamed</button>');
    expect(live).toContain('data-name="Two" hidden="" role="tabpanel"');
    expect(live).not.toContain('data-name="One" hidden=""');
  });

  test('the first sheet is active when activePage is absent', () => {
    const html = render(<PageControl name="Pages"><TabSheet name="One" /><TabSheet name="Two" /></PageControl>);
    expect(html).toContain('aria-selected="true">One</button>');
  });

  test('splitter, toolbar and statusbar', () => {
    expect(render(<Splitter name="S" />)).toContain('role="separator"');
    expect(render(<ToolBar name="T"><Button name="B" caption="Go" /></ToolBar>)).toContain(
      '<div class="vcl-toolbar vcl-align-top" data-name="T" role="toolbar" data-layout="row"><button',
    );
    expect(render(<StatusBar name="S" caption="Ready" />)).toBe(
      '<div class="vcl-statusbar vcl-align-bottom" data-name="S" role="status">Ready</div>',
    );
  });
});

describe('display and input', () => {
  test('image and progressbar', () => {
    expect(render(<Image name="I" src="res/a.png" caption="Logo" stretch proportional />)).toContain(
      'style="object-fit:contain" data-name="I" src="res/a.png" alt="Logo"',
    );
    expect(render(<Image name="I" src="a.png" />)).toContain('object-fit:none');
    expect(render(<ProgressBar name="P" position={30} min={10} max={60} />)).toContain('value="20" max="50"');
  });

  test('button', () => {
    expect(render(<Button name="B" caption="Save" default enabled={false} tabOrder={2} />)).toBe(
      '<button class="vcl-button vcl-align-none vcl-disabled vcl-button-default" data-name="B" type="button" disabled="" tabindex="2" data-default="true">Save</button>',
    );
    expect(render(<Button name="B" caption="Close" cancel />)).toContain('data-cancel="true"');
    expect(render(<Button name="B" caption="Save" enabled={false} />, { B: { enabled: true } })).not.toContain('disabled');
  });

  test('edit and memo show the mirrored text', () => {
    const edit = render(<Edit name="E" text="" placeholder="Name" caption="Project name" />, { E: { text: 'Zebra' } });
    expect(edit).toContain('type="text" placeholder="Name"');
    expect(edit).toContain('value="Zebra"');
    expect(edit).toContain('aria-label="Project name"');
    const secret = render(<Edit name="E" password readOnly />);
    expect(secret).toContain('type="password"');
    expect(secret).toContain('readOnly=""');
    const memo = render(<Memo name="M" text="a&#10;b" wordWrap={false} />);
    expect(memo).toContain('<textarea class="vcl-memo vcl-align-none"');
    expect(memo).toContain('wrap="off"');
  });

  test('checkbox, radiogroup, combobox and trackbar', () => {
    expect(render(<CheckBox name="C" caption="Done" checked />)).toContain('<input type="checkbox" checked=""/><span>Done</span>');
    const radio = render(<RadioGroup name="R" caption="Size" items={['S', 'M']} itemIndex={1} />);
    expect(radio).toContain('<legend>Size</legend>');
    expect(radio).toContain('<input type="radio" name="R"/><span>S</span>');
    expect(radio).toContain('<input type="radio" name="R" checked=""/><span>M</span>');
    const combo = render(<ComboBox name="C" items={['Planned', 'Done']} itemIndex={0} />, { C: { itemIndex: 1 } });
    expect(combo).toContain('<option value="0">Planned</option><option value="1" selected="">Done</option>');
    expect(render(<TrackBar name="T" position={5} max={10} />)).toContain('type="range" min="0" max="10" value="5"');
  });
});

describe('lists', () => {
  test('listbox marks the selected item', () => {
    const html = render(<ListBox name="L" items={[]} itemIndex={-1} />, { L: { items: ['a', 'b'], itemIndex: 1 } });
    expect(html).toContain('<li role="option" aria-selected="false" class="vcl-item">a</li>');
    expect(html).toContain('<li role="option" aria-selected="true" class="vcl-item vcl-item-selected">b</li>');
    expect(render(<ListBox name="L" items={['a']} enabled={false} />)).toContain('tabindex="-1"');
  });

  test('listview draws its columns and the rows from the mirror', () => {
    const view = (
      <ListView name="V" itemIndex={0}>
        <Column name="Id" caption="Id" width={60} />
        <Column name="Name" caption="Name" alignment="right" />
      </ListView>
    );
    expect(render(view)).toContain('<th style="width:60px">Id</th><th style="text-align:right">Name</th>');
    const html = render(view, { V: { rows: [['1', 'One'], ['2']] } });
    expect(html).toContain('<tr aria-selected="true" class="vcl-item vcl-item-selected"><td>1</td><td style="text-align:right">One</td></tr>');
    expect(html).toContain('<tr aria-selected="false" class="vcl-item"><td>2</td><td style="text-align:right"></td></tr>');
    expect(renderToStaticMarkup(<Column name="X" />)).toBe('');
  });

  test('treeview shows the top level closed and marks the selected node', () => {
    const nodes = [{ id: 'a', caption: 'A', children: [{ id: 'a1', caption: 'A1' }] }, { id: 'b', caption: 'B' }];
    const html = render(<TreeView name="T" />, { T: { nodes, selected: 'b' } });
    expect(html).toContain('<li role="treeitem" aria-selected="false" aria-expanded="false">');
    expect(html).toContain('aria-label="Expand">▸</button>A</div>');
    expect(html).toContain('<div class="vcl-item vcl-item-selected"><span class="vcl-tree-toggle"></span>B</div>');
    expect(html).not.toContain('A1');
    expect(render(<TreeView name="T" />)).toContain('role="tree"');
  });
});

describe('shell', () => {
  /** A generated form, as 4b emits it. */
  const Main = (): ReactElement => <Form name="Main"><Label name="L" caption="x" /></Form>;
  /** A form whose render throws. */
  const Broken = (): ReactElement => {
    throw new Error('boom');
  };

  test('renders the form of this window with the host state', () => {
    const html = renderToStaticMarkup(<Shell bridge={bridge({ L: { caption: 'from host' } })} forms={{ Main }} />);
    expect(html).toContain('<div class="vcl-form" data-name="Main"');
    expect(html).toContain('>from host</span>');
  });

  test('a window without a generated form and a broken form are render errors', () => {
    expect(() => renderToStaticMarkup(<Shell bridge={bridge({}, 'Nope')} forms={{ Main }} />)).toThrow(
      "No form named 'Nope' was generated for this window.",
    );
    expect(() => renderToStaticMarkup(<ShellDebug bridge={bridge()} forms={{ Main: Broken }} />)).toThrow('boom');
  });
});

describe('component table', () => {
  test('has one component per visual element of the model and none for the native ones', () => {
    expect(Object.keys(COMPONENTS).sort()).toEqual([
      'button', 'checkbox', 'column', 'combobox', 'edit', 'form', 'groupbox', 'image', 'label', 'listbox', 'listview',
      'memo', 'pagecontrol', 'panel', 'progressbar', 'radiogroup', 'scrollbox', 'splitter', 'statusbar', 'tabsheet',
      'toolbar', 'trackbar', 'treeview',
    ]);
  });
});

test('a container without align in a row keeps the row height, so its client child has room', async () => {
  const css = await Bun.file(new URL('./vcl.css', import.meta.url)).text();
  const centring = css.split('\n').filter(line => line.includes('align-self: center'));
  expect(centring).toHaveLength(1);
  // Only leaf controls (buttons, labels) are centred on the row's cross axis.
  const selector = centring[0]!.split('{')[0]!;
  for (const container of ['[data-layout]', '.vcl-groupbox', '.vcl-pagecontrol']) {
    expect(selector).toContain(container);
    expect(selector.indexOf(':not(')).toBeLessThan(selector.indexOf(container));
  }
});
