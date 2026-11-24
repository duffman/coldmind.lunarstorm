/* VM metadata and lifetime operations only: no parser or name resolution. */
typedef struct JetbunnyFrameLease {
 struct JetbunnyFrameLease *previous;
 JSContext *ctx;JSStackFrame *frame;JSFunctionBytecode *bytecode;
 JSValue function;const uint8_t *pc;int scope;uint64_t id;bool selected;
} JetbunnyFrameLease;
static _Thread_local JetbunnyFrameLease *jetbunny_active_lease;
static _Thread_local uint64_t jetbunny_next_lease;
static bool jetbunny_owner(JSContext *ctx){return ctx&&ctx->jetbunny_owner==&jetbunny_owner_thread;}
static JetbunnyFrameLease *jetbunny_lease(JSContext *ctx,uint64_t id){
 if(!jetbunny_owner(ctx)||!id)return NULL;
 JetbunnyFrameLease *lease=jetbunny_active_lease;
 if(!lease||lease->id!=id||lease->ctx!=ctx)return NULL;
 if(lease->selected&&(!ctx->debug_in_hook||!ctx->debug_pause_sf))return NULL;
 JSStackFrame *sf=ctx->rt->current_stack_frame;
 while(sf&&sf!=lease->frame)sf=sf->prev_frame;
 if(!sf||sf->cur_pc!=lease->pc||JS_VALUE_GET_TAG(sf->cur_func)!=JS_TAG_OBJECT||
    JS_VALUE_GET_OBJ(sf->cur_func)!=JS_VALUE_GET_OBJ(lease->function))return NULL;
 JSObject *object=JS_VALUE_GET_OBJ(sf->cur_func);
 if(!js_class_has_bytecode(object->class_id)||object->u.func.function_bytecode!=lease->bytecode||lease->bytecode->realm!=ctx)return NULL;
 return lease;
}
static int jetbunny_scope_valid(JSFunctionBytecode *b,int scope){
 if(scope==ARG_SCOPE_END||scope==-1)return 1;
 uint32_t count=0;
 while(scope>=0){if(scope>=b->var_count||!b->vardefs||count++>b->var_count)return 0;scope=b->vardefs[b->arg_count+scope].scope_next;}
 return scope==-1||scope==ARG_SCOPE_END;
}
/* Both inspection and eval use the lexical head of the executed marker.
 * Scope teardown invalidates it until another marker, never guessing globals. */
