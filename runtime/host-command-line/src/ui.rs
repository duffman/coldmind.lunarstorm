//! The CLI host's terminal API as the HostApi area `ui`: progress bars,
//! spinners and styles for scripts (`lunarstorm:hostapi/ui`, also importable as
//! `lunarstorm:ui`).
//!
//! The area binds the natives only `ui.js` uses (`columns`, `live`) and serves
//! `lunarstorm_cliui::UI_SOURCE`. `ui.js` also reads `ops.color` and `ops.tty`;
//! those stay with the host's own bindings ([`crate::host::bindings`]) because the
//! Runtime's console reads `ops.color` whether or not the area is used.

use lunar_extension_hostapi::{Arg, Area, Function};
use lunarstorm_cliui::term;
use lunarstorm_event_loop_runtime::vm::{Call, Param, Scope, Sig, Ty, Vm};
use lunarstorm_event_loop_runtime::{Bindings, Use};

/// The area's module.
pub const MODULE: &str = "lunarstorm:hostapi/ui";

/// `ops.columns()`.
const COLUMNS: Sig = Sig { params: &[], rest: None, returns: Ty::Number, about: "The terminal width now." };

/// `ops.live(on)`.
const LIVE: Sig = Sig {
    params: &[Param { name: "on", ty: Ty::Bool, about: "true to enter, false to leave" }],
    rest: None,
    returns: Ty::Undefined,
    about: "Enters or leaves the live region: hides or restores the cursor.",
};

/// The terminal API area.
pub struct Ui;

impl<V: Vm> Area<V> for Ui {
    const NAME: &'static str = "ui";

    /// The functions `ui.js` exports, for the HostApi spec. `style` is an object of
    /// styling functions, not a call, so it is not listed.
    const FUNCTIONS: &'static [Function] = &[
        Function {
            name: "progress",
            summary: "Starts a progress bar; options total, label, width, bytes.",
            args: &[Arg { name: "options", optional: true, multiple: false }],
        },
        Function { name: "spinner", summary: "Starts a spinner for work of unknown length.", args: &[Arg { name: "label", optional: true, multiple: false }] },
    ];

    /// Builds the terminal natives and the module over them.
    ///
    /// # Arguments
    /// * `_using` - unused: the area has no configuration.
    ///
    /// # Returns
    /// `ops.columns`, `ops.live` and `lunarstorm:hostapi/ui`.
    fn bindings(&self, _using: &Use<'_>) -> Result<Bindings<V>, String> {
        Ok(Bindings::<V>::new()
            // The terminal width now; a resize is seen on the next redraw.
            .function("columns", COLUMNS, |call: &Call<'_, V>| Ok(call.scope().int(i32::from(term::columns()))))
            // Enters or leaves the live region: hides or restores the cursor.
            .function("live", LIVE, |call: &Call<'_, V>| {
                let scope = call.scope();
                term::live(call.args().first().and_then(|on| scope.as_bool(on)).unwrap_or(false));
                Ok(scope.undefined())
            })
            .module(MODULE, lunarstorm_cliui::UI_SOURCE))
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
