import ts from 'typescript';
const path = 'src/core/cminject.container.ts';
let source = await Bun.file(path).text();
const contracts = ts.createSourceFile('contracts.ts', await Bun.file('src/core/cminject.contracts.ts').text(), ts.ScriptTarget.Latest, true);
const declaration = contracts.statements.find(item => ts.isInterfaceDeclaration(item) && item.name.text === 'IContainer') as ts.InterfaceDeclaration;
const contextual = new Set(['resolve','resolveSync','resolveAll','instantiate','instantiateAsync','injectMembers','injectMembersAsync','signal']);
let generated = '/** Explicit, compiler-checked public forwarding shared by factory and completion contexts. */\nabstract class ContainerView implements IContainer, Scope {\n  /** Actual receiver for every non-contextual operation. */\n  constructor(readonly target: Container) {}\n  /** Internal resolution keeps the contextual synchronous fast path. */\n  abstract resolveEither(token: InjectionToken): unknown;\n';
for (let i=0;i<declaration.members.length;i++) {
 const member=declaration.members[i]!;
 if(!member.name || !ts.isIdentifier(member.name)) throw Error('unexpected member');
 const name=member.name.text;
 if (ts.isPropertySignature(member)) {
   const type=member.type!.getText(contracts);
   generated += `  /** ${contextual.has(name)?'Contextual':'Target-backed'} ${name}. */\n`;
   generated += contextual.has(name)?`  abstract get ${name}(): ${type};\n`:`  get ${name}(): ${type} { return this.target.${name}; }\n`;
   if (!member.modifiers?.some(m=>m.kind===ts.SyntaxKind.ReadonlyKeyword)) generated+=`  set ${name}(value: ${type}) { this.target.${name} = value; }\n`;
   continue;
 }
 if (!ts.isMethodSignature(member)) throw Error('not method');
 const signature=member.getText(contracts);
 generated+=`  /** ${contextual.has(name)?'Contextual':'Delegates to the actual target:'} ${name}. */\n`;
 if(contextual.has(name)) { generated+=`  abstract ${signature}\n`; continue; }
 const next=declaration.members[i+1];
 if(next?.name?.getText(contracts)===name) {
   generated+=`  ${signature}\n`;
   const second=next as ts.MethodSignature; generated+=`  ${second.getText(contracts)}\n`;
   generated+=`  ${name}<T, D = void>(token: InjectionToken<T>, factory?: ServiceFactory<T, D> | ServiceLifecycle<T, D> | RegisterOptions, options?: RegisterOptions): BindingHandle<T, D> | BindingHandle<T> {\n    if (typeof factory === 'function' || isLifecycle(factory)) return this.target.${name}(token, factory, options);\n    const ctor = token instanceof DelayedConstructor ? token.get() : token;\n    if (typeof ctor !== 'function') throw new TypeError(\`\${tokenName(token)} needs a factory\`);\n    return this.target.${name}(ctor, factory);\n  }\n`;
   i++; continue;
 }
 const args=member.parameters.map(p=>`${p.dotDotDotToken?'...':''}${p.name.getText(contracts)}`).join(', ');
 generated+=`  ${signature.replace(/;$/, '')} { return this.target.${name}(${args}); }\n`;
}
generated+='}\n\n';
source=source.replace('/**\n * One build: its frame', generated+'/**\n * One build: its frame');
source=source.replace('class ResolutionView implements Frame, Resolver, Build {','class ResolutionView extends ContainerView implements Frame, Resolver, Build {').replace('class CompletionView implements Scope {','class CompletionView extends ContainerView {');
source=source.replace('    this.done = false; this.target = target;', '    super(target); this.done = false;');
source=source.replace('constructor(readonly target: Container, private readonly link: CompletionLink, private readonly host: CompletionHost) {}','constructor(target: Container, private readonly link: CompletionLink, private readonly host: CompletionHost) { super(target); }');
const start=source.indexOf('/**\n * Puts a forwarding layer');
const end=source.indexOf('/** No retained instances.',start);
source=source.slice(0,start)+'/** Preserve the historical one-way instanceof relationship without transplanting methods. */\nObject.setPrototypeOf(ContainerView.prototype, Container.prototype);\n\n'+source.slice(end);
await Bun.write(path, source);