static int jetbunny_selected_scope(JSContext *ctx,JSStackFrame *sf,JSFunctionBytecode *b,int *scope){
 if(!sf->debug_scope_known)return -1;
 if(sf==ctx->debug_pause_sf){*scope=ctx->debug_pause_scope;return jetbunny_scope_valid(b,*scope)?0:-1;}
 if(sf->debug_scope_known){*scope=sf->debug_scope;return jetbunny_scope_valid(b,*scope)?0:-1;}
 return -1;

}
static int jetbunny_open_lease(JSContext *ctx,JSStackFrame *sf,int scope,bool selected,JetbunnyFrameLease *lease){
 if(!jetbunny_owner(ctx)||!sf||JS_VALUE_GET_TAG(sf->cur_func)!=JS_TAG_OBJECT||jetbunny_next_lease==UINT64_MAX)return -1;
 JSObject *object=JS_VALUE_GET_OBJ(sf->cur_func);
 if(!js_class_has_bytecode(object->class_id))return -1;
 JSFunctionBytecode *b=object->u.func.function_bytecode;
 if(b->realm!=ctx||!jetbunny_scope_valid(b,scope)||b->var_ref_count>sf->var_ref_count)return -1;
 memset(lease,0,sizeof(*lease));lease->previous=jetbunny_active_lease;lease->ctx=ctx;lease->frame=sf;lease->bytecode=b;
 lease->function=JS_DupValue(ctx,sf->cur_func);lease->pc=sf->cur_pc;lease->scope=scope;lease->selected=selected;lease->id=++jetbunny_next_lease;
 jetbunny_active_lease=lease;return 0;
}
static void jetbunny_close_lease(JetbunnyFrameLease *lease){
 assert(jetbunny_active_lease==lease);jetbunny_active_lease=lease->previous;JS_FreeValue(lease->ctx,lease->function);lease->id=0;
}
int JS_JetbunnySetCompiler(JSContext *ctx,JSJetbunnyCompiler *compiler,void *opaque){
 if(!jetbunny_owner(ctx)||ctx->jetbunny_compiling)return -1;
 ctx->jetbunny_compiler=compiler;ctx->jetbunny_compiler_opaque=opaque;
 /* Reset is fail-closed, even in compiler-enabled test builds. */
 ctx->eval_internal=compiler?jetbunny_compile_dispatch:NULL;return 0;
}
int JS_JetbunnyEnvView(JSContext *ctx,uint64_t id,JSJetbunnyEnvView *out){
 JetbunnyFrameLease *lease=jetbunny_lease(ctx,id);if(!lease||!out)return -1;
 JSFunctionBytecode *b=lease->bytecode;
 out->args=b->arg_count;out->locals=b->var_count;out->captures=b->closure_var_count;out->var_refs=b->var_ref_count;out->strict_mode=b->is_strict_mode;out->scope_head=lease->scope;
 out->flags=b->has_prototype|(b->has_simple_parameter_list<<1)|(b->is_derived_class_constructor<<2)|(b->need_home_object<<3)|(b->func_kind<<4)|(b->new_target_allowed<<6)|(b->super_call_allowed<<7)|(b->super_allowed<<8)|(b->arguments_allowed<<9)|(b->backtrace_barrier<<10);
 /* bit 11: the frame is a script root ("<script>", no new.target): the
    variable environment of sloppy eval code is the global object. */
 JSAtom script=JS_NewAtom(ctx,"<script>");
 if(script!=JS_ATOM_NULL&&b->func_name==script&&!b->new_target_allowed)out->flags|=1u<<11;
 JS_FreeAtom(ctx,script);return 0;
}
int JS_JetbunnyEnvBinding(JSContext *ctx,uint64_t id,uint32_t kind,uint32_t index,JSJetbunnyEnvBinding *out){
 JetbunnyFrameLease *lease=jetbunny_lease(ctx,id);if(!lease||!out)return -1;
 JSFunctionBytecode *b=lease->bytecode;JSStackFrame *sf=lease->frame;memset(out,0,sizeof(*out));out->scope_next=-1;
 if(kind<2){if(!b->vardefs||index>=(kind?b->var_count:b->arg_count))return -1;
  JSVarDef *v=&b->vardefs[(kind?b->arg_count:0)+index];JSValue *values=kind?sf->var_buf:sf->arg_buf;if(!values)return -1;
  out->name=v->var_name;out->scope_level=v->scope_level;out->scope_next=v->scope_next;out->flags=v->is_const|(v->is_lexical<<1)|(v->is_captured<<2)|(v->var_kind<<3);
  out->var_ref_idx=v->is_captured?v->var_ref_idx:0;out->initialized=JS_VALUE_GET_TAG(values[index])!=JS_TAG_UNINITIALIZED;
  out->live=v->is_captured&&v->var_ref_idx<sf->var_ref_count&&sf->var_refs;return 0;
 }
 if(kind==2){if(index>=b->closure_var_count)return -1;JSClosureVar *v=&b->closure_var[index];JSVarRef **refs=JS_VALUE_GET_OBJ(sf->cur_func)->u.func.var_refs;
  out->name=v->var_name;out->flags=v->closure_type|(v->is_const<<3)|(v->is_lexical<<4)|(v->var_kind<<5);out->var_ref_idx=v->var_idx;
  out->live=refs&&refs[index]&&refs[index]->pvalue;out->initialized=out->live&&JS_VALUE_GET_TAG(*refs[index]->pvalue)!=JS_TAG_UNINITIALIZED;return 0;
 }return -1;
}
JSValue JS_JetbunnyEnvName(JSContext *ctx,uint64_t id,uint32_t kind,uint32_t index){
 JSJetbunnyEnvBinding binding;if(JS_JetbunnyEnvBinding(ctx,id,kind,index,&binding)<0)return JS_EXCEPTION;
 return JS_AtomToString(ctx,binding.name);
}
JSValue JS_JetbunnyInstall(JSContext *ctx,const uint8_t *bytes,size_t len,int prepare){
 if(!jetbunny_owner(ctx))return JS_EXCEPTION;
 if(!bytes||!len||len>1024u*1024*1024)return JS_ThrowRangeError(ctx,"invalid verified image size");
 JSValue value=JS_ReadObject(ctx,bytes,len,JS_READ_OBJ_BYTECODE|(prepare?JS_READ_OBJ_JETBUNNY_PREPARE:0));
 if(JS_IsException(value))return value;
 int tag=JS_VALUE_GET_TAG(value);
 if(tag!=JS_TAG_FUNCTION_BYTECODE&&!(prepare&&tag==JS_TAG_MODULE)){JS_FreeValue(ctx,value);return JS_ThrowTypeError(ctx,"verified function/module image required");}
 return value;
}
static int jetbunny_captures_valid(JetbunnyFrameLease *lease,JSFunctionBytecode *b){
 for(uint32_t i=0;i<b->closure_var_count;i++){
  JSClosureVar *cv=&b->closure_var[i];uint32_t kind;
  if(cv->closure_type==JS_CLOSURE_LOCAL)kind=1;else if(cv->closure_type==JS_CLOSURE_ARG)kind=0;
  else if(cv->closure_type==JS_CLOSURE_REF||cv->closure_type==JS_CLOSURE_GLOBAL_REF)kind=2;else return 0;
  JSJetbunnyEnvBinding binding;
  if(JS_JetbunnyEnvBinding(lease->ctx,lease->id,kind,cv->var_idx,&binding)<0||!binding.live||binding.name!=cv->var_name)return 0;
  uint32_t flags=kind==2?((binding.flags>>3)&3):(binding.flags&3);
  if(cv->is_const!=(flags&1)||cv->is_lexical!=((flags>>1)&1)||cv->var_kind!=(kind==2?binding.flags>>5:binding.flags>>3))return 0;
 }return 1;
}
JSValue JS_JetbunnyEvalFrame(JSContext *ctx,uint64_t id,JSValue compiled){
 JetbunnyFrameLease *lease=jetbunny_lease(ctx,id);
 if(!lease||ctx->jetbunny_compiling||JS_VALUE_GET_TAG(compiled)!=JS_TAG_FUNCTION_BYTECODE){if(jetbunny_owner(ctx)){JS_FreeValue(ctx,compiled);return JS_ThrowTypeError(ctx,"invalid, stale or reentrant frame lease");}return JS_EXCEPTION;}
 JSFunctionBytecode *b=JS_VALUE_GET_PTR(compiled);
 if(b->realm!=ctx||!jetbunny_captures_valid(lease,b)){JS_FreeValue(ctx,compiled);return JS_ThrowTypeError(ctx,"eval image does not match live frame bindings");}
 JSObject *function=JS_VALUE_GET_OBJ(lease->frame->cur_func);
 return JS_EvalFunctionInternal(ctx,compiled,JS_UNDEFINED,function->u.func.var_refs,lease->frame);
}
JSValue JS_JetbunnyWithFrame(JSContext *ctx,int level,JSJetbunnyFrameCallback *callback,void *opaque){
 if(!jetbunny_owner(ctx))return JS_EXCEPTION;
 if(!callback||ctx->jetbunny_compiling||(jetbunny_active_lease&&jetbunny_active_lease->selected)||!ctx->debug_in_hook||!ctx->debug_pause_sf)return JS_ThrowTypeError(ctx,"selected-frame lease requires active owner pause");
 JSFunctionBytecode *b;JSStackFrame *sf=js_debug_frame(ctx,level,&b);int scope;
 if(!sf||!b||jetbunny_selected_scope(ctx,sf,b,&scope)<0)return JS_ThrowRangeError(ctx,"selected frame has no exact lexical scope");
 JetbunnyFrameLease lease;if(jetbunny_open_lease(ctx,sf,scope,true,&lease)<0)return JS_ThrowTypeError(ctx,"selected frame realm/layout mismatch");
 JSValue result=callback(ctx,lease.id,opaque);jetbunny_close_lease(&lease);return result;
}
static JSValue jetbunny_compile_units(JSContext *ctx,uint64_t lease,const uint16_t *source,size_t units,const char *filename,int line,int flags){
 static const uint16_t empty_source=0;
 if((!source&&units)||units>256u*1024*1024)return JS_ThrowRangeError(ctx,"compiler source size");
 if(!source)source=&empty_source;
 if(!ctx->jetbunny_compiler)return JS_ThrowTypeError(ctx,"Rust compiler callback is not installed");
 if(ctx->jetbunny_compiling)return JS_ThrowTypeError(ctx,"compiler callback reentry is not allowed");
 if(JS_JetbunnyDebugPoll(ctx))return JS_EXCEPTION;
 ctx->jetbunny_compiling=1;
 JSValue value=ctx->jetbunny_compiler(ctx,lease,source,units,filename,line,flags,ctx->jetbunny_compiler_opaque);
 ctx->jetbunny_compiling=0;
 if(JS_JetbunnyDebugPoll(ctx)){JS_FreeValue(ctx,value);return JS_EXCEPTION;}
 if(!JS_IsException(value)&&((flags&JS_EVAL_TYPE_MASK)==JS_EVAL_TYPE_MODULE)!=(JS_VALUE_GET_TAG(value)==JS_TAG_MODULE)){JS_FreeValue(ctx,value);return JS_ThrowTypeError(ctx,"compiler callback mode mismatch");}
 if(!JS_IsException(value)&&JS_VALUE_GET_TAG(value)==JS_TAG_FUNCTION_BYTECODE&&((JSFunctionBytecode*)JS_VALUE_GET_PTR(value))->realm!=ctx){JS_FreeValue(ctx,value);return JS_ThrowTypeError(ctx,"compiler callback returned foreign realm object");}
 if(!JS_IsException(value)&&JS_VALUE_GET_TAG(value)!=JS_TAG_FUNCTION_BYTECODE&&JS_VALUE_GET_TAG(value)!=JS_TAG_MODULE){JS_FreeValue(ctx,value);return JS_ThrowTypeError(ctx,"compiler callback must return verified compiled object");}
 return value;
}
static JSValue jetbunny_compile_dispatch(JSContext *ctx,JSValueConst this_obj,const char *source,size_t len,const char *filename,int line,int flags,int scope){
 if(!jetbunny_owner(ctx))return JS_EXCEPTION;
 if(ctx->jetbunny_compiling)return JS_ThrowTypeError(ctx,"compiler callback reentry is not allowed");
 JSValue string=JS_NewStringLen(ctx,source,len);if(JS_IsException(string))return string;
 size_t units;const uint16_t *text=JS_ToCStringLenUTF16(ctx,&units,string);JS_FreeValue(ctx,string);if(!text)return JS_EXCEPTION;
 JetbunnyFrameLease lease;bool direct=(flags&JS_EVAL_TYPE_MASK)==JS_EVAL_TYPE_DIRECT;
 if(direct&&jetbunny_open_lease(ctx,ctx->rt->current_stack_frame,scope,false,&lease)<0){JS_FreeCStringUTF16(ctx,text);return JS_ThrowTypeError(ctx,"direct eval has no live lexical frame");}
 JSValue value=jetbunny_compile_units(ctx,direct?lease.id:0,text,units,filename,line,flags);JS_FreeCStringUTF16(ctx,text);
 if(!JS_IsException(value)&&direct&&(flags&JS_EVAL_FLAG_COMPILE_ONLY)){JS_FreeValue(ctx,value);value=JS_ThrowTypeError(ctx,"direct eval compiled objects cannot escape their lease");}
 if(!JS_IsException(value)&&!direct&&JS_VALUE_GET_TAG(value)==JS_TAG_FUNCTION_BYTECODE&&((JSFunctionBytecode*)JS_VALUE_GET_PTR(value))->closure_var_count){JS_FreeValue(ctx,value);value=JS_ThrowTypeError(ctx,"global compiler image has root captures");}
 if(!JS_IsException(value)&&!(flags&JS_EVAL_FLAG_COMPILE_ONLY)){
  if(direct)value=JS_JetbunnyEvalFrame(ctx,lease.id,value);else value=JS_EvalFunctionInternal(ctx,value,this_obj,NULL,NULL);
 }
 if(direct)jetbunny_close_lease(&lease);return value;
}
typedef struct {const uint16_t *text;size_t units;const char *filename;} JetbunnyDebugSource;
static JSValue jetbunny_debug_compile(JSContext *ctx,uint64_t lease,void *opaque){
 JetbunnyDebugSource *source=opaque;
 JSValue value=jetbunny_compile_units(ctx,lease,source->text,source->units,source->filename,1,JS_EVAL_TYPE_DIRECT);
 if(!JS_IsException(value)){if(JS_JetbunnyDebugPoll(ctx)){JS_FreeValue(ctx,value);return JS_EXCEPTION;}value=JS_JetbunnyEvalFrame(ctx,lease,value);}return value;
}
JSValue JS_JetbunnyDebugEvalUTF16(JSContext *ctx,int level,const uint16_t *source,size_t units,const char *filename){
 if(!jetbunny_owner(ctx))return JS_EXCEPTION;
 if((!source&&units)||units>16*1024*1024)return JS_ThrowRangeError(ctx,"eval source size");
 /* lunarstorm patch: the C parser is restored and no Rust compiler callback is
  * registered, so frame evaluation compiles with the C compiler
  * (JS_DebugEvalInFrame) instead of failing with "callback is not installed".
  * The source arrives as UTF-16 and the C compiler reads UTF-8. */
 if(!ctx->jetbunny_compiler){
  JSValue string=JS_NewStringUTF16(ctx,source,units);if(JS_IsException(string))return string;
  size_t len;const char *text=JS_ToCStringLen(ctx,&len,string);JS_FreeValue(ctx,string);if(!text)return JS_EXCEPTION;
  JSValue result=JS_DebugEvalInFrame(ctx,level,text,len,filename);JS_FreeCString(ctx,text);return result;
 }
 JetbunnyDebugSource input={source,units,filename};return JS_JetbunnyWithFrame(ctx,level,jetbunny_debug_compile,&input);
}
static JSValue jetbunny_debug_eval(JSContext *ctx,int level,const char *source,size_t len,const char *filename){
 if(!jetbunny_owner(ctx))return JS_EXCEPTION;
 JSValue string=JS_NewStringLen(ctx,source,len);if(JS_IsException(string))return string;
 size_t units;const uint16_t *text=JS_ToCStringLenUTF16(ctx,&units,string);JS_FreeValue(ctx,string);if(!text)return JS_EXCEPTION;
 JSValue result=JS_JetbunnyDebugEvalUTF16(ctx,level,text,units,filename);JS_FreeCStringUTF16(ctx,text);return result;
}

