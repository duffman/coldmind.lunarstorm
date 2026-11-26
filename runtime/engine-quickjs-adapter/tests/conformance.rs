//! Runs the VM conformance suite against the QuickJS adapter.

use lunarstorm_engine_contract::conformance;
use lunarstorm_engine_quickjs_adapter::QuickJs;

#[test]
fn quickjs_conforms_to_the_vm_contract() {
    assert_eq!(conformance::run::<QuickJs>(), 20);
}
