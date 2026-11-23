import ts from 'typescript';
const entries: string[] = [];
for(const path of ['src/core/cminject.container.ts','src/core/cminject.token.ts','src/core/cminject.decorators.ts','src/core/cminject.contracts.ts']) {
 const text=await Bun.file(path).text(); const source=ts.createSourceFile(path,text,ts.ScriptTarget.Latest,true);
 const visit=(n:ts.Node)=> { if(ts.isAsExpression(n)||ts.isTypeAssertionExpression(n)) {const line=source.getLineAndCharacterOfPosition(n.getStart(source)).line+1;entries.push(`${path}:${line} — ${n.getText(source).replaceAll('\n',' ')}`);} ts.forEachChild(n,visit);};visit(source);
}
await Bun.write('.omx/evidence/prio-one/g04/assertions.txt',entries.join('\n')+'\n');
