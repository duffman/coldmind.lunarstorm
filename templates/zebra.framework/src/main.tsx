/**
 * The WebView runtime of a built application: React, the React VCL and `mount`,
 * bundled once into `dist/vcl.js` and copied into every build's `frontend/`.
 *
 * Decisions:
 * - The runtime is prebuilt and the pipeline embeds the bundle, so building a
 *   project needs no JavaScript toolchain: the frontend generator (spec 4b) only
 *   writes one small module per form that calls `h` on entries of `C`.
 * - One bundle serves debug and release. The shell is chosen at runtime from
 *   `window.zebra.mode`, and React is the production build in both, so a form
 *   behaves the same in dev as when shipped.
 * - The bundle is built with `bun build --production`. Fix: built with only
 *   `--minify` and a NODE_ENV define, bun compiled JSX with the development
 *   transform (`jsxDEV`), which production React does not export, so every VCL
 *   component threw "p is not a function" in the WebView.
 * - A form module is imported by name with the refresh stamp as its query, so
 *   `dev.refresh` re-imports that one module and nothing else (spec 6.1).
 */
import { createElement, type ComponentType } from 'react';
import { createRoot } from 'react-dom/client';
import { COMPONENTS, Shell, ShellDebug, type Bridge } from '@lunarstorm/vcl';

declare global {
  interface Window {
    /** The shell's IPC client, injected by the host before any page script. */
    zebra: Bridge;
  }
}

/** `React.createElement`, under the name the generated form modules use. */
export const h = createElement;

/** The component of each visual element name. */
export const C = COMPONENTS;

/** A generated form module. */
export interface FormModule {
  /** The form component. */
  default: ComponentType;
}

/**
 * Renders this WebView's form and re-renders it on `dev.refresh`.
 * @param load imports the module of a form: `(formName, refreshStamp)`
 */
export function mount(load: (name: string, stamp: number) => Promise<FormModule>): void {
  const bridge = window.zebra;
  const root = createRoot(document.getElementById('root')!);
  const ShellView = bridge.mode === 'release' ? Shell : ShellDebug;
  /**
   * Imports the form and renders it inside the shell.
   * @param stamp the refresh stamp; 0 for the first render
   */
  const show = async (stamp: number): Promise<void> => {
    let forms: Record<string, ComponentType> = {};
    try {
      forms = { [bridge.windowId]: (await load(bridge.windowId, stamp)).default };
    } catch (error) {
      // A form module that does not load leaves `forms` empty, so the shell's
      // error boundary reports the missing form instead of a blank window.
      console.error('[vcl] the form module failed to load', error);
    }
    root.render(createElement(ShellView, { bridge, forms }));
  };
  bridge.onRefresh(stamp => void show(stamp));
  void show(0);
}
