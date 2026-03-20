const esbuild = require('esbuild');

esbuild.build({
  entryPoints: ['./src/extension.ts'],
  bundle: true,
  outfile: './out/extension.js',
  external: ['vscode'],
  format: 'cjs',
  platform: 'node',
  target: 'node20',
  sourcemap: true,
  minify: process.argv.includes('--production'),
  // The wasm module is loaded via require() at runtime with a dynamic path.
  // esbuild can't resolve it at build time, which is fine — it stays as
  // a runtime require() call in the bundle.
}).catch(() => process.exit(1));
