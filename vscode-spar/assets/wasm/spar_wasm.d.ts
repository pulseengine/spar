// world root:component/root
import type * as PulseengineRivetTypes from './interfaces/pulseengine-rivet-types.js'; // pulseengine:rivet/types@0.1.0
import type * as WasiCliEnvironment from './interfaces/wasi-cli-environment.js'; // wasi:cli/environment@0.2.6
import type * as WasiCliExit from './interfaces/wasi-cli-exit.js'; // wasi:cli/exit@0.2.6
import type * as WasiCliStderr from './interfaces/wasi-cli-stderr.js'; // wasi:cli/stderr@0.2.6
import type * as WasiCliStdin from './interfaces/wasi-cli-stdin.js'; // wasi:cli/stdin@0.2.6
import type * as WasiCliStdout from './interfaces/wasi-cli-stdout.js'; // wasi:cli/stdout@0.2.6
import type * as WasiCliTerminalInput from './interfaces/wasi-cli-terminal-input.js'; // wasi:cli/terminal-input@0.2.6
import type * as WasiCliTerminalOutput from './interfaces/wasi-cli-terminal-output.js'; // wasi:cli/terminal-output@0.2.6
import type * as WasiCliTerminalStderr from './interfaces/wasi-cli-terminal-stderr.js'; // wasi:cli/terminal-stderr@0.2.6
import type * as WasiCliTerminalStdin from './interfaces/wasi-cli-terminal-stdin.js'; // wasi:cli/terminal-stdin@0.2.6
import type * as WasiCliTerminalStdout from './interfaces/wasi-cli-terminal-stdout.js'; // wasi:cli/terminal-stdout@0.2.6
import type * as WasiClocksMonotonicClock from './interfaces/wasi-clocks-monotonic-clock.js'; // wasi:clocks/monotonic-clock@0.2.6
import type * as WasiClocksWallClock from './interfaces/wasi-clocks-wall-clock.js'; // wasi:clocks/wall-clock@0.2.6
import type * as WasiFilesystemPreopens from './interfaces/wasi-filesystem-preopens.js'; // wasi:filesystem/preopens@0.2.6
import type * as WasiFilesystemTypes from './interfaces/wasi-filesystem-types.js'; // wasi:filesystem/types@0.2.6
import type * as WasiIoError from './interfaces/wasi-io-error.js'; // wasi:io/error@0.2.6
import type * as WasiIoPoll from './interfaces/wasi-io-poll.js'; // wasi:io/poll@0.2.6
import type * as WasiIoStreams from './interfaces/wasi-io-streams.js'; // wasi:io/streams@0.2.6
import type * as WasiRandomInsecureSeed from './interfaces/wasi-random-insecure-seed.js'; // wasi:random/insecure-seed@0.2.6
import type * as PulseengineRivetAdapter from './interfaces/pulseengine-rivet-adapter.js'; // pulseengine:rivet/adapter@0.1.0
import type * as PulseengineRivetRenderer from './interfaces/pulseengine-rivet-renderer.js'; // pulseengine:rivet/renderer@0.1.0
export interface ImportObject {
  'pulseengine:rivet/types@0.1.0': typeof PulseengineRivetTypes,
  'wasi:cli/environment@0.2.6': typeof WasiCliEnvironment,
  'wasi:cli/exit@0.2.6': typeof WasiCliExit,
  'wasi:cli/stderr@0.2.6': typeof WasiCliStderr,
  'wasi:cli/stdin@0.2.6': typeof WasiCliStdin,
  'wasi:cli/stdout@0.2.6': typeof WasiCliStdout,
  'wasi:cli/terminal-input@0.2.6': typeof WasiCliTerminalInput,
  'wasi:cli/terminal-output@0.2.6': typeof WasiCliTerminalOutput,
  'wasi:cli/terminal-stderr@0.2.6': typeof WasiCliTerminalStderr,
  'wasi:cli/terminal-stdin@0.2.6': typeof WasiCliTerminalStdin,
  'wasi:cli/terminal-stdout@0.2.6': typeof WasiCliTerminalStdout,
  'wasi:clocks/monotonic-clock@0.2.6': typeof WasiClocksMonotonicClock,
  'wasi:clocks/wall-clock@0.2.6': typeof WasiClocksWallClock,
  'wasi:filesystem/preopens@0.2.6': typeof WasiFilesystemPreopens,
  'wasi:filesystem/types@0.2.6': typeof WasiFilesystemTypes,
  'wasi:io/error@0.2.6': typeof WasiIoError,
  'wasi:io/poll@0.2.6': typeof WasiIoPoll,
  'wasi:io/streams@0.2.6': typeof WasiIoStreams,
  'wasi:random/insecure-seed@0.2.6': typeof WasiRandomInsecureSeed,
}
export interface Root {
  'pulseengine:rivet/adapter@0.1.0': typeof PulseengineRivetAdapter,
  adapter: typeof PulseengineRivetAdapter,
  'pulseengine:rivet/renderer@0.1.0': typeof PulseengineRivetRenderer,
  renderer: typeof PulseengineRivetRenderer,
}

/**
* Instantiates this component with the provided imports and
* returns a map of all the exports of the component.
*
* This function is intended to be similar to the
* `WebAssembly.instantiate` function. The second `imports`
* argument is the "import object" for wasm, except here it
* uses component-model-layer types instead of core wasm
* integers/numbers/etc.
*
* The first argument to this function, `getCoreModule`, is
* used to compile core wasm modules within the component.
* Components are composed of core wasm modules and this callback
* will be invoked per core wasm module. The caller of this
* function is responsible for reading the core wasm module
* identified by `path` and returning its compiled
* `WebAssembly.Module` object. This would use `compileStreaming`
* on the web, for example.
*/
export function instantiate(
getCoreModule: (path: string) => WebAssembly.Module,
imports: ImportObject,
instantiateCore?: (module: WebAssembly.Module, imports: Record<string, any>) => WebAssembly.Instance
): Root;
export function instantiate(
getCoreModule: (path: string) => WebAssembly.Module | Promise<WebAssembly.Module>,
imports: ImportObject,
instantiateCore?: (module: WebAssembly.Module, imports: Record<string, any>) => WebAssembly.Instance | Promise<WebAssembly.Instance>
): Root | Promise<Root>;

