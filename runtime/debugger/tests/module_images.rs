use coldmind_runtime::engine::*;
fn profile() -> ImageProfile {
    ImageProfile {
        adapter_id: "fixture".into(),
        native_revision: "rev".into(),
        format_version: 1,
        target_fingerprint: "target".into(),
        build_fingerprint: "flags".into(),
        debug_info: true,
    }
}
#[test]
fn compiler_images_retain_exact_bytes_and_lossless_requests_without_source() {
    let request = JsString::from_units(vec![0xd800, 0, 0x61]);
    // SAFETY: This fixture defines its trusted compiler representation as these exact bytes.
    let image = unsafe {
        CompiledModuleImage::from_trusted_compiler_output(
            profile(),
            vec![1, 2, 3],
            vec![request.clone()],
        )
    };
    assert_eq!(image.profile(), &profile());
    assert_eq!(image.payload(), [1, 2, 3]);
    assert_eq!(image.imports(), [request]);
    let cloned = image.clone();
    drop(image);
    assert_eq!(cloned.payload(), [1, 2, 3]);
    let mut other = profile();
    other.build_fingerprint.push('x');
    assert_ne!(cloned.profile(), &other);
}

#[test]
fn execution_caches_bind_source_identity_and_origin_without_live_contexts() {
    let text = JsString::from_utf8("fixture source");
    let identity = JsString::from_utf8("entry");
    let source = ScriptSource::new(text.clone(), "memory:entry".into());
    // SAFETY: This fixture defines these exact compiler bytes for the stated source/key/origin.
    let cache = unsafe {
        SourceModuleCache::from_compiler_output(
            profile(),
            vec![1, 2, 3],
            vec![],
            SourceDigest::of_utf16(&text),
            identity.clone(),
            source.origin.clone(),
        )
    };
    assert!(cache.matches_source(&identity, &source));
    assert!(!cache.matches_source(&JsString::from_utf8("other"), &source));
    assert!(!cache.matches_source(&identity, &ScriptSource::new(text, "memory:other".into())));
    assert!(!cache.matches_source(
        &identity,
        &ScriptSource::new(JsString::from_utf8("different source"), source.origin)
    ));
    fn portable<T: Send + Sync>() {}
    portable::<SourceModuleCache>();
}
