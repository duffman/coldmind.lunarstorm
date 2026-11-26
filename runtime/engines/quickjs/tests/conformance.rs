//! Runs the VM conformance suite against the QuickJS adapter.

use lunarstorm_vm::conformance;
use lunarstorm_vm_quickjs::QuickJs;

#[test]
fn quickjs_conforms_to_the_vm_contract() {
    assert_eq!(conformance::run::<QuickJs>(), 19);
}
