//! Lunarstorm projects: the project types products register with their actions,
//! and inspecting, compiling and signing a project.
//!
//! | module        | role                                                                  |
//! |---------------|-----------------------------------------------------------------------|
//! | [`command`]   | the `project` package of the `lunar` command line: specs and handlers |
//! | [`developer`] | developer keys issued by Lunarstorm, and the root that verifies them  |

pub mod command;
pub mod developer;

pub use command::{create, package, Registration, Type};
