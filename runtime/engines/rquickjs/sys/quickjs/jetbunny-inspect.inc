/* Single authoritative native metadata inspector. No JS_WriteObject calls. */
const void *JS_JetbunnyInspectRoot(JSContext *ctx,JSValueConst value){
 if(!jetbunny_owner(ctx))return NULL;
 if(JS_VALUE_GET_TAG(value)==JS_TAG_MODULE){JSModuleDef *m=jetbunny_checked_module(ctx,value);if(!m)return NULL;value=m->func_obj;}
 if(JS_VALUE_GET_TAG(value)!=JS_TAG_FUNCTION_BYTECODE)return NULL;
 const JSFunctionBytecode *b=JS_VALUE_GET_PTR(value);return b->realm==ctx?b:NULL;
}
int JS_JetbunnyInspectView(JSContext *ctx,const void *handle,JSJetbunnyGraphView *v){
 const JSFunctionBytecode *b=handle;if(!b||b->realm!=ctx||!v)return -1;
 v->code=b->byte_code_buf;v->code_len=b->byte_code_len;
 v->positions=b->pc2line_buf;v->positions_len=b->pc2line_len;
 v->locals=b->var_count;v->args=b->arg_count;v->defined_args=b->defined_arg_count;
 v->stack=b->stack_size;v->constants=b->cpool_count;v->strict_mode=b->is_strict_mode;
 v->captures=b->closure_var_count;v->var_refs=b->var_ref_count;
 v->name=b->func_name;v->filename=b->filename;v->line=b->line_num;v->column=b->col_num;
 v->flags=b->has_prototype|(b->has_simple_parameter_list<<1)|(b->is_derived_class_constructor<<2)|(b->need_home_object<<3)|(b->func_kind<<4)|(b->new_target_allowed<<6)|(b->super_call_allowed<<7)|(b->super_allowed<<8)|(b->arguments_allowed<<9)|(b->backtrace_barrier<<10);
 return 0;
}
int JS_JetbunnyInspectValue(JSContext *ctx,JSValueConst value,JSJetbunnyValueView *out){
 memset(out,0,sizeof(*out));
 if(JS_IsNumber(value)){out->kind=0;return JS_ToFloat64(ctx,&out->number,value);}
 if(JS_IsBool(value)){out->kind=1;out->number=JS_ToBool(ctx,value);return 0;}
 if(JS_IsUndefined(value)){out->kind=2;return 0;}
 if(JS_IsNull(value)){out->kind=3;return 0;}
 if(JS_IsString(value)){out->kind=4;return 0;}
 if(JS_IsBigInt(value)){out->kind=6;return 0;}
 if(JS_VALUE_GET_TAG(value)==JS_TAG_FUNCTION_BYTECODE){out->kind=5;out->function=JS_VALUE_GET_PTR(value);return 0;}
 /* Tagged-template site object: a non-extensible array; number = length. */
 if(JS_IsArray(value)==1&&JS_IsExtensible(ctx,value)==0){uint32_t len;if(js_get_length32(ctx,&len,value))return -1;out->kind=7;out->number=len;return 0;}
 return -1;
}
int JS_JetbunnyInspectConstant(JSContext *ctx,const void *handle,uint32_t index,JSJetbunnyValueView *v){
 const JSFunctionBytecode *b=handle;if(!b||b->realm!=ctx||index>=(uint32_t)b->cpool_count)return -1;return JS_JetbunnyInspectValue(ctx,b->cpool[index],v);
}
int JS_JetbunnyInspectLocal(JSContext *ctx,const void *handle,uint32_t index,JSJetbunnyLocalView *v){
 const JSFunctionBytecode *b=handle;if(!b||b->realm!=ctx||index>=(uint32_t)b->arg_count+b->var_count)return -1;
 if(!b->vardefs)return 1;
 const JSVarDef *d=&b->vardefs[index];v->name=d->var_name;v->scope_level=d->scope_level;v->scope_next=d->scope_next;
 v->flags=d->is_const|(d->is_lexical<<1)|(d->is_captured<<2)|(d->var_kind<<3);v->var_ref_idx=d->is_captured?d->var_ref_idx:0;return 0;
}
int JS_JetbunnyInspectCapture(JSContext *ctx,const void *handle,uint32_t index,JSJetbunnyCaptureView *v){
 const JSFunctionBytecode *b=handle;if(!b||b->realm!=ctx||index>=(uint32_t)b->closure_var_count)return -1;
 const JSClosureVar *d=&b->closure_var[index];v->name=d->var_name;v->var_idx=d->var_idx;
 v->flags=d->closure_type|(d->is_const<<3)|(d->is_lexical<<4)|(d->var_kind<<5);return 0;
}

