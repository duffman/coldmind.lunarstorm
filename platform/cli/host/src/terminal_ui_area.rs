//! The CLI host's terminal API as the HostApi area `ui`: progress bars,
//! spinners and styles for scripts (`lunarstorm:hostapi/ui`, also importable as
//! `lunarstorm:ui`).
//!
//! The area serves one native module, built by
//! [`crate::terminal_ui_module`] at its first import. It adds nothing to `ops`:
//! the module reads the terminal's width, depths and cursor through
//! [`lunarstorm_cliui::term`] directly. `ops.color` and `ops.tty` stay with the
//! host's own bindings ([`crate::command_line_host_services::bindings`]) because
//! the Runtime's console reads `ops.color` whether or not the area is used.

use crate::terminal_ui_module;
use lunar_extension_hostapi::{Arg, Area, Function};
use lunarstorm_event_loop_runtime::vm::Vm;
use lunarstorm_event_loop_runtime::{Bindings, Use};

/// The area's module.
pub const MODULE: &str = "lunarstorm:hostapi/ui";

/// The terminal API area.
pub struct Ui;

impl<V: Vm> Area<V> for Ui {
    const NAME: &'static str = "ui";

    /// The functions the module exports, for the HostApi spec. `style` is an object of
    /// styling functions, not a call, so it is not listed.
    const FUNCTIONS: &'static [Function] = &[
        Function {
            name: "progress",
            summary: "Starts a progress bar; options total, label, width, bytes.",
            args: &[Arg { name: "options", optional: true, multiple: false }],
        },
        Function { name: "spinner", summary: "Starts a spinner for work of unknown length.", args: &[Arg { name: "label", optional: true, multiple: false }] },
    ];

    /// Builds the area's module.
    ///
    /// # Arguments
    /// * `_using` - unused: the area has no configuration.
    ///
    /// # Returns
    /// `lunarstorm:hostapi/ui`, native.
    fn bindings(&self, _using: &Use<'_>) -> Result<Bindings<V>, String> {
        Ok(Bindings::<V>::new().native_module_with_internals(terminal_ui_module::declaration(MODULE), |scope, ops, live, exports| {
            terminal_ui_module::exports::<V>(scope, ops, live, exports)
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::Ui;
    use lunar_extension_hostapi::HostApi;
    use lunarstorm_engine_quickjs_adapter::QuickJs;

    /// The spec names the area's module and its two functions with their optional argument.
    #[test]
    fn spec_lists_the_terminal_functions() {
        let spec = HostApi::<QuickJs, _>::new((Ui,)).spec();
        let area = &spec["areas"][0];
        assert_eq!(area["module"], "lunarstorm:hostapi/ui");
        let names: Vec<_> = area["functions"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap().to_owned()).collect();
        assert_eq!(names, ["progress", "spinner"]);
        assert_eq!(area["functions"][1]["args"][0]["name"], "label");
        assert_eq!(area["functions"][1]["args"][0]["optional"], true);
    }
}
