mod support;
use coldmind_runtime::{engine::*, engines::*};
use std::{cell::Cell, rc::Rc};
use support::{Boxed, Inline, RustFixture};
struct Factory {
    id: &'static str,
    descriptor: &'static str,
    boxed: bool,
    fail: bool,
    calls: Rc<Cell<usize>>,
}
impl EngineFactory for Factory {
    fn id(&self) -> &str {
        self.id
    }
    fn create(&self) -> Result<Box<dyn JsEngine>, FailureReport> {
        self.calls.set(self.calls.get() + 1);
        if self.fail {
            return Err(FailureReport::fault(
                FailureCode::Capacity,
                "factory failure",
            ));
        }
        Ok(if self.boxed {
            Box::new(RustFixture::<Boxed>::new(self.descriptor))
        } else {
            Box::new(RustFixture::<Inline>::new(self.descriptor))
        })
    }
}
fn factory(
    id: &'static str,
    descriptor: &'static str,
    boxed: bool,
    fail: bool,
    calls: &Rc<Cell<usize>>,
) -> Rc<dyn EngineFactory> {
    Rc::new(Factory {
        id,
        descriptor,
        boxed,
        fail,
        calls: calls.clone(),
    })
}
#[test]
fn selection_is_explicit_and_never_falls_back() {
    let mut registry = EngineRegistry::new();
    let calls = Rc::new(Cell::new(0));
    assert!(registry.create_selected(None).is_err());
    assert!(
        registry
            .register(factory("", "", false, false, &calls))
            .is_err()
    );
    registry
        .register(factory("inline", "inline", false, false, &calls))
        .unwrap();
    assert_eq!(calls.get(), 0);
    assert_eq!(
        registry
            .create_selected(None)
            .unwrap()
            .descriptor()
            .adapter_id,
        "inline"
    );
    assert!(
        registry
            .register(factory("inline", "wrong", false, false, &calls))
            .is_err()
    );
    registry
        .register(factory("boxed", "boxed", true, false, &calls))
        .unwrap();
    assert_eq!(
        registry.identifiers().collect::<Vec<_>>(),
        vec!["boxed", "inline"]
    );
    assert!(registry.create_selected(None).is_err());
    assert!(registry.create_selected(Some("missing")).is_err());
    for id in ["inline", "boxed"] {
        assert_eq!(
            registry
                .create_selected(Some(id))
                .unwrap()
                .descriptor()
                .adapter_id,
            id
        );
    }
    registry
        .register(factory("mismatch", "other", false, false, &calls))
        .unwrap();
    assert!(registry.create_selected(Some("mismatch")).is_err());
    registry
        .register(factory("failure", "failure", false, true, &calls))
        .unwrap();
    assert_eq!(
        registry
            .create_selected(Some("failure"))
            .err()
            .unwrap()
            .code,
        FailureCode::Capacity
    );
}
