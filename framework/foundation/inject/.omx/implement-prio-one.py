from pathlib import Path
import shutil,json,re,time
root=Path.cwd(); archive=root.parents[2]/'.archive'/'2026-10-27'/'revisions'/('solo-delivery-'+str(int(time.time())))/'packages/foundation/inject'
for name in ['src','scripts','tests','examples','docs']:
 if (root/name).exists(): shutil.copytree(root/name,archive/name)
for name in ['package.json','tsconfig.json','tsconfig.build.json','vitest.config.ts','vitest.react.config.ts','README.md','CLAUDE.md','DECISIONS.md','PRIO_ONE.md']:
 if (root/name).exists(): shutil.copy2(root/name,archive/name)
paths={'.':'src/core/cminject.core.ts','./server':'src/server/cminject.server.ts','./hot':'src/cminject.hot.ts','./inspect':'src/cminject.inspect.ts','./react':'src/cminject.react.ts','./testing':'src/cminject.testing.ts'}
for n in ['contracts','token','errors','graph','decorators','container','module','lazy']: paths['./'+n]='src/core/cminject.'+n+'.ts'
for n in ['async','binding','view','observe','construction','resolution','mutation','host']: paths['./internal/'+n]='src/core/cminject.'+n+'.ts'
for n in ['context','runtime']: paths['./internal/server/'+n]='src/server/cminject.'+n+'.ts'
p=json.loads(Path('package.json').read_text()); p['exports']={k:{'types':'./'+v.replace('src/','dist/',1).replace('.ts','.d.ts'),'bun':'./'+v,'default':'./'+v.replace('src/','dist/',1).replace('.ts','.js')} for k,v in paths.items()}; p['exports']['./package.json']='./package.json'; p['files']=['src','dist','README.md'];p['sideEffects']=['./src/core/cminject.decorators.ts','./dist/core/cminject.decorators.js'];p['scripts'].update({'check:imports':'bun scripts/check-imports.ts','test:dist':'node --test tests/node/*.test.mjs','docs:api':'typedoc --options typedoc.json'});Path('package.json').write_text(json.dumps(p,indent=2)+'\n')
mapping={'@coldmind/inject/'+v:'@coldmind/inject'+('' if k=='.' else '/'+k[2:]) for k,v in paths.items()}
for folder in ['src','tests','scripts','examples','docs']:
 for f in Path(folder).rglob('*'):
  if f.is_file() and f.suffix in ['.ts','.tsx','.mts','.mjs','.md','.json']:
   s=f.read_text(); old=s
   for a,b in mapping.items(): s=s.replace(a,b)
   if s!=old:f.write_text(s)
for name in ['tsconfig.json','tsconfig.build.json']:
 f=Path(name);v=json.loads(f.read_text());v['compilerOptions'].update({'rootDir':'src','outDir':'dist','rewriteRelativeImportExtensions':True,'verbatimModuleSyntax':True});f.write_text(json.dumps(v,indent=2)+'\n')
for name in ['vitest.config.ts','vitest.react.config.ts']:
 f=Path(name);s=f.read_text();s=s.replace('defineConfig(shared({','defineConfig(shared({\n  resolve: { conditions: ["bun", "module", "browser", "development"] },');f.write_text(s)
# Preserve the prepared qualification implementation without invoking tools or checks.
proposal=Path('.omx/evidence/prio-one/qualification-prep/cminject.token.proposed.ts').read_text(); original=Path('.omx/evidence/prio-one/qualification-prep/cminject.token.original.ts').read_text(); start=proposal.index('/** Public derived tokens'); end=proposal.index('/** Finds an existing string key',start);f=Path('src/core/cminject.token.ts');s=f.read_text();s=s.replace('/** Finds an existing string key',proposal[start:end]+'/** Finds an existing string key');f.write_text(s)
# Prepared exact-arity aliases and overloads, adapted to active routes.
f=Path('src/core/cminject.contracts.ts');s=f.read_text();facade=Path('.omx/prepared-tests/typed-factory.facade.ts').read_text();aliases=facade[facade.index('/** Dependency values'):facade.index('/** Typed dialect first')];s="import type { DependencySlot, DependencyValues } from '@coldmind/inject/lazy';\nimport type { ModuleDefinition, ModuleHandle } from '@coldmind/inject/module';\n"+s;s=s.replace('/** Optional lifecycle hooks',aliases+'\n/** Optional lifecycle hooks',1)
methods=['singleton','transient','scoped','tenant','request','singletonMulti','transientMulti','scopedMulti','tenantMulti','requestMulti']
for name in methods:
 needle='  '+name+'<T, TData = void>'
 pos=s.index(needle);s=s[:pos]+f'  {name}<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;\n'+s[pos:]
 # Discriminate lifecycle dialect, preserving factory/data contracts.
 line=re.compile(r'(  '+name+r'<T, TData = void>[^\n]+)');s=line.sub(lambda m:m[0].replace('ServiceLifecycle<T, TData>',"(ServiceLifecycle<T, TData> & { readonly inject?: never })"),s)