int JS_JetbunnyDebugEvalControlled(JSContext *ctx,int level,const uint16_t *source,size_t units,const char *filename,JSValue *out){
 if(!jetbunny_owner(ctx)||!out||ctx->jetbunny_debug_evaluating)return -1;
 JSRuntime *rt=ctx->rt;
 JSValue original=rt->current_exception, trace=ctx->error_back_trace;
 uint8_t notified=ctx->debug_exception_notified;
 int counter=ctx->interrupt_counter;
 bool oom=rt->jetbunny_out_of_memory, in_oom=rt->in_out_of_memory;
 rt->current_exception=JS_UNINITIALIZED;ctx->error_back_trace=JS_UNDEFINED;
 rt->jetbunny_out_of_memory=false;ctx->jetbunny_debug_evaluating=1;ctx->jetbunny_debug_unwind=0;
 JSValue result=JS_JetbunnyDebugPoll(ctx)?JS_EXCEPTION:JS_JetbunnyDebugEvalUTF16(ctx,level,source,units,filename);
 JS_JetbunnyDebugPoll(ctx);
 int status=ctx->jetbunny_debug_unwind;
 if(!status)status=rt->jetbunny_out_of_memory?5:JS_IsException(result)?1:0;
 if(status==1){*out=JS_GetException(ctx);JS_FreeValue(ctx,result);}
 else if(status){JS_FreeValue(ctx,result);*out=JS_UNDEFINED;}
 else *out=result;
 JS_FreeValue(ctx,rt->current_exception);JS_FreeValue(ctx,ctx->error_back_trace);
 rt->current_exception=original;ctx->error_back_trace=trace;ctx->debug_exception_notified=notified;
 ctx->interrupt_counter=counter;rt->jetbunny_out_of_memory=oom;rt->in_out_of_memory=in_oom;
 ctx->jetbunny_debug_evaluating=0;ctx->jetbunny_debug_unwind=0;
 return status;
}

