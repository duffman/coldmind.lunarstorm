//! Zebra native desktop process (spec section 6).
//!
//! | module     | role                                                                 |
//! |------------|----------------------------------------------------------------------|
//! | `manifest` | host configuration (4e) and per-form property/event tables (4d)      |
//! | `envelope` | IPC envelope, WebView/backend message validation, property mirror    |
//! | `assets`   | asset store, custom protocol responses, release archive + attachment |
//! | `pipeline` | the pipeline boundary the debug runtime embeds; output-dir loader     |
//! | `changes`  | the dev change table (6.1) as a diff of two pipeline outputs         |
//! | `js`       | JS thread: VM Instance, timers, budget, reload by epoch              |
//! | `native`   | native services and their thread routing                             |
//! | `ui`       | UI thread: TAO loop, windows, WRY WebViews, menus, tray, timers      |
//! | `dev`      | debug runtime: file watcher and compiler thread                      |
//! | `package`  | release step: attach archive, bundle, sign per OS                    |
//! | `browser`  | browser host: loopback server + in-page host and backend Worker      |
//! | `runtime`  | AppRuntime interface, build-time selection (`build.rs`), MockRuntime |

pub mod assets;
pub mod browser;
pub mod changes;
pub mod dev;
pub mod envelope;
pub mod js;
pub mod manifest;
pub mod native;
pub mod package;
pub mod pipeline;
pub mod runtime;
pub mod ui;
