import { lazy, Suspense, type ComponentType } from 'react';
import { createPortal } from 'react-dom';
import { createRoot } from 'react-dom/client';
import { desktopConnection } from './api/frontend';
__SHELL_IMPORT__

const appRoot = document.getElementById('root')!;
const shellRoot = document.createElement('div');
document.body.append(shellRoot);

let entryFailed = false;
const App = lazy(async () => {
    try {
        const entry = await import('../../src/main.tsx') as { default?: ComponentType };
        // Older entries mount into #root themselves when their module runs.
        return { default: entry.default ?? (() => null) };
    } catch (error) {
        entryFailed = true;
        throw error;
    }
});

createRoot(shellRoot).render(
    <ShellWrapper connection={desktopConnection} reloadOnUpdate={() => entryFailed}>
        <Suspense fallback={null}>
            {createPortal(<App />, appRoot)}
        </Suspense>
    </ShellWrapper>,
);
