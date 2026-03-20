/**
 * WASI filesystem shim for spar-wasm in VS Code.
 *
 * Maps WASI filesystem calls to VS Code workspace files.
 * spar-wasm reads .aadl files via std::fs::read_dir(".") and
 * std::fs::read_to_string(path). This shim intercepts those calls
 * and provides workspace .aadl file contents.
 */

/** In-memory file system for the WASM module */
export class VirtualFs {
  private files: Map<string, Uint8Array> = new Map();

  setFile(name: string, content: string) {
    this.files.set(name, new TextEncoder().encode(content));
  }

  clear() {
    this.files.clear();
  }

  getFileNames(): string[] {
    return Array.from(this.files.keys());
  }

  getFile(name: string): Uint8Array | undefined {
    return this.files.get(name);
  }
}

/**
 * Build the full WASI import object for jco-transpiled spar-wasm.
 * Most interfaces are stubs. Only filesystem is real.
 */
export function buildWasiImports(vfs: VirtualFs): Record<string, any> {
  // InputStream: reads bytes from a virtual file
  class InputStream {
    private data: Uint8Array;
    private offset: number = 0;

    constructor(data: Uint8Array) {
      this.data = data;
    }

    blockingRead(len: bigint): Uint8Array {
      const n = Math.min(Number(len), this.data.length - this.offset);
      if (n <= 0) return new Uint8Array(0);
      const chunk = this.data.slice(this.offset, this.offset + n);
      this.offset += n;
      return chunk;
    }

    subscribe() { return new Pollable(); }
    [Symbol.dispose]() {}
  }

  class OutputStream {
    checkWrite(): bigint { return BigInt(65536); }
    write(_data: Uint8Array) {}
    blockingFlush() {}
    subscribe() { return new Pollable(); }
    [Symbol.dispose]() {}
  }

  class Pollable {
    block() {}
    ready(): boolean { return true; }
    [Symbol.dispose]() {}
  }

  class IoError {
    toDebugString(): string { return 'io error'; }
    [Symbol.dispose]() {}
  }

  // DirectoryEntryStream: yields file entries from virtual FS
  class DirectoryEntryStream {
    private entries: string[];
    private index: number = 0;

    constructor(entries: string[]) {
      this.entries = entries;
    }

    readDirectoryEntry(): { name: string; type: string } | undefined {
      if (this.index >= this.entries.length) return undefined;
      const name = this.entries[this.index++];
      return { name, type: 'regular-file' };
    }

    [Symbol.dispose]() {}
  }

  // Descriptor: represents a file or directory
  class Descriptor {
    private name: string;
    private isDir: boolean;

    constructor(name: string, isDir: boolean) {
      this.name = name;
      this.isDir = isDir;
    }

    readViaStream(_offset: bigint): InputStream {
      const data = vfs.getFile(this.name);
      return new InputStream(data ?? new Uint8Array(0));
    }

    readDirectory(): DirectoryEntryStream {
      return new DirectoryEntryStream(vfs.getFileNames());
    }

    stat(): Record<string, any> {
      const data = vfs.getFile(this.name);
      return {
        type: this.isDir ? 'directory' : 'regular-file',
        size: BigInt(data?.length ?? 0),
        dataAccessTimestamp: undefined,
        dataModificationTimestamp: undefined,
        statusChangeTimestamp: undefined,
        linkCount: BigInt(1),
      };
    }

    openAt(_pathFlags: number, path: string, _openFlags: number, _flags: number): Descriptor {
      return new Descriptor(path, false);
    }

    metadataHash(): { upper: bigint; lower: bigint } {
      return { upper: BigInt(0), lower: BigInt(0) };
    }

    metadataHashAt(_flags: number, _path: string): { upper: bigint; lower: bigint } {
      return { upper: BigInt(0), lower: BigInt(0) };
    }

    getFlags(): number { return 0; }
    getType(): string { return this.isDir ? 'directory' : 'regular-file'; }
    writeViaStream(): OutputStream { return new OutputStream(); }
    appendViaStream(): OutputStream { return new OutputStream(); }

    [Symbol.dispose]() {}
  }

  const rootDir = new Descriptor('.', true);
  const stdout = new OutputStream();
  const stderr = new OutputStream();
  const stdin = new InputStream(new Uint8Array(0));

  return {
    'wasi:filesystem/types@0.2.6': {
      Descriptor,
      DirectoryEntryStream,
    },
    'wasi:filesystem/preopens@0.2.6': {
      getDirectories(): [Descriptor, string][] {
        return [[rootDir, '/']];
      },
    },
    'wasi:io/streams@0.2.6': {
      InputStream,
      OutputStream,
    },
    'wasi:io/error@0.2.6': {
      Error: IoError,
    },
    'wasi:io/poll@0.2.6': {
      Pollable,
      poll(_pollables: Pollable[]): Uint32Array { return new Uint32Array([0]); },
    },
    'wasi:cli/environment@0.2.6': {
      getEnvironment(): [string, string][] { return []; },
      getArguments(): string[] { return []; },
    },
    'wasi:cli/exit@0.2.6': {
      exit(_code: { tag: string; val?: number }) {},
    },
    'wasi:cli/stdin@0.2.6': {
      getStdin(): InputStream { return stdin; },
    },
    'wasi:cli/stdout@0.2.6': {
      getStdout(): OutputStream { return stdout; },
    },
    'wasi:cli/stderr@0.2.6': {
      getStderr(): OutputStream { return stderr; },
    },
    'wasi:cli/terminal-input@0.2.6': {
      TerminalInput: class { [Symbol.dispose]() {} },
    },
    'wasi:cli/terminal-output@0.2.6': {
      TerminalOutput: class { [Symbol.dispose]() {} },
    },
    'wasi:cli/terminal-stdin@0.2.6': {
      getTerminalStdin(): undefined { return undefined; },
    },
    'wasi:cli/terminal-stdout@0.2.6': {
      getTerminalStdout(): undefined { return undefined; },
    },
    'wasi:cli/terminal-stderr@0.2.6': {
      getTerminalStderr(): undefined { return undefined; },
    },
    'wasi:clocks/monotonic-clock@0.2.6': {
      now(): bigint { return BigInt(Date.now() * 1_000_000); },
      resolution(): bigint { return BigInt(1_000_000); },
      subscribeInstant(_when: bigint) { return new Pollable(); },
      subscribeDuration(_duration: bigint) { return new Pollable(); },
    },
    'wasi:clocks/wall-clock@0.2.6': {
      now(): { seconds: bigint; nanoseconds: number } {
        const ms = Date.now();
        return { seconds: BigInt(Math.floor(ms / 1000)), nanoseconds: (ms % 1000) * 1_000_000 };
      },
      resolution(): { seconds: bigint; nanoseconds: number } {
        return { seconds: BigInt(0), nanoseconds: 1_000_000 };
      },
    },
    'wasi:random/insecure-seed@0.2.6': {
      insecureSeed(): [bigint, bigint] { return [BigInt(42), BigInt(0)]; },
    },
    'pulseengine:rivet/types@0.1.0': {},
  };
}
