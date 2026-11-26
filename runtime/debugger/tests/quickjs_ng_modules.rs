//! Real native static graph behavior. Tests execute during final qualification only.
#![cfg(feature = "quickjs-ng")]
#[path = "support/no_services.rs"]
mod support;
use coldmind_runtime::{engine::*, engines::quickjs_ng::QuickJsNgEngine};
fn source(text: &str) -> ScriptSource {
    ScriptSource::new(JsString::from_utf8(text), "file:///module.js".into())
}
fn key(s: &str) -> JsString {
    JsString::from_utf8(s)
}
#[test]
fn cycles_link_without_execution_and_preserve_live_bindings() {
    let engine = QuickJsNgEngine::new().unwrap();
    let ctx = engine.create_context(ContextLimits::default()).unwrap();
    engine.with_scope(ctx,&support::NoLoop,&mut |s: &dyn JsScope|{
  let m=s.modules().unwrap();
  let a=m.prepare(&source("import {read} from 'b'; export let n=1; export function inc(){n++} export function both(){return read()+n} globalThis.runs=(globalThis.runs||0)+1;"),&key("a"))?;
  let b=m.prepare(&source("import {n} from 'a'; export function read(){return n}"),&key("b"))?;
  let ab=[ModuleBinding{specifier:key("b"),module:b}];let ba=[ModuleBinding{specifier:key("a"),module:a}];
  assert_eq!(m.imports(&a)?,vec![key("b")]);
  let image=m.images().unwrap().compile(&a)?;assert!(!image.payload().is_empty());
  assert!(m.link(&a,&ab).is_err(),"unbound cycle must fail before native recursion");
  m.bind(&b,&ba)?;m.link(&a,&ab)?;
  let before=s.source().unwrap().evaluate(&source("typeof globalThis.runs")).map_err(|e|s.detach(e))?;
  assert_eq!(s.export_data(before,ExportLimits::default()).map_err(|e|s.detach(e))?,JsData::String(key("undefined")));
  let _=m.evaluate(&a).map_err(|e|s.detach(e))?;
  let _=m.evaluate(&a).map_err(|e|s.detach(e))?;
  let namespace=m.namespace(&a).map_err(|e|s.detach(e))?;
  let inc=s.get(namespace,PropertyKey::String(&key("inc"))).map_err(|e|s.detach(e))?;
  s.call(inc,None,&[]).map_err(|e|s.detach(e))?;
  let both=s.get(namespace,PropertyKey::String(&key("both"))).map_err(|e|s.detach(e))?;
  let result=s.call(both,None,&[]).map_err(|e|s.detach(e))?;
  assert_eq!(s.export_data(result,ExportLimits::default()).map_err(|e|s.detach(e))?,JsData::Number(4.0));
  assert!(m.images().unwrap().compile(&a).is_err());
  let runs=s.source().unwrap().evaluate(&source("globalThis.runs")).map_err(|e|s.detach(e))?;
  assert_eq!(s.export_data(runs,ExportLimits::default()).map_err(|e|s.detach(e))?,JsData::Number(1.0));
  Ok(())
 }).unwrap();
    engine.close_context(ctx).unwrap();
}
#[test]
fn invalid_edges_foreign_ids_and_conflicting_bindings_fail() {
    let e = QuickJsNgEngine::new().unwrap();
    let a = e.create_context(ContextLimits::default()).unwrap();
    let b = e.create_context(ContextLimits::default()).unwrap();
    let mut foreign = None;
    e.with_scope(b, &support::NoLoop, &mut |s: &dyn JsScope| {
        foreign = Some(
            s.modules()
                .unwrap()
                .prepare(&source("export const x=1"), &key("foreign"))?,
        );
        Ok(())
    })
    .unwrap();
    e.with_scope(a, &support::NoLoop, &mut |s: &dyn JsScope| {
        let m = s.modules().unwrap();
        let root = m.prepare(
            &source("import {x} from 'dep'; globalThis.sideEffect=x"),
            &key("root"),
        )?;
        assert!(m.bind(&root, &[]).is_err());
        assert!(m.namespace(&root).is_err());
        assert!(m.evaluate(&root).is_err());
        assert!(
            m.bind(
                &root,
                &[ModuleBinding {
                    specifier: key("dep"),
                    module: foreign.unwrap()
                }]
            )
            .is_err()
        );
        let wrong = m.prepare(&source("export const missing=1"), &key("wrong"))?;
        let edges = [ModuleBinding {
            specifier: key("dep"),
            module: wrong,
        }];
        m.bind(&root, &edges)?;
        assert!(m.link(&root, &edges).is_err());
        assert!(m.link(&root, &edges).is_err());
        assert!(
            m.bind(
                &root,
                &[ModuleBinding {
                    specifier: key("dep"),
                    module: root
                }]
            )
            .is_err()
        );
        let good = m.prepare(&source("export const ok=42"), &key("good"))?;
        m.link(&good, &[])?;
        m.evaluate(&good).map_err(|e| s.detach(e))?;
        Ok(())
    })
    .unwrap();
    e.close_context(a).unwrap();
    e.close_context(b).unwrap();
}
#[test]
fn reused_native_import_slots_preserve_request_sequence() {
    let e = QuickJsNgEngine::new().unwrap();
    let context = e.create_context(ContextLimits::default()).unwrap();
    e.with_scope(context, &support::NoLoop, &mut |s: &dyn JsScope| {
        let evaluate = |text: &str| -> Result<(), FailureReport> {
            s.source()
                .unwrap()
                .evaluate(&source(text))
                .map_err(|e| s.detach(e))?;
            s.drain_microtasks(std::num::NonZeroUsize::new(64).unwrap())?;
            Ok(())
        };
        let m = s.modules().unwrap();
        evaluate("import('a');import('b')")?;
        let first = m.take_dynamic_imports()?.requests;
        assert_eq!(first.len(), 2);
        evaluate("import('c')")?;
        m.finish_dynamic_import(
            first[0].id,
            Err(FailureReport::fault(
                FailureCode::InvalidHandle,
                "load failed",
            )),
        )?;
        evaluate("import('d')")?;
        let later = m.take_dynamic_imports()?.requests;
        assert_eq!(
            later
                .iter()
                .map(|r| r.specifier.clone())
                .collect::<Vec<_>>(),
            vec![key("c"), key("d")]
        );
        for request in first.into_iter().skip(1).chain(later) {
            m.finish_dynamic_import(
                request.id,
                Err(FailureReport::fault(
                    FailureCode::InvalidHandle,
                    "load failed",
                )),
            )?;
        }
        Ok(())
    })
    .unwrap();
    e.close_context(context).unwrap();
}
#[test]
fn failed_link_does_not_poison_an_already_linked_shared_dependency() {
    let engine = QuickJsNgEngine::new().unwrap();
    let context = engine.create_context(ContextLimits::default()).unwrap();
    engine
        .with_scope(context, &support::NoLoop, &mut |scope: &dyn JsScope| {
            let modules = scope.modules().unwrap();
            let dependency = modules.prepare(&source("export const n=42"), &key("shared"))?;
            modules.link(&dependency, &[])?;
            let bad = modules.prepare(
                &source("import {missing} from 'shared'; export const bad=missing"),
                &key("bad"),
            )?;
            assert!(
                modules
                    .link(
                        &bad,
                        &[ModuleBinding {
                            specifier: key("shared"),
                            module: dependency
                        }]
                    )
                    .is_err()
            );
            let good = modules.prepare(
                &source("import {n} from 'shared'; export const answer=n"),
                &key("good"),
            )?;
            modules.link(
                &good,
                &[ModuleBinding {
                    specifier: key("shared"),
                    module: dependency,
                }],
            )?;
            modules.evaluate(&good).map_err(|e| scope.detach(e))?;
            let namespace = modules.namespace(&good).map_err(|e| scope.detach(e))?;
            let answer = scope
                .get(namespace, PropertyKey::String(&key("answer")))
                .map_err(|e| scope.detach(e))?;
            assert_eq!(
                scope
                    .export_data(answer, scope.data_limits())
                    .map_err(|e| scope.detach(e))?,
                JsData::Number(42.0)
            );
            Ok(())
        })
        .unwrap();
    engine.close_context(context).unwrap();
}
