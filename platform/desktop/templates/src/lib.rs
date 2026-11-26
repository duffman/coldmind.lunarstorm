//! The desktop product's project templates for `lunar project init`.
//!
//! | template | writes |
//! |----------|--------|
//! | `app` | a forms project `lunar build` builds: `src/app.tsx`, one form view and its form class |
//!
//! Each folder of this crate holding a `template.json` is one template; the format
//! is documented in `lunarstorm-project` (`platform/project/README.md`). `build.rs`
//! embeds the folders. `app` is also the starter the model and `lunar build` tests
//! build as it stands, so its files carry no `{{name}}` placeholders outside README.md.

include!(concat!(env!("OUT_DIR"), "/files.rs"));
