/**
 * Stylesheets are imported for their side effect only (the bundler injects them).
 *
 * Fixes TS2882 under TypeScript 6+, which checks side-effect imports by default
 * (`noUncheckedSideEffectImports`). Decision: declare `*.css` rather than turning
 * that check off, so every other side-effect import stays checked.
 */
declare module '*.css';
