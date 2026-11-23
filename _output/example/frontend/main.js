// Entry of every form's WebView: loads the form module named by the shell and mounts it.
// A dev.refresh re-imports the module and remounts; control values come from the shell's
// state, so they are kept (spec 6.1).
import { mount } from './vcl.js';

const zebra = window.zebra;
const root = document.getElementById('app');

/**
 * Imports the generated module of this window's form.
 * @param {number} [stamp] cache buster from dev.refresh
 * @returns {Promise<{tree: any[]}>}
 */
const load = async (stamp) => (await import(`./forms/${zebra.windowId}.js${stamp ? `?t=${stamp}` : ''}`)).default;

let view = mount(root, await load(), zebra);
zebra.onRefresh(async (stamp) => {
  const next = await load(stamp);
  view.unmount();
  view = mount(root, next, zebra);
});
