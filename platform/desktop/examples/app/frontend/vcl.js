// Minimal DOM VCL for the example. It stands in for the React VCL that 4b fills in: one
// component per control type, literal props from the view resource, values from the shell.

/**
 * Creates one control element.
 * @param {string} type control type
 * @param {Record<string, any>} props view-resource props
 * @param {(event: string, props?: object) => void} emit sends an event for this control
 * @returns {{el: HTMLElement, set: (props: Record<string, any>) => void, host: HTMLElement}}
 */
function create(type, props, emit) {
  const el = document.createElement(type === 'edit' ? 'input' : type === 'listbox' ? 'ul' : type === 'button' ? 'button' : 'div');
  el.className = `vcl-${type} vcl-align-${props.align ?? 'none'}`;
  if (props.layout) el.dataset.layout = props.layout;
  if (props.gap !== undefined) el.style.gap = `${props.gap}px`;
  if (props.padding !== undefined) el.style.padding = `${props.padding}px`;
  if (props.width !== undefined) el.style.width = `${props.width}px`;
  if (props.height !== undefined) el.style.height = `${props.height}px`;
  if (props.hint) el.title = props.hint;
  let items = [];
  let index = -1;
  const paintList = () => {
    el.replaceChildren(
      ...items.map((text, i) => {
        const li = document.createElement('li');
        li.textContent = text;
        li.setAttribute('role', 'option');
        li.setAttribute('aria-selected', String(i === index));
        li.onclick = () => {
          if (el.getAttribute('aria-disabled') === 'true') return;
          index = i;
          paintList();
          emit('onChange', { itemIndex: i });
        };
        return li;
      }),
    );
  };
  if (type === 'button') el.onclick = () => emit('onClick');
  if (type === 'edit') el.oninput = () => emit('onChange', { text: el.value });
  if (type === 'listbox') el.setAttribute('role', 'listbox');
  const set = (values) => {
    for (const [key, value] of Object.entries(values)) {
      if (key === 'visible') el.hidden = !value;
      else if (key === 'enabled') {
        if ('disabled' in el) el.disabled = !value;
        el.setAttribute('aria-disabled', String(!value));
      } else if (key === 'caption' && type !== 'panel') el.textContent = value;
      else if (key === 'text' && el.value !== value) el.value = value;
      else if (key === 'placeholder') el.placeholder = value;
      else if (key === 'items') { items = value; paintList(); }
      else if (key === 'itemIndex') { index = value; paintList(); }
    }
  };
  set(props);
  return { el, set, host: el };
}

/**
 * Mounts a generated form tree.
 * @param {HTMLElement} root mount point
 * @param {{tree: any[]}} form generated form module
 * @param {typeof window.zebra} zebra shell client
 * @returns {{unmount: () => void}}
 */
export function mount(root, form, zebra) {
  const controls = new Map();
  const build = ([type, props, children]) => {
    const control = create(type, props, (event, values) => zebra.emit(props.name, event, values));
    controls.set(props.name, control);
    for (const child of children ?? []) control.host.appendChild(build(child));
    return control.el;
  };
  root.replaceChildren(build(form.tree));
  const apply = (values) => {
    for (const [name, props] of Object.entries(values)) controls.get(name)?.set(props);
  };
  apply(zebra.state);
  const off = zebra.onUpdate(apply);
  return {
    unmount() {
      off();
      root.replaceChildren();
    },
  };
}
