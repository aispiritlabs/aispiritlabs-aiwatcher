import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

const root = fileURLToPath(new URL('../src/', import.meta.url));
function filesIn(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const file = path.join(directory, entry.name);
    return entry.isDirectory() ? filesIn(file) : /\.tsx?$/.test(file) ? [file] : [];
  });
}

const errors = [];
for (const file of filesIn(root)) {
  const relative = path.relative(root, file);
  if (relative.startsWith('api/generated/') || relative === 'routeTree.gen.ts') continue;
  if (/^(components|lib)\//.test(relative)) {
    errors.push(`${relative}: put code in its feature or in shared`);
  }
  const source = ts.createSourceFile(
    file,
    readFileSync(file, 'utf8'),
    ts.ScriptTarget.Latest,
    true,
  );
  const [layer, feature] = relative.split('/');
  function check(specifier) {
    const target = specifier.startsWith('@/')
      ? specifier.slice(2)
      : specifier.startsWith('.')
        ? path.relative(root, path.resolve(path.dirname(file), specifier))
        : null;
    if (!target) return;
    const [targetLayer, targetFeature] = target.split('/');
    let reason;
    if (layer === 'shared' && ['features', 'app', 'routes'].includes(targetLayer)) {
      reason = 'shared code cannot depend on features or application composition';
    } else if (layer === 'features' && ['app', 'routes'].includes(targetLayer)) {
      reason = 'features use getRouteApi; they cannot import route registration or app code';
    } else if (layer === 'features' && targetLayer === 'features' && feature !== targetFeature) {
      reason = 'features are independent; extract intentionally reusable code into shared';
    } else if (
      file.endsWith('/search.ts') &&
      /\/(page|components)(\/|$)/.test(target) &&
      targetLayer === 'features'
    ) {
      reason = 'URL contracts cannot import screens or feature UI';
    }
    if (reason) errors.push(`${relative} → ${specifier}: ${reason}`);
  }
  function visit(node) {
    if (
      (ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) &&
      node.moduleSpecifier &&
      ts.isStringLiteral(node.moduleSpecifier)
    ) {
      check(node.moduleSpecifier.text);
    }
    if (
      ts.isCallExpression(node) &&
      node.expression.kind === ts.SyntaxKind.ImportKeyword &&
      node.arguments[0] &&
      ts.isStringLiteral(node.arguments[0])
    ) {
      check(node.arguments[0].text);
    }
    ts.forEachChild(node, visit);
  }
  visit(source);
}

if (errors.length) {
  console.error(errors.join('\n'));
  process.exitCode = 1;
} else {
  console.log('Panel architecture boundaries passed.');
}