int JS_JetbunnyInspectString(JSContext *ctx,JSValueConst value,uint16_t *out,size_t cap){
 if(!JS_IsString(value))return -1;size_t len=0;const uint16_t *text=JS_ToCStringLenUTF16(ctx,&len,value);
 if(!text)return -1;int result=-1;
 if(len<=16*1024*1024){if(!out)result=(int)len;else if(cap>=len){memcpy(out,text,len*sizeof(uint16_t));result=(int)len;}}
 JS_FreeCStringUTF16(ctx,text);return result;
}
int JS_JetbunnyInspectText(JSContext *ctx,const void *handle,uint32_t kind,uint32_t index,uint16_t *out,size_t cap){
 const JSFunctionBytecode *b=handle;JSValue value;
 if(kind==0)value=JS_AtomToString(ctx,index);
 else if(!b||b->realm!=ctx)return -1;
 else if(kind==1){if(index>=(uint32_t)b->cpool_count||!JS_IsString(b->cpool[index]))return -1;value=JS_DupValue(ctx,b->cpool[index]);}
 else if(kind==2){if(!b->source_len)return -2;value=JS_NewStringLen(ctx,b->source,b->source_len);}
 else if(kind>=4){
  /* Template object constant: kind 4+2e cooked element e, 5+2e raw element e. */
  uint32_t element=(kind-4)/2,len;JSValueConst t;
  if(index>=(uint32_t)b->cpool_count)return -1;t=b->cpool[index];
  if(JS_IsArray(t)!=1||JS_IsExtensible(ctx,t)!=0)return -1;
  JSValue array=(kind-4)&1?JS_GetProperty(ctx,t,JS_ATOM_raw):JS_DupValue(ctx,t);if(JS_IsException(array))return -1;
  if(js_get_length32(ctx,&len,array)||element>=len){JS_FreeValue(ctx,array);return -1;}
  value=JS_GetPropertyUint32(ctx,array,element);JS_FreeValue(ctx,array);
  if(JS_IsUndefined(value))return -2;
 }
 else return -1;
 if(JS_IsException(value))return -1;int result=JS_JetbunnyInspectString(ctx,value,out,cap);JS_FreeValue(ctx,value);return result;
}
/* Canonical little-endian two's complement, independent of serialized bytes. */
int JS_JetbunnyInspectBigInt(JSContext *ctx,const void *handle,uint32_t index,uint8_t *out,size_t cap){
 const JSFunctionBytecode *b=handle;if(!b||b->realm!=ctx||index>=(uint32_t)b->cpool_count)return -1;
 JSValueConst value=b->cpool[index];if(!JS_IsBigInt(value))return -1;
 uint32_t small=(uint32_t)JS_VALUE_GET_INT(value);const js_limb_t *limbs=&small;size_t count=1;
 if(JS_VALUE_GET_TAG(value)==JS_TAG_BIG_INT){const JSBigInt *big=JS_VALUE_GET_PTR(value);limbs=big->tab;count=big->len;}
 if(!count||count>JS_BIGINT_MAX_SIZE)return -1;
 size_t len=count*4;
 #define QB_BIG_BYTE(i) ((uint8_t)(limbs[(i)/4]>>(((i)%4)*8)))
 while(len>1&&((QB_BIG_BYTE(len-1)==0&&QB_BIG_BYTE(len-2)<128)||(QB_BIG_BYTE(len-1)==255&&QB_BIG_BYTE(len-2)>=128)))len--;
 if(len==1&&QB_BIG_BYTE(0)==0)len=0;
 if(!out)return (int)len;if(cap<len)return -1;
 for(size_t i=0;i<len;i++)out[i]=QB_BIG_BYTE(i);
 #undef QB_BIG_BYTE
 return (int)len;
}
int JS_JetbunnyInspectModule(JSContext *ctx,JSValueConst value,JSJetbunnyModuleView *out){
 if(!jetbunny_owner(ctx)||!out)return -1;if(JS_VALUE_GET_TAG(value)==JS_TAG_FUNCTION_BYTECODE)return 1;
 JSModuleDef *m=jetbunny_checked_module(ctx,value);if(!m||m->init_func||!JS_IsUndefined(m->attributes))return -1;
 out->name=m->module_name;out->required=m->req_module_entries_count;out->exports=m->export_entries_count;out->stars=m->star_export_entries_count;out->imports=m->import_entries_count;out->has_tla=m->has_tla;return 0;
}
int JS_JetbunnyInspectModuleEntry(JSContext *ctx,JSValueConst value,uint32_t kind,uint32_t index,JSJetbunnyModuleEntry *out){
 if(!jetbunny_owner(ctx)||!out)return -1;JSModuleDef *m=jetbunny_checked_module(ctx,value);if(!m)return -1;memset(out,0,sizeof(*out));
 if(kind==0){if(index>=(uint32_t)m->req_module_entries_count||!JS_IsUndefined(m->req_module_entries[index].attributes))return -1;out->name=m->req_module_entries[index].module_name;}
 else if(kind==1){if(index>=(uint32_t)m->export_entries_count)return -1;JSExportEntry *e=&m->export_entries[index];out->kind=e->export_type;out->name=e->export_name;if(e->export_type==JS_EXPORT_TYPE_LOCAL)out->capture=e->u.local.var_idx;else if(e->export_type==JS_EXPORT_TYPE_INDIRECT){out->module=e->u.req_module_idx;out->import_name=e->local_name;}else return -1;}
 else if(kind==2){if(index>=(uint32_t)m->star_export_entries_count)return -1;out->module=m->star_export_entries[index].req_module_idx;}
 else if(kind==3){if(index>=(uint32_t)m->import_entries_count)return -1;JSImportEntry *e=&m->import_entries[index];out->capture=e->var_idx;out->name=e->import_name;out->module=e->req_module_idx;}
 else return -1;return 0;
}
