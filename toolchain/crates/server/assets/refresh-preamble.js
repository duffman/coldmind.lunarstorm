// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Raphael Amorim

import { injectIntoGlobalHook } from "/@devermind/refresh-runtime.js";
injectIntoGlobalHook(window);
window.$RefreshReg$ = () => {};
window.$RefreshSig$ = () => (type) => type;
window.__devermind_refresh_installed__ = true;
window.process ??= { env: { NODE_ENV: "development" } };
window.global ??= window;
window.setImmediate ??= (fn, ...args) => setTimeout(fn, 0, ...args);
window.clearImmediate ??= (id) => clearTimeout(id);
