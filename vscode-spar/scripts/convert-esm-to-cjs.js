#!/usr/bin/env node
/**
 * Convert jco-transpiled ESM module to CommonJS.
 * jco outputs `export function instantiate(...)` but VS Code extensions
 * need CJS (require). This script rewrites the exports.
 */
const fs = require('fs');
const path = require('path');

const jsPath = path.join(__dirname, '..', 'assets', 'wasm', 'spar_wasm.js');
let src = fs.readFileSync(jsPath, 'utf8');

// Replace ESM export with CJS module.exports
src = src.replace(/^"use jco";\n?/, '');
src = src.replace('export function instantiate(', 'module.exports.instantiate = function instantiate(');

fs.writeFileSync(jsPath, src);
console.log('Converted spar_wasm.js to CJS');
