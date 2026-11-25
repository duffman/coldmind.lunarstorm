//! The CLI host's terminal API as the HostApi area `ui`: progress bars,
//! spinners and styles for scripts (`lunarstorm:hostapi/ui`, also importable as
//! `lunarstorm:ui`).
//!
//! The area binds the natives only `ui.js` uses (`columns`, `live`) and serves
//! `lunarstorm_cliui::UI_SOURCE`. `ui.js` also reads `ops.color` and `ops.tty`;
//! those stay with the host's own bindings ([`crate::host::bindings`]) because the
//! Runtime's console reads `ops.color` whether or not the area is used.

use lunar_extension_hostapi::Area;
use lunarstorm_cliui::term;
use lunarstorm_runtime::vm::{Call, Scope, Vm};
use lunarstorm_runtime::{Bindings, Use};

/// The area's module.
pub const MODULE: &str = "lunarstorm:hostapi/ui";

/// The terminal API area.
pub struct Ui;

impl<V: Vm> Area<V> for Ui {
    const NAME: &'static str = "ui";

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
            .function("columns", |call: &Call<'_, V>| Ok(call.scope().int(i32::from(term::columns()))))
            // Enters or leaves the live region: hides or restores the cursor.
            .function("live", |call: &Call<'_, V>| {
                let scope = call.scope();
                term::live(call.args().first().and_then(|on| scope.as_bool(on)).unwrap_or(false));
                Ok(scope.undefined())
            })
            .module(MODULE, lunarstorm_cliui::UI_SOURCE))
    }
}