s=s.replace('deps?: readonly InjectionToken[];', 'deps?: readonly DependencySlot[];')
s=s.replace('export interface RegisterOptions {',"export interface RegisterOptions {\n  /** Ordered candidate predicate; the unconditional candidate is the fallback. */\n  when?: (context: ConditionContext) => boolean;")
s=s.replace('export interface BindingDescriptor<T = unknown> {','export interface BindingDescriptor<T = unknown> {\n  /** Registration module, when loaded transactionally. */\n  module?: string;')
s=s.replace("'dangling' | 'decorator'","'dangling' | 'decorator' | 'module-cycle' | 'undeclared' | 'undeclared-edge' | 'ambiguous'")
s=s.replace('export interface DependencyEdge {','export interface DependencyEdge {\n  /** Deferred edges are checked for availability/captivity but not eager cycles. */\n  lazy?: boolean;')
s=s.replace('export type StaticInjection = InjectionToken | { token: InjectionToken;', 'export type StaticInjection = DependencySlot | { token: DependencySlot;')
# All authored injection metadata accepts dependency slots; registry/service tokens remain ordinary.
for interface in ['ParameterInjection','MemberInjection']:
 start=s.index('export interface '+interface);end=s.index('\n}',start);part=s[start:end].replace('token?: InjectionToken','token?: DependencySlot').replace('token: InjectionToken','token: DependencySlot');s=s[:start]+part+s[end:]
s=s.replace('inject(spec: { token: InjectionToken;', 'inject(spec: { token: DependencySlot;')
s=s.replace('  validate(): void;', '  validate(options?: ValidationOptions): void;')
s=s.replace('export interface IContainer {', '''export interface ValidationOptions { requireDeps?: boolean; includeObserved?: boolean; }
export interface ConditionContext { readonly requester?: ResolutionPathStep; readonly path: readonly ResolutionPathStep[]; readonly scope: ScopeIdentity; }
/** Opaque owner-bound registry checkpoint. */
declare const snapshotBrand: unique symbol;
export interface RegistrySnapshot { readonly [snapshotBrand]: true; }
export interface IContainer {
  load(...modules: ModuleDefinition[]): Promise<ModuleHandle[]>;
  snapshot(): RegistrySnapshot;
  restore(snapshot: RegistrySnapshot): Promise<void>;
''')
f.write_text(s)
# Export actual implemented leaf APIs.
f=Path('src/core/cminject.core.ts');s=f.read_text()+"\nexport * from '@coldmind/inject/module';\nexport * from '@coldmind/inject/lazy';\n";f.write_text(s)
# Incorporate tested ownership engine into its existing cohesive module.
f=Path('src/core/cminject.module.ts');s=f.read_text();engine=Path('.omx/evidence/prio-one/module-loader-prep/engine.ts').read_text();engine=engine.replace("import type { ModuleDefinition, ModuleHandle } from '@coldmind/inject/src/core/cminject.module.ts';\n",'');
for a,b in mapping.items():engine=engine.replace(a,b)
f.write_text(s+'\n'+engine)
# Registration overloads in concrete and explicit forwarding views.
for filename in ['src/core/cminject.container.ts','src/core/cminject.view.ts']:
 f=Path(filename);s=f.read_text();s="import type { DependencySlot } from '@coldmind/inject/lazy';\nimport type { InjectedFactory, InjectedLifecycle, ExactFactoryOptions, RegistrySnapshot, ValidationOptions } from '@coldmind/inject/contracts';\nimport type { ModuleDefinition, ModuleHandle } from '@coldmind/inject/module';\n"+s
 for name in methods:
  pattern=re.compile(r'(^  '+name+r'<T, D = void>[^\n]+)',re.M);match=pattern.search(s)
  if match:
   typed=f'  {name}<T, const Slots extends readonly DependencySlot[], Factory extends InjectedFactory<T, Slots>>(token: InjectionToken<T>, lifecycle: InjectedLifecycle<T, Slots, Factory>, ...options: ExactFactoryOptions<Slots, Factory>): BindingHandle<T>;\n'
   s=s[:match.start()]+typed+s[match.start():];s=pattern.sub(lambda m:m[0].replace('ServiceLifecycle<T, D>',"(ServiceLifecycle<T, D> & { readonly inject?: never })"),s)
  # Implementation accepts typed lifecycle erased only at operational registration boundary.
  s=re.sub(r'('+name+r'<T, D>\(token: InjectionToken<T>, factory\?: )',r'\1',s)
 if filename.endswith('view.ts'):
  s=s.replace('  validate(): void { this.target.validate(); }','  validate(options?: ValidationOptions): void { this.target.validate(options); }')
  pos=s.index('\n',s.index('export class ContainerView'))+1;s=s[:pos]+'''  load(...modules: ModuleDefinition[]): Promise<ModuleHandle[]> { return this.target.load(...modules); }
  snapshot(): RegistrySnapshot { return this.target.snapshot(); }
  restore(snapshot: RegistrySnapshot): Promise<void> { return this.target.restore(snapshot); }
'''+s[pos:]
 f.write_text(s)
Path('typedoc.json').write_text(json.dumps({'entryPoints':['src/core/cminject.core.ts','src/server/cminject.server.ts','src/cminject.testing.ts','src/cminject.inspect.ts','src/cminject.react.ts'],'out':'docs/api','skipErrorChecking':False,'excludePrivate':True,'excludeInternal':True},indent=2)+'\n')
print('Wrote explicit distribution/imports, leaf integration, public contracts and overloads; no validation executed.')
