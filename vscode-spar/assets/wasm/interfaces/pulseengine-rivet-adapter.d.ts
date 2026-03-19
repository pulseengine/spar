/** @module Interface pulseengine:rivet/adapter@0.1.0 **/
export function id(): string;
export function name(): string;
export function supportedTypes(): Array<string>;
export { _import as import };
function _import(source: Uint8Array, config: AdapterConfig): Array<Artifact>;
export { _export as export };
function _export(artifacts: Array<Artifact>, config: AdapterConfig): Uint8Array;
export type Artifact = import('./pulseengine-rivet-types.js').Artifact;
export type AdapterConfig = import('./pulseengine-rivet-types.js').AdapterConfig;
export type AdapterError = import('./pulseengine-rivet-types.js').AdapterError;