/* Exact debugger edit identity. Never retain pvalue, shape index or descriptor
 * storage across explicit RHS evaluation. Targets are owner-only and bounded by
 * the Rust inspection table; the operation holds one root across invalidation. */
typedef struct {
 JSContext *ctx;
 JSValue root;
 JSAtom atom;
 int frame;
 uint32_t scope,kind,index;
 const uint8_t *pc;
} JetbunnyEditTarget;
static JSValue *jetbunny_property_slot(JSContext *ctx,JetbunnyEditTarget *t){
 size_t bound;
 if(JS_DebugOwnPropertyBound(ctx,t->root,&bound)<0)return NULL;
 JSObject *p=JS_VALUE_GET_OBJ(t->root);
 /* Synthetic collection/promise/raw byte views are never own data targets. */
 if(p->class_id==JS_CLASS_MAP||p->class_id==JS_CLASS_SET||p->class_id==JS_CLASS_PROMISE||
    p->class_id==JS_CLASS_ARRAY_BUFFER||p->class_id==JS_CLASS_SHARED_ARRAY_BUFFER||
    p->class_id==JS_CLASS_DATAVIEW||is_typed_array(p->class_id))return NULL;
 if(p->class_id==JS_CLASS_ARRAY&&p->fast_array&&__JS_AtomIsTaggedInt(t->atom)){
  uint32_t index=__JS_AtomToUInt32(t->atom);
  if(index<p->u.array.count)return &p->u.array.u.values[index];
 }
 JSProperty *pr;
 JSShapeProperty *sp=find_own_property(&pr,p,t->atom);
 if(!sp||(sp->flags&JS_PROP_TMASK)!=0||(sp->flags&JS_PROP_LENGTH)||!(sp->flags&JS_PROP_WRITABLE))return NULL;
 return &pr->u.value;
}
static JSValue *jetbunny_binding_slot(JSContext *ctx,JetbunnyFrameLease *lease,JetbunnyEditTarget *t){
 JSFunctionBytecode *b=lease->bytecode;JSStackFrame *sf=lease->frame;
 if(t->pc&&t->pc!=lease->pc)return NULL;
 if(!JS_IsUndefined(t->root)&&JS_VALUE_GET_PTR(t->root)!=JS_VALUE_GET_PTR(sf->cur_func))return NULL;
 JSJetbunnyEnvBinding meta;
 if(t->kind<2||t->kind>4||JS_JetbunnyEnvBinding(ctx,lease->id,t->kind-2,t->index,&meta)<0||meta.name!=t->atom||!meta.initialized)return NULL;
 uint32_t permissions=t->kind==4?(meta.flags>>3)&3:meta.flags&3;
 if(permissions&1)return NULL;
 if(t->kind==4){
  if(t->scope!=JS_DEBUG_SCOPE_CLOSURE||!meta.live)return NULL;
  return JS_VALUE_GET_OBJ(sf->cur_func)->u.func.var_refs[t->index]->pvalue;
 }
 int at=lease->scope,last=-1,mode=0;uint32_t depth=0;bool visible=false;
 while(at>=0){
  JSVarDef *v=&b->vardefs[b->arg_count+at];
  mode=js_debug_scope_mode(mode,v);
  if(v->scope_level>0){
   if(last>=0&&v->scope_level!=last)depth++;
   last=v->scope_level;
   if(t->kind==3&&t->index==(uint32_t)at&&t->scope==depth)visible=true;
  }
  at=v->scope_next;
 }
 if(at==ARG_SCOPE_END)mode|=JS_DEBUG_SCOPE_SPLIT;
 if(t->kind==2)visible=t->scope==JS_DEBUG_SCOPE_FUNCTION&&!(mode&JS_DEBUG_SCOPE_SPLIT);
 else if(meta.scope_level==0)visible=t->scope==JS_DEBUG_SCOPE_FUNCTION&&
  js_debug_function_local_visible(b,t->index,mode);
 if(!visible)return NULL;
 return t->kind==2?&sf->arg_buf[t->index]:&sf->var_buf[t->index];
}
typedef struct {JetbunnyEditTarget *target;JSValueConst value;bool store;int result;} JetbunnyEditAction;
static JSValue jetbunny_target_frame(JSContext *ctx,uint64_t id,void *opaque){
 JetbunnyEditAction *a=opaque;JetbunnyFrameLease *lease=jetbunny_lease(ctx,id);
 JSValue *slot=lease?jetbunny_binding_slot(ctx,lease,a->target):NULL;
 if(!slot)return JS_UNDEFINED;
 if(JS_IsUndefined(a->target->root)){
  a->target->root=JS_DupValue(ctx,lease->function);a->target->pc=lease->pc;
 }
 if(a->store)set_value(ctx,slot,JS_DupValue(ctx,a->value));
 a->result=0;return JS_UNDEFINED;
}
static int jetbunny_target_action(JSContext *ctx,JetbunnyEditTarget *t,bool store,JSValueConst value){
 if(!t||t->ctx!=ctx||!jetbunny_owner(ctx)||!ctx->debug_in_hook||!ctx->debug_pause_sf)return -1;
 if(t->kind==1){
  JSValue *slot=jetbunny_property_slot(ctx,t);if(!slot)return -1;
  if(store)set_value(ctx,slot,JS_DupValue(ctx,value));
  return 0;
 }
 JetbunnyEditAction action={t,value,store,-1};
 /* Valid frame lease construction is allocation-free; isolate rejection from
  * the paused application's pending exception just like inspection. */
 JSValue saved=ctx->rt->current_exception,trace=ctx->error_back_trace;
 uint8_t notified=ctx->debug_exception_notified;bool oom=ctx->rt->jetbunny_out_of_memory;
 ctx->rt->current_exception=JS_UNINITIALIZED;ctx->error_back_trace=JS_UNDEFINED;
 JSValue result=JS_JetbunnyWithFrame(ctx,t->frame,jetbunny_target_frame,&action);
 JS_FreeValue(ctx,result);JS_FreeValue(ctx,ctx->rt->current_exception);JS_FreeValue(ctx,ctx->error_back_trace);
 ctx->rt->current_exception=saved;ctx->error_back_trace=trace;ctx->debug_exception_notified=notified;ctx->rt->jetbunny_out_of_memory=oom;
 return action.result;
}
static void *jetbunny_target_new(JSContext *ctx,JSValueConst object,int frame,uint32_t scope,JSAtom atom,uint32_t kind,uint32_t index,int *status){
 if(status)*status=-1;
 if(!kind||!atom||!jetbunny_owner(ctx))return NULL;
 JetbunnyEditTarget target={ctx,kind==1?object:JS_UNDEFINED,atom,frame,scope,kind,index,NULL};
 if(jetbunny_target_action(ctx,&target,false,JS_UNDEFINED)<0)return NULL;
 JetbunnyEditTarget *owned=js_mallocz(ctx,sizeof(*owned));
 if(!owned){if(kind!=1)JS_FreeValue(ctx,target.root);if(status)*status=-2;return NULL;}
 *owned=target;owned->atom=JS_DupAtom(ctx,atom);
 if(kind==1)owned->root=JS_DupValue(ctx,object);
 if(status)*status=0;
 return owned;
}
void *JS_JetbunnyTargetNew(JSContext *ctx,JSValueConst object,int frame,uint32_t scope,JSAtom atom,uint32_t kind,uint32_t index,int *status){
 JSValue saved=ctx->rt->current_exception,trace=ctx->error_back_trace;
 uint8_t notified=ctx->debug_exception_notified;bool oom=ctx->rt->jetbunny_out_of_memory;
 ctx->rt->current_exception=JS_UNINITIALIZED;ctx->error_back_trace=JS_UNDEFINED;ctx->rt->jetbunny_out_of_memory=false;
 void *target=jetbunny_target_new(ctx,object,frame,scope,atom,kind,index,status);
 JS_FreeValue(ctx,ctx->rt->current_exception);JS_FreeValue(ctx,ctx->error_back_trace);
 ctx->rt->current_exception=saved;ctx->error_back_trace=trace;ctx->debug_exception_notified=notified;ctx->rt->jetbunny_out_of_memory=oom;
 return target;
}
void JS_JetbunnyTargetFree(JSContext *ctx,void *opaque){
 JetbunnyEditTarget *t=opaque;if(!t)return;
 assert(t->ctx==ctx);JS_FreeAtom(ctx,t->atom);JS_FreeValue(ctx,t->root);js_free(ctx,t);
}
int JS_JetbunnyTargetValidate(JSContext *ctx,void *t){return jetbunny_target_action(ctx,t,false,JS_UNDEFINED);}
int JS_JetbunnyTargetStore(JSContext *ctx,void *t,JSValueConst value){return jetbunny_target_action(ctx,t,true,value);}

