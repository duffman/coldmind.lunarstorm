/* Jetbunny private owner-thread graph API. Values are checked module objects. */
static JSModuleDef *jetbunny_checked_module(JSContext *ctx, JSValueConst value)
{
    struct list_head *entry;
    if (JS_VALUE_GET_TAG(value) == JS_TAG_MODULE) {
        void *pointer = JS_VALUE_GET_PTR(value);
        list_for_each(entry, &ctx->loaded_modules) {
            JSModuleDef *module = list_entry(entry, JSModuleDef, link);
            if (module == pointer)
                return module;
        }
    }
    JS_ThrowTypeError(ctx, "foreign or invalid prepared module");
    return NULL;
}
int JS_JetbunnyModuleCount(JSContext *ctx, JSValueConst value)
{
    JSModuleDef *m = jetbunny_checked_module(ctx, value);
    return m ? m->req_module_entries_count : -1;
}
JSValue JS_JetbunnyModuleImport(JSContext *ctx, JSValueConst value, int index) {
    JSModuleDef *m = jetbunny_checked_module(ctx, value);
    if (!m) return JS_EXCEPTION;
    if (index < 0 || index >= m->req_module_entries_count)
        return JS_ThrowRangeError(ctx, "module import index");
    /* lunarstorm patch: import attributes now reach the module (rquickjs
       loaders receive them). Lunarstorm's host picks a module's kind from
       its file, so they are ignored here, as they were when jetbunny's
       compiler dropped them; jetbunny's "import attributes unsupported"
       rejection was unreachable then and would break such imports now. */
    return JS_AtomToString(ctx, m->req_module_entries[index].module_name);
}
int JS_JetbunnyModuleBind(JSContext *ctx, JSValueConst value, int index, JSValueConst target) {
    JSModuleDef *m = jetbunny_checked_module(ctx, value);
    if (!m) return -1;
    if (index < 0 || index >= m->req_module_entries_count || JS_VALUE_GET_TAG(target) != JS_TAG_MODULE) {
        JS_ThrowTypeError(ctx, "invalid module edge"); return -1;
    }
    JSModuleDef **edge = &m->req_module_entries[index].module;
    JSModuleDef *next = jetbunny_checked_module(ctx, target);
    if (!next) return -1;
    if (*edge && *edge != next) { JS_ThrowTypeError(ctx, "module edge already bound"); return -1; }
    *edge = next; return 0;
}
int JS_JetbunnyModuleLink(JSContext *ctx, JSValueConst value) {
    JSModuleDef *m = jetbunny_checked_module(ctx, value);
    if (!m) return -1;
    /* The host preflights the whole graph before this call. */
    if (js_create_module_function(ctx, m) < 0) return -1;
    return js_link_module(ctx, m);
}
static int jetbunny_rekey_function(JSContext *ctx, JSValueConst value, JSAtom name) {
    if (JS_VALUE_GET_TAG(value) != JS_TAG_FUNCTION_BYTECODE) return 0;
    if (js_check_stack_overflow(ctx->rt, 0)) { JS_ThrowStackOverflow(ctx); return -1; }
    JSFunctionBytecode *b = JS_VALUE_GET_PTR(value);
    JS_FreeAtom(ctx, b->filename); b->filename = JS_DupAtom(ctx, name);
    for (int i = 0; i < b->cpool_count; ++i)
        if (jetbunny_rekey_function(ctx, b->cpool[i], name) < 0) return -1;
    return 0;
}
int JS_JetbunnyModuleRekey(JSContext *ctx, JSValueConst value, JSValueConst name) {
    if (JS_VALUE_GET_TAG(value) != JS_TAG_MODULE) { JS_ThrowTypeError(ctx,"module image required"); return -1; }
    if (!JS_IsString(name)) { JS_ThrowTypeError(ctx, "module identity must be a string"); return -1; }
    JSAtom atom = JS_ValueToAtom(ctx, name);
    if (!atom) return -1;
    JSModuleDef *m = jetbunny_checked_module(ctx, value);
    if (!m) { JS_FreeAtom(ctx, atom); return -1; }
    JS_FreeAtom(ctx, m->module_name); m->module_name = atom;
    return jetbunny_rekey_function(ctx, m->func_obj, atom);
}

JSValue JS_JetbunnyModuleValue(JSContext *ctx, JSModuleDef *m) { return JS_NewModuleValue(ctx,m); }

JSValue JS_JetbunnyModuleNamespace(JSContext *ctx, JSValueConst value)
{
    JSModuleDef *m = jetbunny_checked_module(ctx, value);
    if (!m) return JS_EXCEPTION;
    if (m->status == JS_MODULE_STATUS_UNLINKED || m->status == JS_MODULE_STATUS_LINKING)
        return JS_ThrowTypeError(ctx, "module must be instantiated");
    return JS_GetModuleNamespace(ctx, m);
}
void JS_JetbunnyDiscardBuiltin(JSContext *ctx, JSModuleDef *m)
{
    /* Only called for an unexposed JS_NewCModule result before a ModuleValue exists. */
    js_free_module_def(ctx, m);
}

void JS_JetbunnySetDynamicImportFunc(JSRuntime *rt, JSJetbunnyDynamicImportFunc *callback, void *opaque)
{
    rt->jetbunny_dynamic_import = callback;
    rt->jetbunny_dynamic_import_opaque = opaque;
}
