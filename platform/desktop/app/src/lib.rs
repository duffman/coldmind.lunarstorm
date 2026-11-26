//! The Lunarstorm platform's native desktop process (spec section 6).
//!
//! | module     | role                                                                 |
//! |------------|----------------------------------------------------------------------|
//! | `manifest` | host configuration (4e) and per-form property/event tables (4d)      |
//! | `envelope` | IPC envelope, WebView/backend message validation, property mirror    |
//! | `assets`   | asset store, custom protocol responses, release archive + attachment |
//! | `pipeline` | the pipeline boundary the debug runtime embeds; output-dir loader     |
//! | `changes`  | the dev change table (6.1) as a diff of two pipeline outputs         |
//! | `native`   | native services and their thread routing                             |
//! | `ui`       | UI thread: TAO loop, windows, WRY WebViews, menus, tray, timers      |
//! | `dev`      | debug runtime: file watcher and compiler thread                      |
//! | `runtime`  | AppRuntime interface, build-time selection (`build.rs`), the Lunarstorm |
//! |            | Runtime adapter and MockRuntime                                       |
//!
//! Packaging is the `lunarstorm-pack` crate and the browser host the `lunarstorm-web`
//! crate, both in `platform/`; the shipped process links neither.

pub mod assets;
pub mod changes;
pub mod dev;
pub mod envelope;
pub mod manifest;
pub mod native;
pub mod pipeline;
pub mod runtime;
pub mod ui;
