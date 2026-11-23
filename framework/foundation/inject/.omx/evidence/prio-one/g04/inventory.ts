import ts from 'typescript';
const text=await Bun.file('src/core/cminject.container.ts').text(); const source=ts.createSourceFile('container.ts',text,ts.ScriptTarget.Latest,true);
const cls=source.statements.find(n=>ts.isClassDeclaration(n)&&n.name?.text==='Container') as ts.ClassDeclaration;
const view=source.statements.find(n=>ts.isClassDeclaration(n)&&n.name?.text==='ContainerView') as ts.ClassDeclaration;
const names=new Set(view.members.map(n=>n.name?.getText(source)));
console.log(cls.members.filter(n=>n.name&&!n.modifiers?.some(m=>[ts.SyntaxKind.PrivateKeyword,ts.SyntaxKind.ProtectedKeyword,ts.SyntaxKind.StaticKeyword].includes(m.kind))&&!names.has(n.name.getText(source))).map(n=>n.name!.getText(source)));