/* Restricted own-data lookup. This deliberately does not call WithFrame: that
 * evaluator API throws formatted errors on proof failure. Refusals here are
 * statuses and cannot reach Error.prepareStackTrace or any application hook. */
static JSAtom jetbunny_lookup_atom(JSContext *ctx,const uint16_t *text,size_t units){
 JSValue string=JS_NewStringUTF16(ctx,text,units);
 if(JS_IsException(string))return JS_ATOM_NULL;
 return JS_NewAtomStr(ctx,JS_VALUE_GET_STRING(string)); /* consumes string */
}
static int jetbunny_lookup_own(JSContext *ctx,JSValueConst receiver,JSAtom atom,JSValue *out){
 size_t bound;
 /* Reuse inspection's supported-class/exotic guard before raw descriptor access.
  * Typed/shared/raw byte views remain unsupported; no synthetic fields appear. */
 if(JS_DebugOwnPropertyBound(ctx,receiver,&bound)<0)return 2;
 JSObject *p=JS_VALUE_GET_OBJ(receiver);
 if(p->class_id==JS_CLASS_ARRAY_BUFFER||p->class_id==JS_CLASS_SHARED_ARRAY_BUFFER||
    p->class_id==JS_CLASS_DATAVIEW||is_typed_array(p->class_id))return 2;
 if(p->class_id==JS_CLASS_ARRAY&&p->fast_array&&__JS_AtomIsTaggedInt(atom)){
  uint32_t index=__JS_AtomToUInt32(atom);
  if(index<p->u.array.count){
   JSValue value=p->u.array.u.values[index];
   if(JS_IsUninitialized(value))return 1;
   *out=JS_DupValue(ctx,value);return 0;
  }
 }
 JSProperty *pr;JSShapeProperty *sp=find_own_property(&pr,p,atom);
 if(!sp)return 1;
 JSValue value;
 switch(sp->flags&JS_PROP_TMASK){
  case 0:value=pr->u.value;break;
  case JS_PROP_VARREF:
   if(!pr->u.var_ref||!pr->u.var_ref->pvalue)return 2;
   value=*pr->u.var_ref->pvalue;break;
  default:return 2; /* GETSET and AUTOINIT are never touched */
 }
 if(JS_IsUninitialized(value))return 2;
 *out=JS_DupValue(ctx,value);return 0;
}
static bool jetbunny_lookup_dynamic(JSAtom name){
 return name==JS_ATOM__with_||name==JS_ATOM__var_||name==JS_ATOM__arg_var_;
}
static int jetbunny_lookup_binding(JSContext *ctx,JetbunnyFrameLease *lease,JSAtom name,int root_this,JSValue *out){
 JSFunctionBytecode *b=lease->bytecode;JSStackFrame *sf=lease->frame;
 if(!b->vardefs&&(b->arg_count||b->var_count))return 2;
 JSValue value=JS_UNDEFINED;bool found=false;
 int i=lease->scope,mode=0,parameters=-1;
 while(i>=0){
  JSVarDef *vd=&b->vardefs[b->arg_count+i];mode=js_debug_scope_mode(mode,vd);
  /* In the body, the function's var environment precedes outer parameters. */
  if((mode&JS_DEBUG_SCOPE_BODY)&&vd->scope_level==1){parameters=i;mode|=JS_DEBUG_SCOPE_SPLIT;break;}
  if(vd->scope_level>0&&!root_this&&jetbunny_lookup_dynamic(vd->var_name)&&
     (!sf->var_buf||!JS_IsUndefined(sf->var_buf[i])))return 2;
  if(vd->scope_level>0&&vd->var_name==name){if(!sf->var_buf)return 2;value=sf->var_buf[i];found=true;break;}
  i=vd->scope_next;
 }
 if(i==ARG_SCOPE_END)mode|=JS_DEBUG_SCOPE_SPLIT;
 if(!found&&!(mode&JS_DEBUG_SCOPE_SPLIT)){
  for(int j=0;j<b->arg_count;j++)if(b->vardefs[j].var_name==name){
   if(!sf->arg_buf)return 2;value=sf->arg_buf[j];found=true;break;
  }
 }
 if(!found)for(int j=0;j<b->var_count;j++){
  JSVarDef *vd=&b->vardefs[b->arg_count+j];
  if(!js_debug_function_local_visible(b,j,mode)||((mode&JS_DEBUG_SCOPE_BODY)&&js_debug_arg_pseudo(vd)))continue;
  if(!root_this&&jetbunny_lookup_dynamic(vd->var_name)&&(!sf->var_buf||!JS_IsUndefined(sf->var_buf[j])))return 2;
  if(vd->var_name==name){if(!sf->var_buf)return 2;value=sf->var_buf[j];found=true;break;}
 }
 for(i=parameters;!found&&i>=0;i=b->vardefs[b->arg_count+i].scope_next){
  JSVarDef *vd=&b->vardefs[b->arg_count+i];
  if(vd->var_name==name){if(!sf->var_buf)return 2;value=sf->var_buf[i];found=true;break;}
 }
 if(!found&&(mode&JS_DEBUG_SCOPE_BODY))for(int j=0;j<b->var_count;j++){
  JSVarDef *vd=&b->vardefs[b->arg_count+j];
  if(!js_debug_function_local_visible(b,j,mode)||!js_debug_arg_pseudo(vd))continue;
  if(!root_this&&jetbunny_lookup_dynamic(vd->var_name)&&(!sf->var_buf||!JS_IsUndefined(sf->var_buf[j])))return 2;
  if(vd->var_name==name){if(!sf->var_buf)return 2;value=sf->var_buf[j];found=true;break;}
 }
 if(!found){
  JSVarRef **refs=JS_VALUE_GET_OBJ(sf->cur_func)->u.func.var_refs;
  for(int j=0;j<b->closure_var_count;j++){
   JSAtom capture=b->closure_var[j].var_name;
   if(!root_this&&jetbunny_lookup_dynamic(capture)&&
      (!refs||!refs[j]||!refs[j]->pvalue||!JS_IsUndefined(*refs[j]->pvalue)))return 2;
   if(capture==name){if(!refs||!refs[j]||!refs[j]->pvalue)return 2;value=*refs[j]->pvalue;found=true;break;}
  }
 }
 if(found){if(JS_IsUninitialized(value))return 2;*out=JS_DupValue(ctx,value);return 0;}
 if(root_this)return 2;
 int status=jetbunny_lookup_own(ctx,ctx->global_var_obj,name,out);
 if(status!=1)return status;
 return jetbunny_lookup_own(ctx,ctx->global_obj,name,out);
}

