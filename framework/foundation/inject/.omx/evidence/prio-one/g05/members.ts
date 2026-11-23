import ts from 'typescript';
const text=await Bun.file('src/core/cminject.container.ts').text();const source=ts.createSourceFile('container.ts',text,ts.ScriptTarget.Latest,true);
const declaration=source.statements.find(node=>ts.isClassDeclaration(node)&&node.name?.text==='Container') as ts.ClassDeclaration;
await Bun.write('.omx/evidence/prio-one/g05/members.json',JSON.stringify(declaration.members.filter(node=>ts.isMethodDeclaration(node)).map(node=>({name:node.name.getText(source),start:node.getFullStart(),end:node.end,text:node.getFullText(source)}))));
await Bun.write('.omx/evidence/prio-one/g05/globals.json', JSON.stringify(source.statements.filter(node=>ts.isFunctionDeclaration(node)||ts.isInterfaceDeclaration(node)).map(node=>({name:node.name?.getText(source),start:node.getFullStart(),end:node.end,text:node.getFullText(source)}))));
await Bun.write('.omx/evidence/prio-one/g05/nodes.json',JSON.stringify(declaration.members.filter(node=>ts.isPropertyDeclaration(node)||ts.isGetAccessorDeclaration(node)).map(node=>({name:node.name.getText(source),start:node.getFullStart(),end:node.end,text:node.getFullText(source)}))));
