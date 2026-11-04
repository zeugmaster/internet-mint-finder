import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const directory = process.argv[2] || 'target/site/pkg';
try {
  const javascript = readFileSync(join(directory, 'imf.js')).toString('base64');
  const frontend = await import(`data:text/javascript;base64,${javascript}`);
  frontend.initSync({ module: readFileSync(join(directory, 'imf.wasm')) });
  assert.equal(typeof frontend.hydrate, 'function');
  console.log('Compiled WebAssembly initializes successfully.');
} catch (error) {
  console.error(`Compiled WebAssembly failed: ${error.message}`);
  process.exitCode = 1;
}