int JS_JetbunnyDebugLookup(JSContext *ctx,int level,int root_this,const uint16_t *root,size_t units,
 const JSJetbunnyLookupKey *keys,size_t count,JSValue *out){
 *out=JS_UNDEFINED;
 if(!jetbunny_owner(ctx)||!ctx->debug_in_hook||!ctx->debug_pause_sf||ctx->jetbunny_compiling||
    (jetbunny_active_lease&&jetbunny_active_lease->selected)||count>64||units>16384||
    (!root&&units)||(!keys&&count))return -1;
 JSFunctionBytecode *b;JSStackFrame *sf=js_debug_frame(ctx,level,&b);int scope;
 if(!sf||!b||jetbunny_selected_scope(ctx,sf,b,&scope)<0)return -1;
 JetbunnyFrameLease lease;if(jetbunny_open_lease(ctx,sf,scope,true,&lease)<0)return -1;
 JSValue original=ctx->rt->current_exception,trace=ctx->error_back_trace;
 uint8_t notified=ctx->debug_exception_notified;bool oom=ctx->rt->jetbunny_out_of_memory;
 bool in_oom=ctx->rt->in_out_of_memory;
 ctx->rt->current_exception=JS_UNINITIALIZED;ctx->error_back_trace=JS_UNDEFINED;
 ctx->rt->jetbunny_out_of_memory=false;ctx->rt->in_out_of_memory=false;
 JSValue value=JS_UNDEFINED;int status=-2;
 JSAtom name=root_this?JS_DupAtom(ctx,JS_ATOM_this):jetbunny_lookup_atom(ctx,root,units);
 if(name){
  status=(!root_this&&js_debug_internal_name(name))?2:jetbunny_lookup_binding(ctx,&lease,name,root_this,&value);
  JS_FreeAtom(ctx,name);
 }
 for(size_t i=0;status==0&&i<count;i++){
  const JSJetbunnyLookupKey *key=&keys[i];
  if(key->units>16384||(!key->text&&key->units)){status=2;break;}
  name=key->is_index?JS_NewAtomUInt32(ctx,key->index):jetbunny_lookup_atom(ctx,key->text,key->units);
  if(!name){status=-2;break;}
  JSValue next=JS_UNDEFINED;status=jetbunny_lookup_own(ctx,value,name,&next);
  JS_FreeAtom(ctx,name);JS_FreeValue(ctx,value);value=next;
 }
 if(ctx->rt->jetbunny_out_of_memory)status=-2;
 if(status==0)*out=value;else JS_FreeValue(ctx,value);
 JS_FreeValue(ctx,ctx->rt->current_exception);JS_FreeValue(ctx,ctx->error_back_trace);
 ctx->rt->current_exception=original;ctx->error_back_trace=trace;ctx->debug_exception_notified=notified;
 ctx->rt->jetbunny_out_of_memory=oom;ctx->rt->in_out_of_memory=in_oom;
 jetbunny_close_lease(&lease);return status;
}
