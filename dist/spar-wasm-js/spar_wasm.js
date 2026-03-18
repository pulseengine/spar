"use jco";
import { getEnvironment } from '@aspect-build/jco-shim/cli/environment';
import { exit } from '@aspect-build/jco-shim/cli/exit';
import { getStderr } from '@aspect-build/jco-shim/cli/stderr';
import { getStdin } from '@aspect-build/jco-shim/cli/stdin';
import { getStdout } from '@aspect-build/jco-shim/cli/stdout';
import { TerminalInput } from '@aspect-build/jco-shim/cli/terminal-input';
import { TerminalOutput } from '@aspect-build/jco-shim/cli/terminal-output';
import { getTerminalStderr } from '@aspect-build/jco-shim/cli/terminal-stderr';
import { getTerminalStdin } from '@aspect-build/jco-shim/cli/terminal-stdin';
import { getTerminalStdout } from '@aspect-build/jco-shim/cli/terminal-stdout';
import { now } from '@aspect-build/jco-shim/clocks/monotonic-clock';
import { now as now$1 } from '@aspect-build/jco-shim/clocks/wall-clock';
import { getDirectories } from '@aspect-build/jco-shim/filesystem/preopens';
import { Descriptor, DirectoryEntryStream } from '@aspect-build/jco-shim/filesystem/types';
import { Error as Error$1 } from '@aspect-build/jco-shim/io/error';
import { Pollable } from '@aspect-build/jco-shim/io/poll';
import { InputStream, OutputStream } from '@aspect-build/jco-shim/io/streams';
import { insecureSeed } from '@aspect-build/jco-shim/random/insecure-seed';

const _debugLog = (...args) => {
  if (!globalThis?.process?.env?.JCO_DEBUG) { return; }
  console.debug(...args);
}
const ASYNC_DETERMINISM = 'random';

class GlobalComponentAsyncLowers {
  static map = new Map();
  
  constructor() { throw new Error('GlobalComponentAsyncLowers should not be constructed'); }
  
  static define(args) {
    const { componentIdx, qualifiedImportFn, fn } = args;
    let inner = GlobalComponentAsyncLowers.map.get(componentIdx);
    if (!inner) {
      inner = new Map();
      GlobalComponentAsyncLowers.map.set(componentIdx, inner);
    }
    
    inner.set(qualifiedImportFn, fn);
  }
  
  static lookup(componentIdx, qualifiedImportFn) {
    let inner = GlobalComponentAsyncLowers.map.get(componentIdx);
    if (!inner) {
      inner = new Map();
      GlobalComponentAsyncLowers.map.set(componentIdx, inner);
    }
    
    const found = inner.get(qualifiedImportFn);
    if (found) { return found; }
    
    // In some cases, async lowers are *not* host provided, and
    // but contain/will call an async function in the host.
    //
    // One such case is `stream.write`/`stream.read` trampolines which are
    // actually re-exported through a patch up container *before*
    // they call the relevant async host trampoline.
    //
    // So the path of execution from a component export would be:
    //
    // async guest export --> stream.write import (host wired) -> guest export (patch component) -> async host trampoline
    //
    // On top of all this, the trampoline that is eventually called is async,
    // so we must await the patched guest export call.
    //
    if (qualifiedImportFn.includes("[stream-write-") || qualifiedImportFn.includes("[stream-read-")) {
      return async (...args) => {
        const [originalFn, ...params] = args;
        return await originalFn(...params);
      };
    }
    
    // All other cases can call the registered function directly
    return (...args) => {
      const [originalFn, ...params] = args;
      return originalFn(...params);
    };
  }
}

class GlobalAsyncParamLowers {
  static map = new Map();
  
  static generateKey(args) {
    const { componentIdx, iface, fnName } = args;
    if (componentIdx === undefined) { throw new TypeError("missing component idx"); }
    if (iface === undefined) { throw new TypeError("missing iface name"); }
    if (fnName === undefined) { throw new TypeError("missing function name"); }
    return `${componentIdx}-${iface}-${fnName}`;
  }
  
  static define(args) {
    const { componentIdx, iface, fnName, fn } = args;
    if (!fn) { throw new TypeError('missing function'); }
    const key = GlobalAsyncParamLowers.generateKey(args);
    GlobalAsyncParamLowers.map.set(key, fn);
  }
  
  static lookup(args) {
    const { componentIdx, iface, fnName } = args;
    const key = GlobalAsyncParamLowers.generateKey(args);
    return GlobalAsyncParamLowers.map.get(key);
  }
}

class GlobalComponentMemories {
  static map = new Map();
  
  constructor() { throw new Error('GlobalComponentMemories should not be constructed'); }
  
  static save(args) {
    const { idx, componentIdx, memory } = args;
    let inner = GlobalComponentMemories.map.get(componentIdx);
    if (!inner) {
      inner = [];
      GlobalComponentMemories.map.set(componentIdx, inner);
    }
    inner.push({ memory, idx });
  }
  
  static getMemoriesForComponentIdx(componentIdx) {
    const metas = GlobalComponentMemories.map.get(componentIdx);
    return metas.map(meta => meta.memory);
  }
  
  static getMemory(componentIdx, idx) {
    const metas = GlobalComponentMemories.map.get(componentIdx);
    return metas.find(meta => meta.idx === idx)?.memory;
  }
}

class RepTable {
  #data = [0, null];
  #target;
  
  constructor(args) {
    this.target = args?.target;
  }
  
  insert(val) {
    _debugLog('[RepTable#insert()] args', { val, target: this.target });
    const freeIdx = this.#data[0];
    if (freeIdx === 0) {
      this.#data.push(val);
      this.#data.push(null);
      return (this.#data.length >> 1) - 1;
    }
    this.#data[0] = this.#data[freeIdx << 1];
    const placementIdx = freeIdx << 1;
    this.#data[placementIdx] = val;
    this.#data[placementIdx + 1] = null;
    return freeIdx;
  }
  
  get(rep) {
    _debugLog('[RepTable#get()] args', { rep, target: this.target });
    const baseIdx = rep << 1;
    const val = this.#data[baseIdx];
    return val;
  }
  
  contains(rep) {
    _debugLog('[RepTable#contains()] args', { rep, target: this.target });
    const baseIdx = rep << 1;
    return !!this.#data[baseIdx];
  }
  
  remove(rep) {
    _debugLog('[RepTable#remove()] args', { rep, target: this.target });
    if (this.#data.length === 2) { throw new Error('invalid'); }
    
    const baseIdx = rep << 1;
    const val = this.#data[baseIdx];
    if (val === 0) { throw new Error('invalid resource rep (cannot be 0)'); }
    
    this.#data[baseIdx] = this.#data[0];
    this.#data[0] = rep;
    
    return val;
  }
  
  clear() {
    _debugLog('[RepTable#clear()] args', { rep, target: this.target });
    this.#data = [0, null];
  }
}
const _coinFlip = () => { return Math.random() > 0.5; };
let SCOPE_ID = 0;
const I32_MIN = -2_147_483_648;
const I32_MAX = 2_147_483_647;
const _typeCheckValidI32 = (n) => typeof n === 'number' && n >= I32_MIN && n <= I32_MAX;

const _typeCheckAsyncFn= (f) => {
  return f instanceof ASYNC_FN_CTOR;
};

const ASYNC_FN_CTOR = (async () => {}).constructor;
const ASYNC_CURRENT_TASK_IDS = [];
const ASYNC_CURRENT_COMPONENT_IDXS = [];

function unpackCallbackResult(result) {
  _debugLog('[unpackCallbackResult()] args', { result });
  if (!(_typeCheckValidI32(result))) { throw new Error('invalid callback return value [' + result + '], not a valid i32'); }
  const eventCode = result & 0xF;
  if (eventCode < 0 || eventCode > 3) {
    throw new Error('invalid async return value [' + eventCode + '], outside callback code range');
  }
  if (result < 0 || result >= 2**32) { throw new Error('invalid callback result'); }
  // TODO: table max length check?
  const waitableSetRep = result >> 4;
  return [eventCode, waitableSetRep];
}

function promiseWithResolvers() {
  if (Promise.withResolvers) {
    return Promise.withResolvers();
  } else {
    let resolve;
    let reject;
    const promise = new Promise((res, rej) => {
      resolve = res;
      reject = rej;
    });
    return { promise, resolve, reject };
  }
}

function _prepareCall(
memoryIdx,
getMemoryFn,
startFn,
returnFn,
callerInstanceIdx,
calleeInstanceIdx,
taskReturnTypeIdx,
isCalleeAsyncInt,
stringEncoding,
resultCountOrAsync,
) {
  _debugLog('[_prepareCall()]', {
    callerInstanceIdx,
    calleeInstanceIdx,
    taskReturnTypeIdx,
    isCalleeAsyncInt,
    stringEncoding,
    resultCountOrAsync,
  });
  const argArray = [...arguments];
  
  // Since Rust will happily pass large u32s over, resultCountOrAsync should be one of:
  // (a) u32 max size     => callee is async fn with no result
  // (b) u32 max size - 1 => callee is async fn with result
  // (c) any other value  => callee is sync with the given result count
  //
  // Due to JS handling the value as 2s complement, the `resultCountOrAsync` ends up being:
  // (a) -1 as u32 max size
  // (b) -2 as u32 max size - 1
  // (c) x
  //
  // Due to JS mishandling the value as 2s complement, the actual values we get are:
  // see. https://github.com/wasm-bindgen/wasm-bindgen/issues/1388
  let isAsync = false;
  let hasResultPointer = false;
  if (resultCountOrAsync === -1) {
    isAsync = true;
    hasResultPointer = false;
  } else if (resultCountOrAsync === -2) {
    isAsync = true;
    hasResultPointer = true;
  }
  
  const currentCallerTaskMeta = getCurrentTask(callerInstanceIdx);
  if (!currentCallerTaskMeta) {
    throw new Error('invalid/missing current task for caller during prepare call');
  }
  
  const currentCallerTask = currentCallerTaskMeta.task;
  if (!currentCallerTask) {
    throw new Error('unexpectedly missing task in meta for caller during prepare call');
  }
  
  if (currentCallerTask.componentIdx() !== callerInstanceIdx) {
    throw new Error(`task component idx [${ currentCallerTask.componentIdx() }] !== [${ callerInstanceIdx }] (callee ${ calleeInstanceIdx })`);
  }
  
  let getCalleeParamsFn;
  let resultPtr = null;
  if (hasResultPointer) {
    const directParamsArr = argArray.slice(11);
    getCalleeParamsFn = () => directParamsArr;
    resultPtr = argArray[10];
  } else {
    const directParamsArr = argArray.slice(10);
    getCalleeParamsFn = () => directParamsArr;
  }
  
  let encoding;
  switch (stringEncoding) {
    case 0:
    encoding = 'utf8';
    break;
    case 1:
    encoding = 'utf16';
    break;
    case 2:
    encoding = 'compact-utf16';
    break;
    default:
    throw new Error(`unrecognized string encoding enum [${stringEncoding}]`);
  }
  
  const [newTask, newTaskID] = createNewCurrentTask({
    componentIdx: calleeInstanceIdx,
    isAsync: isCalleeAsyncInt !== 0,
    getCalleeParamsFn,
    // TODO: find a way to pass the import name through here
    entryFnName: 'task/' + currentCallerTask.id() + '/new-prepare-task',
    stringEncoding,
  });
  
  const subtask = currentCallerTask.createSubtask({
    componentIdx: callerInstanceIdx,
    parentTask: currentCallerTask,
    childTask: newTask,
    callMetadata: {
      memory: getMemoryFn(),
      memoryIdx,
      resultPtr,
      returnFn,
      startFn,
    }
  });
  
  newTask.setParentSubtask(subtask);
  // NOTE: This isn't really a return memory idx for the caller, it's for checking
  // against the task.return (which will be called from the callee)
  newTask.setReturnMemoryIdx(memoryIdx);
}

function _asyncStartCall(args, callee, paramCount, resultCount, flags) {
  const { getCallbackFn, callbackIdx, getPostReturnFn, postReturnIdx } = args;
  _debugLog('[_asyncStartCall()] args', args);
  
  const taskMeta = getCurrentTask(ASYNC_CURRENT_COMPONENT_IDXS.at(-1), ASYNC_CURRENT_TASK_IDS.at(-1));
  if (!taskMeta) { throw new Error('invalid/missing current async task meta during prepare call'); }
  
  const argArray = [...arguments];
  
  // NOTE: at this point we know the current task is the one that was started
  // in PrepareCall, so we *should* be able to pop it back off and be left with
  // the previous task
  const preparedTask = taskMeta.task;
  if (!preparedTask) { throw new Error('unexpectedly missing task in task meta during prepare call'); }
  
  if (resultCount < 0 || resultCount > 1) { throw new Error('invalid/unsupported result count'); }
  
  const callbackFnName = 'callback_' + callbackIdx;
  const callbackFn = getCallbackFn();
  preparedTask.setCallbackFn(callbackFn, callbackFnName);
  preparedTask.setPostReturnFn(getPostReturnFn());
  
  const subtask = preparedTask.getParentSubtask();
  
  if (resultCount < 0 || resultCount > 1) { throw new Error(`unsupported result count [${ resultCount }]`); }
  
  const params = preparedTask.getCalleeParams();
  if (paramCount !== params.length) {
    throw new Error(`unexpected callee param count [${ params.length }], _asyncStartCall invocation expected [${ paramCount }]`);
  }
  
  subtask.setOnProgressFn(() => {
    subtask.setPendingEventFn(() => {
      if (subtask.resolved()) { subtask.deliverResolve(); }
      return {
        code: ASYNC_EVENT_CODE.SUBTASK,
        index: rep,
        result: subtask.getStateNumber(),
      }
    });
  });
  
  const subtaskState = subtask.getStateNumber();
  if (subtaskState < 0 || subtaskState > 2**5) {
    throw new Error('invalid subtask state, out of valid range');
  }
  
  const callerComponentState = getOrCreateAsyncState(subtask.componentIdx());
  const rep = callerComponentState.subtasks.insert(subtask);
  subtask.setRep(rep);
  
  const calleeComponentState = getOrCreateAsyncState(preparedTask.componentIdx());
  const calleeBackpressure = calleeComponentState.hasBackpressure();
  
  // Set up a handler on subtask completion to lower results from the call into the caller's memory region.
  //
  // NOTE: during fused guest->guest calls this handler is triggered, but does not actually perform
  // lowering manually, as fused modules provider helper functions that can
  subtask.registerOnResolveHandler((res) => {
    _debugLog('[_asyncStartCall()] handling subtask result', { res, subtaskID: subtask.id() });
    let subtaskCallMeta = subtask.getCallMetadata();
    
    // NOTE: in the case of guest -> guest async calls, there may be no memory/realloc present,
    // as the host will intermediate the value storage/movement between calls.
    //
    // We can simply take the value and lower it as a parameter
    if (subtaskCallMeta.memory || subtaskCallMeta.realloc) {
      throw new Error("call metadata unexpectedly contains memory/realloc for guest->guest call");
    }
    
    const callerTask = subtask.getParentTask();
    const calleeTask = preparedTask;
    const callerMemoryIdx = callerTask.getReturnMemoryIdx();
    const callerComponentIdx = callerTask.componentIdx();
    
    // If a helper function was provided we are likely in a fused guest->guest call,
    // and the result will be delivered (lift/lowered) via helper function
    if (subtaskCallMeta.returnFn) {
      _debugLog('[_asyncStartCall()] return function present while ahndling subtask result, returning early (skipping lower)');
      return;
    }
    
    // If there is no where to lower the results, exit early
    if (!subtaskCallMeta.resultPtr) {
      _debugLog('[_asyncStartCall()] no result ptr during subtask result handling, returning early (skipping lower)');
      return;
    }
    
    let callerMemory;
    if (callerMemoryIdx) {
      callerMemory = GlobalComponentMemories.getMemory(callerComponentIdx, callerMemoryIdx);
    } else {
      const callerMemories = GlobalComponentMemories.getMemoriesForComponentIdx(callerComponentIdx);
      if (callerMemories.length != 1) { throw new Error(`unsupported amount of caller memories`); }
      callerMemory = callerMemories[0];
    }
    
    if (!callerMemory) {
      throw new Error(`missing memory for to guest->guest call result (subtask [${subtask.id()}])`);
    }
    
    const lowerFns = calleeTask.getReturnLowerFns();
    if (!lowerFns || lowerFns.length === 0) {
      throw new Error(`missing result lower metadata for guest->guests call (subtask [${subtask.id()}])`);
    }
    
    if (lowerFns.length !== 1) {
      throw new Error(`only single result supported for guest->guest calls (subtask [${subtask.id()}])`);
    }
    
    lowerFns[0]({
      realloc: undefined,
      memory: callerMemory,
      vals: [res],
      storagePtr: subtaskCallMeta.resultPtr,
      componentIdx: callerComponentIdx
    });
    
  });
  
  // Build call params
  const subtaskCallMeta = subtask.getCallMetadata();
  let startFnParams = [];
  let calleeParams = [];
  if (subtaskCallMeta.startFn && subtaskCallMeta.resultPtr) {
    // If we're using a fused component start fn  and a result pointer is present,
    // then we need to pass the result pointer and other params to the start fn
    startFnParams.push(subtaskCallMeta.resultPtr, ...params);
  } else {
    // if not we need to pass params to the callee instead
    startFnParams.push(...params);
    calleeParams.push(...params);
  }
  
  preparedTask.registerOnResolveHandler((res) => {
    _debugLog('[_asyncStartCall()] signaling subtask completion due to task completion', {
      childTaskID: preparedTask.id(),
      subtaskID: subtask.id(),
      parentTaskID: subtask.getParentTask().id(),
    });
    subtask.onResolve(res);
  });
  
  // TODO(fix): start fns sometimes produce results, how should they be used?
  // the result should theoretically be used for flat lowering, but fused components do
  // this automatically!
  subtask.onStart({ startFnParams });
  
  _debugLog("[_asyncStartCall()] initial call", {
    task: preparedTask.id(),
    subtaskID: subtask.id(),
    calleeFnName: callee.name,
  });
  
  const callbackResult = callee.apply(null, calleeParams);
  
  _debugLog("[_asyncStartCall()] after initial call", {
    task: preparedTask.id(),
    subtaskID: subtask.id(),
    calleeFnName: callee.name,
  });
  
  const doSubtaskResolve = () => {
    subtask.deliverResolve();
  };
  
  // If a single call resolved the subtask and there is no backpressure in the guest,
  // we can return immediately
  if (subtask.resolved() && !calleeBackpressure) {
    _debugLog("[_asyncStartCall()] instantly resolved", {
      calleeComponentIdx: preparedTask.componentIdx(),
      task: preparedTask.id(),
      subtaskID: subtask.id(),
      callerComponentIdx: subtask.componentIdx(),
    });
    
    // If a fused component return function was specified for the subtask,
    // we've likely already called it during resolution of the task.
    //
    // In this case, we do not want to actually return 2 AKA "RETURNED",
    // but the normal started task state, because the fused component expects to get
    // the waitable + the original subtask state (0 AKA "STARTING")
    //
    if (subtask.getCallMetadata().returnFn) {
      return Number(subtask.waitableRep()) << 4 | subtaskState;
    }
    
    doSubtaskResolve();
    return AsyncSubtask.State.RETURNED;
  }
  
  // Start the (event) driver loop that will resolve the task
  new Promise(async (resolve, reject) => {
    if (subtask.resolved() && calleeBackpressure) {
      await calleeComponentState.waitForBackpressure();
      
      _debugLog("[_asyncStartCall()] instantly resolved after cleared backpressure", {
        calleeComponentIdx: preparedTask.componentIdx(),
        task: preparedTask.id(),
        subtaskID: subtask.id(),
        callerComponentIdx: subtask.componentIdx(),
      });
      return;
    }
    
    const started = await preparedTask.enter();
    if (!started) {
      _debugLog('[_asyncStartCall()] task failed early', {
        taskID: preparedTask.id(),
        subtaskID: subtask.id(),
      });
      throw new Error("task failed to start");
      return;
    }
    
    // TODO: retrieve/pass along actual fn name the callback corresponds to
    // (at least something like `<lifted fn name>_callback`)
    const fnName = [
    '<task ',
    subtask.parentTaskID(),
    '/subtask ',
    subtask.id(),
    '/task ',
    preparedTask.id(),
    '>',
    ].join("");
    
    try {
      _debugLog("[_asyncStartCall()] starting driver loop", { fnName, componentIdx: preparedTask.componentIdx(), });
      await _driverLoop({
        componentState: calleeComponentState,
        task: preparedTask,
        fnName,
        isAsync: true,
        callbackResult,
        resolve,
        reject
      });
    } catch (err) {
      _debugLog("[AsyncStartCall] drive loop call failure", { err });
    }
    
  });
  
  return Number(subtask.waitableRep()) << 4 | subtaskState;
}

function _syncStartCall(callbackIdx) {
  _debugLog('[_syncStartCall()] args', { callbackIdx });
  throw new Error('synchronous start call not implemented!');
}

let dv = new DataView(new ArrayBuffer());
const dataView = mem => dv.buffer === mem.buffer ? dv : dv = new DataView(mem.buffer);

const toUint64 = val => BigInt.asUintN(64, BigInt(val));

function toUint32(val) {
  return val >>> 0;
}
const TEXT_DECODER_UTF8 = new TextDecoder();
const TEXT_ENCODER_UTF8 = new TextEncoder();

function _utf8AllocateAndEncode(s, realloc, memory) {
  if (typeof s !== 'string') {
    throw new TypeError('expected a string, received [' + typeof s + ']');
  }
  if (s.length === 0) { return { ptr: 1, len: 0 }; }
  let buf = TEXT_ENCODER_UTF8.encode(s);
  let ptr = realloc(0, 0, 1, buf.length);
  new Uint8Array(memory.buffer).set(buf, ptr);
  return { ptr, len: buf.length, codepoints: [...s].length };
}


const T_FLAG = 1 << 30;

function rscTableCreateOwn(table, rep) {
  const free = table[0] & ~T_FLAG;
  if (free === 0) {
    table.push(0);
    table.push(rep | T_FLAG);
    return (table.length >> 1) - 1;
  }
  table[0] = table[free << 1];
  table[free << 1] = 0;
  table[(free << 1) + 1] = rep | T_FLAG;
  return free;
}

function rscTableRemove(table, handle) {
  const scope = table[handle << 1];
  const val = table[(handle << 1) + 1];
  const own = (val & T_FLAG) !== 0;
  const rep = val & ~T_FLAG;
  if (val === 0 || (scope & T_FLAG) !== 0) {
    throw new TypeError("Invalid handle");
  }
  table[handle << 1] = table[0] | T_FLAG;
  table[0] = handle | T_FLAG;
  return { rep, scope, own };
}

let curResourceBorrows = [];

function getCurrentTask(componentIdx) {
  if (componentIdx === undefined || componentIdx === null) {
    throw new Error('missing/invalid component instance index [' + componentIdx + '] while getting current task');
  }
  const tasks = ASYNC_TASKS_BY_COMPONENT_IDX.get(componentIdx);
  if (tasks === undefined) { return undefined; }
  if (tasks.length === 0) { return undefined; }
  return tasks[tasks.length - 1];
}

function createNewCurrentTask(args) {
  _debugLog('[createNewCurrentTask()] args', args);
  const {
    componentIdx,
    isAsync,
    entryFnName,
    parentSubtaskID,
    callbackFnName,
    getCallbackFn,
    getParamsFn,
    stringEncoding,
    errHandling,
    getCalleeParamsFn,
    resultPtr,
    callingWasmExport,
  } = args;
  if (componentIdx === undefined || componentIdx === null) {
    throw new Error('missing/invalid component instance index while starting task');
  }
  const taskMetas = ASYNC_TASKS_BY_COMPONENT_IDX.get(componentIdx);
  const callbackFn = getCallbackFn ? getCallbackFn() : null;
  
  const newTask = new AsyncTask({
    componentIdx,
    isAsync,
    entryFnName,
    callbackFn,
    callbackFnName,
    stringEncoding,
    getCalleeParamsFn,
    resultPtr,
    errHandling,
  });
  
  const newTaskID = newTask.id();
  const newTaskMeta = { id: newTaskID, componentIdx, task: newTask };
  
  ASYNC_CURRENT_TASK_IDS.push(newTaskID);
  ASYNC_CURRENT_COMPONENT_IDXS.push(componentIdx);
  
  if (!taskMetas) {
    ASYNC_TASKS_BY_COMPONENT_IDX.set(componentIdx, [newTaskMeta]);
  } else {
    taskMetas.push(newTaskMeta);
  }
  
  return [newTask, newTaskID];
}

function endCurrentTask(componentIdx, taskID) {
  componentIdx ??= ASYNC_CURRENT_COMPONENT_IDXS.at(-1);
  taskID ??= ASYNC_CURRENT_TASK_IDS.at(-1);
  _debugLog('[endCurrentTask()] args', { componentIdx, taskID });
  
  if (componentIdx === undefined || componentIdx === null) {
    throw new Error('missing/invalid component instance index while ending current task');
  }
  
  const tasks = ASYNC_TASKS_BY_COMPONENT_IDX.get(componentIdx);
  if (!tasks || !Array.isArray(tasks)) {
    throw new Error('missing/invalid tasks for component instance while ending task');
  }
  if (tasks.length == 0) {
    throw new Error('no current task(s) for component instance while ending task');
  }
  
  if (taskID) {
    const last = tasks[tasks.length - 1];
    if (last.id !== taskID) {
      // throw new Error('current task does not match expected task ID');
      return;
    }
  }
  
  ASYNC_CURRENT_TASK_IDS.pop();
  ASYNC_CURRENT_COMPONENT_IDXS.pop();
  
  const taskMeta = tasks.pop();
  return taskMeta.task;
}
const ASYNC_TASKS_BY_COMPONENT_IDX = new Map();

class AsyncTask {
  static _ID = 0n;
  
  static State = {
    INITIAL: 'initial',
    CANCELLED: 'cancelled',
    CANCEL_PENDING: 'cancel-pending',
    CANCEL_DELIVERED: 'cancel-delivered',
    RESOLVED: 'resolved',
  }
  
  static BlockResult = {
    CANCELLED: 'block.cancelled',
    NOT_CANCELLED: 'block.not-cancelled',
  }
  
  #id;
  #componentIdx;
  #state;
  #isAsync;
  #entryFnName = null;
  #subtasks = [];
  
  #onResolveHandlers = [];
  #completionPromise = null;
  
  #memoryIdx = null;
  
  #callbackFn = null;
  #callbackFnName = null;
  
  #postReturnFn = null;
  
  #getCalleeParamsFn = null;
  
  #stringEncoding = null;
  
  #parentSubtask = null;
  
  #needsExclusiveLock = false;
  
  #errHandling;
  
  #backpressurePromise;
  #backpressureWaiters = 0n;
  
  #returnLowerFns = null;
  
  cancelled = false;
  requested = false;
  alwaysTaskReturn = false;
  
  returnCalls =  0;
  storage = [0, 0];
  borrowedHandles = {};
  
  awaitableResume = null;
  awaitableCancel = null;
  
  constructor(opts) {
    this.#id = ++AsyncTask._ID;
    
    if (opts?.componentIdx === undefined) {
      throw new TypeError('missing component id during task creation');
    }
    this.#componentIdx = opts.componentIdx;
    
    this.#state = AsyncTask.State.INITIAL;
    this.#isAsync = opts?.isAsync ?? false;
    this.#entryFnName = opts.entryFnName;
    
    const {
      promise: completionPromise,
      resolve: resolveCompletionPromise,
      reject: rejectCompletionPromise,
    } = promiseWithResolvers();
    this.#completionPromise = completionPromise;
    
    this.#onResolveHandlers.push((results) => {
      resolveCompletionPromise(results);
    })
    
    if (opts.callbackFn) { this.#callbackFn = opts.callbackFn; }
    if (opts.callbackFnName) { this.#callbackFnName = opts.callbackFnName; }
    
    if (opts.getCalleeParamsFn) { this.#getCalleeParamsFn = opts.getCalleeParamsFn; }
    
    if (opts.stringEncoding) { this.#stringEncoding = opts.stringEncoding; }
    
    if (opts.parentSubtask) { this.#parentSubtask = opts.parentSubtask; }
    
    this.#needsExclusiveLock = this.isSync() || !this.hasCallback();
    
    if (opts.errHandling) { this.#errHandling = opts.errHandling; }
  }
  
  taskState() { return this.#state; }
  id() { return this.#id; }
  componentIdx() { return this.#componentIdx; }
  isAsync() { return this.#isAsync; }
  entryFnName() { return this.#entryFnName; }
  completionPromise() { return this.#completionPromise; }
  
  isAsync() { return this.#isAsync; }
  isSync() { return !this.isAsync(); }
  
  getErrHandling() { return this.#errHandling; }
  
  hasCallback() { return this.#callbackFn !== null; }
  
  setReturnMemoryIdx(idx) { this.#memoryIdx = idx; }
  getReturnMemoryIdx() { return this.#memoryIdx; }
  
  setReturnLowerFns(fns) { this.#returnLowerFns = fns; }
  getReturnLowerFns() { return this.#returnLowerFns; }
  
  setParentSubtask(subtask) {
    if (!subtask || !(subtask instanceof AsyncSubtask)) { return }
    if (this.#parentSubtask) { throw new Error('parent subtask can only be set once'); }
    this.#parentSubtask = subtask;
  }
  
  getParentSubtask() { return this.#parentSubtask; }
  
  // TODO(threads): this is very inefficient, we can pass along a root task,
  // and ideally do not need this once thread support is in place
  getRootTask() {
    let currentSubtask = this.getParentSubtask();
    let task = this;
    while (currentSubtask) {
      task = currentSubtask.getParentTask();
      currentSubtask = task.getParentSubtask();
    }
    return task;
  }
  
  setPostReturnFn(f) {
    if (!f) { return; }
    if (this.#postReturnFn) { throw new Error('postReturn fn can only be set once'); }
    this.#postReturnFn = f;
  }
  
  setCallbackFn(f, name) {
    if (!f) { return; }
    if (this.#callbackFn) { throw new Error('callback fn can only be set once'); }
    this.#callbackFn = f;
    this.#callbackFnName = name;
  }
  
  getCallbackFnName() {
    if (!this.#callbackFnName) { return undefined; }
    return this.#callbackFnName;
  }
  
  runCallbackFn(...args) {
    if (!this.#callbackFn) { throw new Error('on callback function has been set for task'); }
    return this.#callbackFn.apply(null, args);
  }
  
  getCalleeParams() {
    if (!this.#getCalleeParamsFn) { throw new Error('missing/invalid getCalleeParamsFn'); }
    return this.#getCalleeParamsFn();
  }
  
  mayEnter(task) {
    const cstate = getOrCreateAsyncState(this.#componentIdx);
    if (cstate.hasBackpressure()) {
      _debugLog('[AsyncTask#mayEnter()] disallowed due to backpressure', { taskID: this.#id });
      return false;
    }
    if (!cstate.callingSyncImport()) {
      _debugLog('[AsyncTask#mayEnter()] disallowed due to sync import call', { taskID: this.#id });
      return false;
    }
    const callingSyncExportWithSyncPending = cstate.callingSyncExport && !task.isAsync;
    if (!callingSyncExportWithSyncPending) {
      _debugLog('[AsyncTask#mayEnter()] disallowed due to sync export w/ sync pending', { taskID: this.#id });
      return false;
    }
    return true;
  }
  
  async enter() {
    _debugLog('[AsyncTask#enter()] args', { taskID: this.#id });
    const cstate = getOrCreateAsyncState(this.#componentIdx);
    
    if (this.isSync()) { return true; }
    
    if (cstate.hasBackpressure()) {
      cstate.addBackpressureWaiter();
      
      const result = await this.waitUntil({
        readyFn: () => !cstate.hasBackpressure(),
        cancellable: true,
      });
      
      cstate.removeBackpressureWaiter();
      
      if (result === AsyncTask.BlockResult.CANCELLED) {
        this.cancel();
        return false;
      }
    }
    
    if (this.needsExclusiveLock()) { cstate.exclusiveLock(); }
    
    return true;
  }
  
  isRunning() {
    return this.#state !== AsyncTask.State.RESOLVED;
  }
  
  async waitUntil(opts) {
    const { readyFn, waitableSetRep, cancellable } = opts;
    _debugLog('[AsyncTask#waitUntil()] args', { taskID: this.#id, waitableSetRep, cancellable });
    
    const state = getOrCreateAsyncState(this.#componentIdx);
    const wset = state.waitableSets.get(waitableSetRep);
    
    let event;
    
    wset.incrementNumWaiting();
    
    const keepGoing = await this.suspendUntil({
      readyFn: () => {
        const hasPendingEvent = wset.hasPendingEvent();
        return readyFn() && hasPendingEvent;
      },
      cancellable,
    });
    
    if (keepGoing) {
      event = wset.getPendingEvent();
    } else {
      event = {
        code: ASYNC_EVENT_CODE.TASK_CANCELLED,
        index: 0,
        result: 0,
      };
    }
    
    wset.decrementNumWaiting();
    
    return event;
  }
  
  async onBlock(awaitable) {
    _debugLog('[AsyncTask#onBlock()] args', { taskID: this.#id, awaitable });
    if (!(awaitable instanceof Awaitable)) {
      throw new Error('invalid awaitable during onBlock');
    }
    
    // Build a promise that this task can await on which resolves when it is awoken
    const { promise, resolve, reject } = promiseWithResolvers();
    this.awaitableResume = () => {
      _debugLog('[AsyncTask] resuming after onBlock', { taskID: this.#id });
      resolve();
    };
    this.awaitableCancel = (err) => {
      _debugLog('[AsyncTask] rejecting after onBlock', { taskID: this.#id, err });
      reject(err);
    };
    
    // Park this task/execution to be handled later
    const state = getOrCreateAsyncState(this.#componentIdx);
    state.parkTaskOnAwaitable({ awaitable, task: this });
    
    try {
      await promise;
      return AsyncTask.BlockResult.NOT_CANCELLED;
    } catch (err) {
      // rejection means task cancellation
      return AsyncTask.BlockResult.CANCELLED;
    }
  }
  
  async asyncOnBlock(awaitable) {
    _debugLog('[AsyncTask#asyncOnBlock()] args', { taskID: this.#id, awaitable });
    if (!(awaitable instanceof Awaitable)) {
      throw new Error('invalid awaitable during onBlock');
    }
    // TODO: watch for waitable AND cancellation
    // TODO: if it WAS cancelled:
    // - return true
    // - only once per subtask
    // - do not wait on the scheduler
    // - control flow should go to the subtask (only once)
    // - Once subtask blocks/resolves, reqlinquishControl() will tehn resolve request_cancel_end (without scheduler lock release)
    // - control flow goes back to request_cancel
    //
    // Subtask cancellation should work similarly to an async import call -- runs sync up until
    // the subtask blocks or resolves
    //
    throw new Error('AsyncTask#asyncOnBlock() not yet implemented');
  }
  
  async yieldUntil(opts) {
    const { readyFn, cancellable } = opts;
    _debugLog('[AsyncTask#yieldUntil()] args', { taskID: this.#id, cancellable });
    
    const keepGoing = await this.suspendUntil({ readyFn, cancellable });
    if (!keepGoing) {
      return {
        code: ASYNC_EVENT_CODE.TASK_CANCELLED,
        index: 0,
        result: 0,
      };
    }
    
    return {
      code: ASYNC_EVENT_CODE.NONE,
      index: 0,
      result: 0,
    };
  }
  
  async suspendUntil(opts) {
    const { cancellable, readyFn } = opts;
    _debugLog('[AsyncTask#suspendUntil()] args', { cancellable });
    
    const pendingCancelled = this.deliverPendingCancel({ cancellable });
    if (pendingCancelled) { return false; }
    
    const completed = await this.immediateSuspendUntil({ readyFn, cancellable });
    return completed;
  }
  
  // TODO(threads): equivalent to thread.suspend_until()
  async immediateSuspendUntil(opts) {
    const { cancellable, readyFn } = opts;
    _debugLog('[AsyncTask#immediateSuspendUntil()] args', { cancellable, readyFn });
    
    const ready = readyFn();
    if (ready && !ASYNC_DETERMINISM && _coinFlip()) {
      return true;
    }
    
    const cstate = getOrCreateAsyncState(this.#componentIdx);
    cstate.addPendingTask(this);
    
    const keepGoing = await this.immediateSuspend({ cancellable, readyFn });
    return keepGoing;
  }
  
  async immediateSuspend(opts) { // NOTE: equivalent to thread.suspend()
  // TODO(threads): store readyFn on the thread
  const { cancellable, readyFn } = opts;
  _debugLog('[AsyncTask#immediateSuspend()] args', { cancellable, readyFn });
  
  const pendingCancelled = this.deliverPendingCancel({ cancellable });
  if (pendingCancelled) { return false; }
  
  const cstate = getOrCreateAsyncState(this.#componentIdx);
  
  // TODO(fix): update this to tick until there is no more action to take.
  setTimeout(() => cstate.tick(), 0);
  
  const taskWait = await cstate.suspendTask({ task: this, readyFn });
  const keepGoing = await taskWait;
  return keepGoing;
}

deliverPendingCancel(opts) {
  const { cancellable } = opts;
  _debugLog('[AsyncTask#deliverPendingCancel()] args', { cancellable });
  
  if (cancellable && this.#state === AsyncTask.State.PENDING_CANCEL) {
    this.#state = Task.State.CANCEL_DELIVERED;
    return true;
  }
  
  return false;
}

isCancelled() { return this.cancelled }

cancel() {
  _debugLog('[AsyncTask#cancel()] args', { });
  if (!this.taskState() !== AsyncTask.State.CANCEL_DELIVERED) {
    throw new Error(`(component [${this.#componentIdx}]) task [${this.#id}] invalid task state for cancellation`);
  }
  if (this.borrowedHandles.length > 0) { throw new Error('task still has borrow handles'); }
  this.cancelled = true;
  this.onResolve(new Error('cancelled'));
  this.#state = AsyncTask.State.RESOLVED;
}

onResolve(taskValue) {
  for (const f of this.#onResolveHandlers) {
    try {
      f(taskValue);
    } catch (err) {
      console.error("error during task resolve handler", err);
      throw err;
    }
  }
  
  if (this.#postReturnFn) {
    _debugLog('[AsyncTask#onResolve()] running post return ', {
      componentIdx: this.#componentIdx,
      taskID: this.#id,
    });
    this.#postReturnFn();
  }
}

registerOnResolveHandler(f) {
  this.#onResolveHandlers.push(f);
}

resolve(results) {
  _debugLog('[AsyncTask#resolve()] args', {
    results,
    componentIdx: this.#componentIdx,
    taskID: this.#id,
  });
  
  if (this.#state === AsyncTask.State.RESOLVED) {
    throw new Error(`(component [${this.#componentIdx}]) task [${this.#id}]  is already resolved (did you forget to wait for an import?)`);
  }
  if (this.borrowedHandles.length > 0) { throw new Error('task still has borrow handles'); }
  switch (results.length) {
    case 0:
    this.onResolve(undefined);
    break;
    case 1:
    this.onResolve(results[0]);
    break;
    default:
    throw new Error('unexpected number of results');
  }
  this.#state = AsyncTask.State.RESOLVED;
}

exit() {
  _debugLog('[AsyncTask#exit()] args', { });
  
  // TODO: ensure there is only one task at a time (scheduler.lock() functionality)
  if (this.#state !== AsyncTask.State.RESOLVED) {
    // TODO(fix): only fused, manually specified post returns seem to break this invariant,
    // as the TaskReturn trampoline is not activated it seems.
    //
    // see: test/p3/ported/wasmtime/component-async/post-return.js
    //
    // We *should* be able to upgrade this to be more strict and throw at some point,
    // which may involve rewriting the upstream test to surface task return manually somehow.
    //
    //throw new Error(`(component [${this.#componentIdx}]) task [${this.#id}] exited without resolution`);
    _debugLog('[AsyncTask#exit()] task exited without resolution', {
      componentIdx: this.#componentIdx,
      taskID: this.#id,
      subtask: this.getParentSubtask(),
      subtaskID: this.getParentSubtask()?.id(),
    });
    this.#state = AsyncTask.State.RESOLVED;
  }
  
  if (this.borrowedHandles > 0) {
    throw new Error('task [${this.#id}] exited without clearing borrowed handles');
  }
  
  const state = getOrCreateAsyncState(this.#componentIdx);
  if (!state) { throw new Error('missing async state for component [' + this.#componentIdx + ']'); }
  if (!this.#isAsync && !state.inSyncExportCall) {
    throw new Error('sync task must be run from components known to be in a sync export call');
  }
  state.inSyncExportCall = false;
  
  if (this.needsExclusiveLock() && !state.isExclusivelyLocked()) {
    throw new Error('task [' + this.#id + '] exit: component [' + this.#componentIdx + '] should have been exclusively locked');
  }
  
  state.exclusiveRelease();
}

needsExclusiveLock() { return this.#needsExclusiveLock; }

createSubtask(args) {
  _debugLog('[AsyncTask#createSubtask()] args', args);
  const { componentIdx, childTask, callMetadata } = args;
  const newSubtask = new AsyncSubtask({
    componentIdx,
    childTask,
    parentTask: this,
    callMetadata,
  });
  this.#subtasks.push(newSubtask);
  return newSubtask;
}

getLatestSubtask() { return this.#subtasks.at(-1); }

currentSubtask() {
  _debugLog('[AsyncTask#currentSubtask()]');
  if (this.#subtasks.length === 0) { return undefined; }
  return this.#subtasks.at(-1);
}

endCurrentSubtask() {
  _debugLog('[AsyncTask#endCurrentSubtask()]');
  if (this.#subtasks.length === 0) { throw new Error('cannot end current subtask: no current subtask'); }
  const subtask = this.#subtasks.pop();
  subtask.drop();
  return subtask;
}
}

function _lowerImport(args, exportFn) {
  const params = [...arguments].slice(2);
  _debugLog('[_lowerImport()] args', { args, params, exportFn });
  const {
    functionIdx,
    componentIdx,
    isAsync,
    paramLiftFns,
    resultLowerFns,
    metadata,
    memoryIdx,
    getMemoryFn,
    getReallocFn,
  } = args;
  
  const parentTaskMeta = getCurrentTask(componentIdx);
  const parentTask = parentTaskMeta?.task;
  if (!parentTask) { throw new Error('missing parent task during lower of import'); }
  
  const cstate = getOrCreateAsyncState(componentIdx);
  
  const subtask = parentTask.createSubtask({
    componentIdx,
    parentTask,
    callMetadata: {
      memoryIdx,
      memory: getMemoryFn(),
      realloc: getReallocFn(),
      resultPtr: params[0],
    }
  });
  parentTask.setReturnMemoryIdx(memoryIdx);
  
  const rep = cstate.subtasks.insert(subtask);
  subtask.setRep(rep);
  
  subtask.setOnProgressFn(() => {
    subtask.setPendingEventFn(() => {
      if (subtask.resolved()) { subtask.deliverResolve(); }
      return {
        code: ASYNC_EVENT_CODE.SUBTASK,
        index: rep,
        result: subtask.getStateNumber(),
      }
    });
  });
  
  // Set up a handler on subtask completion to lower results from the call into the caller's memory region.
  subtask.registerOnResolveHandler((res) => {
    _debugLog('[_lowerImport()] handling subtask result', { res, subtaskID: subtask.id() });
    const { memory, resultPtr, realloc } = subtask.getCallMetadata();
    if (resultLowerFns.length === 0) { return; }
    resultLowerFns[0]({ componentIdx, memory, realloc, vals: [res], storagePtr: resultPtr });
  });
  
  const subtaskState = subtask.getStateNumber();
  if (subtaskState < 0 || subtaskState > 2**5) {
    throw new Error('invalid subtask state, out of valid range');
  }
  
  // NOTE: we must wait a bit before calling the export function,
  // to ensure the subtask state is not modified before the lower call return
  //
  // TODO: we should trigger via subtask state changing, rather than a static wait?
  setTimeout(async () => {
    try {
      _debugLog('[_lowerImport()] calling lowered import', { exportFn, params });
      exportFn.apply(null, params);
      
      const task = subtask.getChildTask();
      task.registerOnResolveHandler((res) => {
        _debugLog('[_lowerImport()] cascading subtask completion', {
          childTaskID: task.id(),
          subtaskID: subtask.id(),
          parentTaskID: parentTask.id(),
        });
        
        subtask.onResolve(res);
        
        cstate.tick();
      });
    } catch (err) {
      console.error("post-lower import fn error:", err);
      throw err;
    }
  }, 100);
  
  return Number(subtask.waitableRep()) << 4 | subtaskState;
}

function _liftFlatU8(ctx) {
  _debugLog('[_liftFlatU8()] args', { ctx });
  let val;
  
  if (ctx.useDirectParams) {
    if (ctx.params.length === 0) { throw new Error('expected at least a single i32 argument'); }
    val = ctx.params[0];
    ctx.params = ctx.params.slice(1);
    return [val, ctx];
  }
  
  if (ctx.storageLen !== undefined && ctx.storageLen < ctx.storagePtr + 1) {
    throw new Error('not enough storage remaining for lift');
  }
  val = new DataView(ctx.memory.buffer).getUint8(ctx.storagePtr, true);
  ctx.storagePtr += 1;
  if (ctx.storageLen !== undefined) { ctx.storageLen -= 1; }
  
  return [val, ctx];
}

function _liftFlatU16(ctx) {
  _debugLog('[_liftFlatU16()] args', { ctx });
  let val;
  
  if (ctx.useDirectParams) {
    if (params.length === 0) { throw new Error('expected at least a single i32 argument'); }
    val = ctx.params[0];
    ctx.params = ctx.params.slice(1);
    return [val, ctx];
  }
  
  if (ctx.storageLen !== undefined && ctx.storageLen < ctx.storagePtr + 2) {
    throw new Error('not enough storage remaining for lift');
  }
  val = new DataView(ctx.memory.buffer).getUint16(ctx.storagePtr, true);
  ctx.storagePtr += 2;
  if (ctx.storageLen !== undefined) { ctx.storageLen -= 2; }
  
  return [val, ctx];
}

function _liftFlatU32(ctx) {
  _debugLog('[_liftFlatU32()] args', { ctx });
  let val;
  
  if (ctx.useDirectParams) {
    if (ctx.params.length === 0) { throw new Error('expected at least a single i34 argument'); }
    val = ctx.params[0];
    ctx.params = ctx.params.slice(1);
    return [val, ctx];
  }
  
  if (ctx.storageLen !== undefined && ctx.storageLen < ctx.storagePtr + 4) {
    throw new Error('not enough storage remaining for lift');
  }
  val = new DataView(ctx.memory.buffer).getUint32(ctx.storagePtr, true);
  ctx.storagePtr += 4;
  if (ctx.storageLen !== undefined) { ctx.storageLen -= 4; }
  
  return [val, ctx];
}

function _liftFlatU64(ctx) {
  _debugLog('[_liftFlatU64()] args', { ctx });
  let val;
  
  if (ctx.useDirectParams) {
    if (ctx.params.length === 0) { throw new Error('expected at least one single i64 argument'); }
    if (typeof ctx.params[0] !== 'bigint') { throw new Error('expected bigint'); }
    val = ctx.params[0];
    ctx.params = ctx.params.slice(1);
    return [val, ctx];
  }
  
  if (ctx.storageLen !== undefined && ctx.storageLen < ctx.storagePtr + 8) {
    throw new Error('not enough storage remaining for lift');
  }
  val = new DataView(ctx.memory.buffer).getUint64(ctx.storagePtr, true);
  ctx.storagePtr += 8;
  if (ctx.storageLen !== undefined) { ctx.storageLen -= 8; }
  
  return [val, ctx];
}

function _liftFlatStringUTF8(ctx) {
  _debugLog('[_liftFlatStringUTF8()] args', { ctx });
  let val;
  
  if (ctx.useDirectParams) {
    if (ctx.params.length < 2) { throw new Error('expected at least two u32 arguments'); }
    const offset = ctx.params[0];
    if (!Number.isSafeInteger(offset)) {  throw new Error('invalid offset'); }
    const len = ctx.params[1];
    if (!Number.isSafeInteger(len)) {  throw new Error('invalid len'); }
    val = TEXT_DECODER_UTF8.decode(new DataView(ctx.memory.buffer, offset, len));
    ctx.params = ctx.params.slice(2);
    return [val, ctx];
  }
  
  const start = new DataView(ctx.memory.buffer).getUint32(ctx.storagePtr, params[0], true);
  const codeUnits = new DataView(memory.buffer).getUint32(ctx.storagePtr, params[0] + 4, true);
  val = TEXT_DECODER_UTF8.decode(new Uint8Array(ctx.memory.buffer, start, codeUnits));
  ctx.storagePtr += codeUnits;
  if (ctx.storageLen !== undefined) { ctx.storageLen -= codeUnits; }
  
  return [val, ctx];
}

function _liftFlatVariant(casesAndLiftFns) {
  return function _liftFlatVariantInner(ctx) {
    _debugLog('[_liftFlatVariant()] args', { ctx });
    
    const origUseParams = ctx.useDirectParams;
    
    let caseIdx;
    if (casesAndLiftFns.length < 256) {
      let discriminantByteLen = 1;
      const [idx, newCtx] = _liftFlatU8(ctx);
      caseIdx = idx;
      ctx = newCtx;
    } else if (casesAndLiftFns.length > 256 && discriminantByteLen < 65536) {
      discriminantByteLen = 2;
      const [idx, newCtx] = _liftFlatU16(ctx);
      caseIdx = idx;
      ctx = newCtx;
    } else if (casesAndLiftFns.length > 65536 && discriminantByteLen < 4_294_967_296) {
      discriminantByteLen = 4;
      const [idx, newCtx] = _liftFlatU32(ctx);
      caseIdx = idx;
      ctx = newCtx;
    } else {
      throw new Error('unsupported number of cases [' + casesAndLIftFns.legnth + ']');
    }
    
    const [ tag, liftFn, size32, alignment32 ] = casesAndLiftFns[caseIdx];
    
    let val;
    if (liftFn === null) {
      val = { tag };
      return [val, ctx];
    }
    
    const [newVal, newCtx] = liftFn(ctx);
    ctx = newCtx;
    val = { tag, val: newVal };
    
    return [val, ctx];
  }
}

function _liftFlatList(elemLiftFn, alignment32, knownLen) {
  function _liftFlatListInner(ctx) {
    _debugLog('[_liftFlatList()] args', { ctx });
    
    let metaPtr;
    let dataPtr;
    let len;
    if (ctx.useDirectParams) {
      if (knownLen) {
        dataPtr = _liftFlatU32(ctx);
      } else {
        metaPtr = _liftFlatU32(ctx);
      }
    } else {
      if (knownLen) {
        dataPtr = _liftFlatU32(ctx);
      } else {
        metaPtr = _liftFlatU32(ctx);
      }
    }
    
    if (metaPtr) {
      if (dataPtr !== undefined) { throw new Error('both meta and data pointers should not be set yet'); }
      
      if (ctx.useDirectParams) {
        ctx.useDirectParams = false;
        ctx.storagePtr = metaPtr;
        ctx.storageLen = 8;
        
        dataPtr = _liftFlatU32(ctx);
        len = _liftFlatU32(ctx);
        
        ctx.useDirectParams = true;
        ctx.storagePtr = null;
        ctx.storageLen = null;
      } else {
        dataPtr = _liftFlatU32(ctx);
        len = _liftFlatU32(ctx);
      }
    }
    
    const val = [];
    for (var i = 0; i < len; i++) {
      ctx.storagePtr = Math.ceil(ctx.storagePtr / alignment32) * alignment32;
      const [res, nextCtx] = elemLiftFn(ctx);
      val.push(res);
      ctx = nextCtx;
    }
    
    return [val, ctx];
  }
}

function _liftFlatFlags(cases) {
  return function _liftFlatFlagsInner(ctx) {
    _debugLog('[_liftFlatFlags()] args', { ctx });
    throw new Error('flat lift for flags not yet implemented!');
  }
}

function _liftFlatResult(casesAndLiftFns) {
  return function _liftFlatResultInner(ctx) {
    _debugLog('[_liftFlatResult()] args', { ctx });
    return _liftFlatVariant(casesAndLiftFns)(ctx);
  }
}

function _liftFlatBorrow(componentTableIdx, size, memory, vals, storagePtr, storageLen) {
  _debugLog('[_liftFlatBorrow()] args', { size, memory, vals, storagePtr, storageLen });
  throw new Error('flat lift for borrowed resources not yet implemented!');
}

function _lowerFlatU8(ctx) {
  _debugLog('[_lowerFlatU8()] args', ctx);
  const { memory, realloc, vals, storagePtr, storageLen } = ctx;
  if (vals.length !== 1) {
    throw new Error('unexpected number (' + vals.length + ') of core vals (expected 1)');
  }
  if (vals[0] > 255 || vals[0] < 0) { throw new Error('invalid value for core value representing u8'); }
  if (!memory) { throw new Error("missing memory for lower"); }
  new DataView(memory.buffer).setUint32(storagePtr, vals[0], true);
  return 1;
}

function _lowerFlatU16(memory, vals, storagePtr, storageLen) {
  _debugLog('[_lowerFlatU16()] args', { memory, vals, storagePtr, storageLen });
  if (vals.length !== 1) {
    throw new Error('unexpected number (' + vals.length + ') of core vals (expected 1)');
  }
  if (vals[0] > 65_535 || vals[0] < 0) { throw new Error('invalid value for core value representing u16'); }
  new DataView(memory.buffer).setUint16(storagePtr, vals[0], true);
  return 2;
}

function _lowerFlatU32(ctx) {
  _debugLog('[_lowerFlatU32()] args', { ctx });
  const { memory, realloc, vals, storagePtr, storageLen } = ctx;
  if (vals.length !== 1) { throw new Error('expected single value to lower, got (' + vals.length + ')'); }
  if (vals[0] > 4_294_967_295 || vals[0] < 0) { throw new Error('invalid value for core value representing u32'); }
  
  // TODO(refactor): fail loudly on misaligned flat lowers?
  const rem = ctx.storagePtr % 4;
  if (rem !== 0) { ctx.storagePtr += (4 - rem); }
  
  new DataView(memory.buffer).setUint32(storagePtr, vals[0], true);
  
  return 4;
}

function _lowerFlatU64(memory, vals, storagePtr, storageLen) {
  _debugLog('[_lowerFlatU64()] args', { memory, vals, storagePtr, storageLen });
  if (vals.length !== 1) { throw new Error('unexpected number of core vals'); }
  if (vals[0] > 18_446_744_073_709_551_615n || vals[0] < 0n) { throw new Error('invalid value for core value representing u64'); }
  new DataView(memory.buffer).setBigUint64(storagePtr, vals[0], true);
  return 8;
}

function _lowerFlatRecord(fieldMetas) {
  return (size, memory, vals, storagePtr, storageLen) => {
    const params = [...arguments].slice(5);
    _debugLog('[_lowerFlatRecord()] args', {
      size,
      memory,
      vals,
      storagePtr,
      storageLen,
      params,
      fieldMetas
    });
    
    const [start] = vals;
    if (storageLen !== undefined && size !== undefined && size > storageLen) {
      throw new Error('not enough storage remaining for record flat lower');
    }
    const data = new Uint8Array(memory.buffer, start, size);
    new Uint8Array(memory.buffer, storagePtr, size).set(data);
    return data.byteLength;
  }
}

function _lowerFlatVariant(metadata, extra) {
  const { discriminantSizeBytes, lowerMetas } = metadata;
  
  return function _lowerFlatVariantInner(ctx) {
    _debugLog('[_lowerFlatVariant()] args', ctx);
    const { memory, realloc, vals, storageLen, componentIdx } = ctx;
    let storagePtr = ctx.storagePtr;
    
    const { tag, val } = vals[0];
    const variant = lowerMetas.find(vm => vm.tag === tag);
    if (!variant) { throw new Error(`missing/invalid variant, no tag matches [${tag}] (options were ${variantMetas.map(vm => vm.tag)})`); }
    if (!variant.discriminant) { throw new Error(`missing/invalid discriminant for variant [${variant}]`); }
    
    let bytesWritten;
    let discriminantLowerArgs = { memory, realloc, vals: [variant.discriminant], storagePtr, componentIdx }
    switch (discriminantSizeBytes) {
      case 1:
      bytesWritten = _lowerFlatU8(discriminantLowerArgs);
      break;
      case 2:
      bytesWritten = _lowerFlatU16(discriminantLowerArgs);
      break;
      case 4:
      bytesWritten = _lowerFlatU32(discriminantLowerArgs);
      break;
      default:
      throw new Error(`unexpected discriminant size bytes [${discriminantSizeBytes}]`);
    }
    if (bytesWritten !== discriminantSizeBytes) {
      throw new Error("unexpectedly wrote more bytes than discriminant");
    }
    storagePtr += bytesWritten;
    
    bytesWritten += variant.lowerFn({ memory, realloc, vals: [val], storagePtr, storageLen, componentIdx });
    
    return bytesWritten;
  }
}

function _lowerFlatList(args) {
  const { elemLowerFn } = args;
  if (!elemLowerFn) { throw new TypeError("missing/invalid element lower fn for list"); }
  
  return function _lowerFlatListInner(ctx) {
    _debugLog('[_lowerFlatList()] args', { ctx });
    
    if (ctx.params.length < 2) { throw new Error('insufficient params left to lower list'); }
    const storagePtr = ctx.params[0];
    const elemCount = ctx.params[1];
    ctx.params = ctx.params.slice(2);
    
    if (ctx.useDirectParams) {
      const list = ctx.vals[0];
      if (!list) { throw new Error("missing direct param value"); }
      
      const elemLowerCtx = { storagePtr, memory: ctx.memory };
      for (let idx = 0; idx < list.length; idx++) {
        elemLowerCtx.vals = list.slice(idx, idx+1);
        elemLowerCtx.storagePtr += elemLowerFn(elemLowerCtx);
      }
      
      const bytesLowered = elemLowerCtx.storagePtr - ctx.storagePtr;
      ctx.storagePtr = elemLowerCtx.storagePtr;
      return bytesLowered;
    }
    
    
    if (ctx.vals.length !== 2) {
      throw new Error('indirect parameter loading must have a pointer and length as vals');
    }
    let [valStartPtr, valLen] = ctx.vals;
    const totalSizeBytes = valLen * size;
    if (ctx.storageLen !== undefined && totalSizeBytes > ctx.storageLen) {
      throw new Error('not enough storage remaining for list flat lower');
    }
    
    const data = new Uint8Array(memory.buffer, valStartPtr, totalSizeBytes);
    new Uint8Array(memory.buffer, storagePtr, totalSizeBytes).set(data);
    
    return totalSizeBytes;
  }
}

function _lowerFlatTuple(size, memory, vals, storagePtr, storageLen) {
  _debugLog('[_lowerFlatTuple()] args', { size, memory, vals, storagePtr, storageLen });
  let [start, len] = vals;
  if (storageLen !== undefined && len > storageLen) {
    throw new Error('not enough storage remaining for tuple flat lower');
  }
  const data = new Uint8Array(memory.buffer, start, len);
  new Uint8Array(memory.buffer, storagePtr, len).set(data);
  return data.byteLength;
}

function _lowerFlatFlags(memory, vals, storagePtr, storageLen) {
  _debugLog('[_lowerFlatFlags()] args', { size, memory, vals, storagePtr, storageLen });
  if (vals.length !== 1) { throw new Error('unexpected number of core vals'); }
  new DataView(memory.buffer).setInt32(storagePtr, vals[0], true);
  return 4;
}

function _lowerFlatEnum(size, memory, vals, storagePtr, storageLen) {
  _debugLog('[_lowerFlatEnum()] args', { size, memory, vals, storagePtr, storageLen });
  let [start] = vals;
  if (storageLen !== undefined && size !== undefined && size > storageLen) {
    throw new Error('not enough storage remaining for enum flat lower');
  }
  const data = new Uint8Array(memory.buffer, start, size);
  new Uint8Array(memory.buffer, storagePtr, size).set(data);
  return data.byteLength;
}

function _lowerFlatOption(size, memory, vals, storagePtr, storageLen) {
  _debugLog('[_lowerFlatOption()] args', { size, memory, vals, storagePtr, storageLen });
  let [start] = vals;
  if (storageLen !== undefined && size !== undefined && size > storageLen) {
    throw new Error('not enough storage remaining for option flat lower');
  }
  const data = new Uint8Array(memory.buffer, start, size);
  new Uint8Array(memory.buffer, storagePtr, size).set(data);
  return data.byteLength;
}

function _lowerFlatResult(lowerMetas) {
  const invalidTag = lowerMetas.find(t => t.tag !== 'ok' && t.tag !== 'error')
  if (invalidTag) { throw new Error(`invalid variant tag [${invalidTag}] found for result`); }
  
  return function _lowerFlatResultInner() {
    _debugLog('[_lowerFlatResult()] args', { lowerMetas });
    let lowerFn = _lowerFlatVariant({ discriminantSizeBytes: 1, lowerMetas }, { forResult: true });
    return lowerFn.apply(null, arguments);
  };
}

function _lowerFlatOwn(size, memory, vals, storagePtr, storageLen) {
  _debugLog('[_lowerFlatOwn()] args', { size, memory, vals, storagePtr, storageLen });
  throw new Error('flat lower for owned resources not yet implemented!');
}
const ASYNC_STATE = new Map();

function getOrCreateAsyncState(componentIdx, init) {
  if (!ASYNC_STATE.has(componentIdx)) {
    const newState = new ComponentAsyncState({ componentIdx });
    ASYNC_STATE.set(componentIdx, newState);
  }
  return ASYNC_STATE.get(componentIdx);
}

class ComponentAsyncState {
  static EVENT_HANDLER_EVENTS = [ 'backpressure-change' ];
  
  #componentIdx;
  #callingAsyncImport = false;
  #syncImportWait = promiseWithResolvers();
  #locked = false;
  #parkedTasks = new Map();
  #suspendedTasksByTaskID = new Map();
  #suspendedTaskIDs = [];
  #pendingTasks = [];
  #errored = null;
  
  #backpressure = 0;
  #backpressureWaiters = 0n;
  
  #handlerMap = new Map();
  #nextHandlerID = 0n;
  
  mayLeave = true;
  
  #streams;
  
  waitableSets;
  waitables;
  subtasks;
  
  constructor(args) {
    this.#componentIdx = args.componentIdx;
    this.waitableSets = new RepTable({ target: `component [${this.#componentIdx}] waitable sets` });
    this.waitables = new RepTable({ target: `component [${this.#componentIdx}] waitables` });
    this.subtasks = new RepTable({ target: `component [${this.#componentIdx}] subtasks` });
    this.#streams = new Map();
  };
  
  componentIdx() { return this.#componentIdx; }
  streams() { return this.#streams; }
  
  errored() { return this.#errored !== null; }
  setErrored(err) {
    _debugLog('[ComponentAsyncState#setErrored()] component errored', { err, componentIdx: this.#componentIdx });
    if (this.#errored) { return; }
    if (!err) {
      err = new Error('error elswehere (see other component instance error)')
      err.componentIdx = this.#componentIdx;
    }
    this.#errored = err;
  }
  
  callingSyncImport(val) {
    if (val === undefined) { return this.#callingAsyncImport; }
    if (typeof val !== 'boolean') { throw new TypeError('invalid setting for async import'); }
    const prev = this.#callingAsyncImport;
    this.#callingAsyncImport = val;
    if (prev === true && this.#callingAsyncImport === false) {
      this.#notifySyncImportEnd();
    }
  }
  
  #notifySyncImportEnd() {
    const existing = this.#syncImportWait;
    this.#syncImportWait = promiseWithResolvers();
    existing.resolve();
  }
  
  async waitForSyncImportCallEnd() {
    await this.#syncImportWait.promise;
  }
  
  setBackpressure(v) { this.#backpressure = v; }
  getBackpressure(v) { return this.#backpressure; }
  incrementBackpressure() {
    const newValue = this.getBackpressure() + 1;
    if (newValue > 2**16) { throw new Error("invalid backpressure value, overflow"); }
    this.setBackpressure(newValue);
  }
  decrementBackpressure() {
    this.setBackpressure(Math.max(0, this.getBackpressure() - 1));
  }
  hasBackpressure() { return this.#backpressure > 0; }
  
  waitForBackpressure() {
    let backpressureCleared = false;
    const cstate = this;
    cstate.addBackpressureWaiter();
    const handlerID = this.registerHandler({
      event: 'backpressure-change',
      fn: (bp) => {
        if (bp === 0) {
          cstate.removeHandler(handlerID);
          backpressureCleared = true;
        }
      }
    });
    return new Promise((resolve) => {
      const interval = setInterval(() => {
        if (backpressureCleared) { return; }
        clearInterval(interval);
        cstate.removeBackpressureWaiter();
        resolve(null);
      }, 0);
    });
  }
  
  registerHandler(args) {
    const { event, fn } = args;
    if (!event) { throw new Error("missing handler event"); }
    if (!fn) { throw new Error("missing handler fn"); }
    
    if (!ComponentAsyncState.EVENT_HANDLER_EVENTS.includes(event)) {
      throw new Error(`unrecognized event handler [${event}]`);
    }
    
    const handlerID = this.#nextHandlerID++;
    let handlers = this.#handlerMap.get(event);
    if (!handlers) {
      handlers = [];
      this.#handlerMap.set(event, handlers)
    }
    
    handlers.push({ id: handlerID, fn, event });
    return handlerID;
  }
  
  removeHandler(args) {
    const { event, handlerID } = args;
    const registeredHandlers = this.#handlerMap.get(event);
    if (!registeredHandlers) { return; }
    const found = registeredHandlers.find(h => h.id === handlerID);
    if (!found) { return; }
    this.#handlerMap.set(event, this.#handlerMap.get(event).filter(h => h.id !== handlerID));
  }
  
  getBackpressureWaiters() { return this.#backpressureWaiters; }
  addBackpressureWaiter() { this.#backpressureWaiters++; }
  removeBackpressureWaiter() {
    this.#backpressureWaiters--;
    if (this.#backpressureWaiters < 0) {
      throw new Error("unexepctedly negative number of backpressure waiters");
    }
  }
  
  parkTaskOnAwaitable(args) {
    if (!args.awaitable) { throw new TypeError('missing awaitable when trying to park'); }
    if (!args.task) { throw new TypeError('missing task when trying to park'); }
    const { awaitable, task } = args;
    
    let taskList = this.#parkedTasks.get(awaitable.id());
    if (!taskList) {
      taskList = [];
      this.#parkedTasks.set(awaitable.id(), taskList);
    }
    taskList.push(task);
    
    this.wakeNextTaskForAwaitable(awaitable);
  }
  
  wakeNextTaskForAwaitable(awaitable) {
    if (!awaitable) { throw new TypeError('missing awaitable when waking next task'); }
    const awaitableID = awaitable.id();
    
    const taskList = this.#parkedTasks.get(awaitableID);
    if (!taskList || taskList.length === 0) {
      _debugLog('[ComponentAsyncState] no tasks waiting for awaitable', { awaitableID: awaitable.id() });
      return;
    }
    
    let task = taskList.shift(); // todo(perf)
    if (!task) { throw new Error('no task in parked list despite previous check'); }
    
    if (!task.awaitableResume) {
      throw new Error('task ready due to awaitable is missing resume', { taskID: task.id(), awaitableID });
    }
    task.awaitableResume();
  }
  
  // TODO: we might want to check for pre-locked status here
  exclusiveLock() {
    this.#locked = true;
  }
  
  exclusiveRelease() {
    _debugLog('[ComponentAsyncState#exclusiveRelease()] releasing', {
      locked: this.#locked,
      componentIdx: this.#componentIdx,
    });
    
    this.#locked = false
  }
  
  isExclusivelyLocked() { return this.#locked === true; }
  
  #getSuspendedTaskMeta(taskID) {
    return this.#suspendedTasksByTaskID.get(taskID);
  }
  
  #removeSuspendedTaskMeta(taskID) {
    _debugLog('[ComponentAsyncState#removeSuspendedTaskMeta()] removing suspended task', { taskID });
    const idx = this.#suspendedTaskIDs.findIndex(t => t === taskID);
    const meta = this.#suspendedTasksByTaskID.get(taskID);
    this.#suspendedTaskIDs[idx] = null;
    this.#suspendedTasksByTaskID.delete(taskID);
    return meta;
  }
  
  #addSuspendedTaskMeta(meta) {
    if (!meta) { throw new Error('missing task meta'); }
    const taskID = meta.taskID;
    this.#suspendedTasksByTaskID.set(taskID, meta);
    this.#suspendedTaskIDs.push(taskID);
    if (this.#suspendedTasksByTaskID.size < this.#suspendedTaskIDs.length - 10) {
      this.#suspendedTaskIDs = this.#suspendedTaskIDs.filter(t => t !== null);
    }
  }
  
  suspendTask(args) {
    // TODO(threads): readyFn is normally on the thread
    const { task, readyFn } = args;
    const taskID = task.id();
    _debugLog('[ComponentAsyncState#suspendTask()]', { taskID });
    
    if (this.#getSuspendedTaskMeta(taskID)) {
      throw new Error('task [' + taskID + '] already suspended');
    }
    
    const { promise, resolve } = Promise.withResolvers();
    this.#addSuspendedTaskMeta({
      task,
      taskID,
      readyFn,
      resume: () => {
        _debugLog('[ComponentAsyncState#suspendTask()] resuming suspended task', { taskID });
        // TODO(threads): it's thread cancellation we should be checking for below, not task
        resolve(!task.isCancelled());
      },
    });
    
    return promise;
  }
  
  resumeTaskByID(taskID) {
    const meta = this.#removeSuspendedTaskMeta(taskID);
    if (!meta) { return; }
    if (meta.taskID !== taskID) { throw new Error('task ID does not match'); }
    meta.resume();
  }
  
  tick() {
    _debugLog('[ComponentAsyncState#tick()]', { suspendedTaskIDs: this.#suspendedTaskIDs });
    const resumableTasks = this.#suspendedTaskIDs.filter(t => t !== null);
    for (const taskID of resumableTasks) {
      const meta = this.#suspendedTasksByTaskID.get(taskID);
      if (!meta || !meta.readyFn) {
        throw new Error(`missing/invalid task despite ID [${taskID}] being present`);
      }
      
      const isReady = meta.readyFn();
      if (!isReady) { continue; }
      
      this.resumeTaskByID(taskID);
    }
    
    return this.#suspendedTaskIDs.filter(t => t !== null).length === 0;
  }
  
  addPendingTask(task) {
    this.#pendingTasks.push(task);
  }
  
  addStreamEnd(args) {
    _debugLog('[ComponentAsyncState#addStreamEnd()] args', args);
    const { tableIdx, streamEnd } = args;
    
    let tbl = this.#streams.get(tableIdx);
    if (!tbl) {
      tbl = new RepTable({ target: `component [${this.#componentIdx}] streams` });
      this.#streams.set(tableIdx, tbl);
    }
    
    const streamIdx = tbl.insert(streamEnd);
    return streamIdx;
  }
  
  createStream(args) {
    _debugLog('[ComponentAsyncState#createStream()] args', args);
    const { tableIdx, elemMeta } = args;
    if (tableIdx === undefined) { throw new Error("missing table idx while adding stream"); }
    if (elemMeta === undefined) { throw new Error("missing element metadata while adding stream"); }
    
    let tbl = this.#streams.get(tableIdx);
    if (!tbl) {
      tbl = new RepTable({ target: `component [${this.#componentIdx}] streams` });
      this.#streams.set(tableIdx, tbl);
    }
    
    const stream = new InternalStream({
      tableIdx,
      componentIdx: this.#componentIdx,
      elemMeta,
    });
    const writeEndIdx = tbl.insert(stream.getWriteEnd());
    stream.setWriteEndIdx(writeEndIdx);
    const readEndIdx = tbl.insert(stream.getReadEnd());
    stream.setReadEndIdx(readEndIdx);
    
    const rep = STREAMS.insert(stream);
    stream.setRep(rep);
    
    return { writeEndIdx, readEndIdx };
  }
  
  getStreamEnd(args) {
    _debugLog('[ComponentAsyncState#getStreamEnd()] args', args);
    const { tableIdx, streamIdx } = args;
    if (tableIdx === undefined) { throw new Error('missing table idx while retrieveing stream end'); }
    if (streamIdx === undefined) { throw new Error('missing stream idx while retrieveing stream end'); }
    
    const tbl = this.#streams.get(tableIdx);
    if (!tbl) {
      throw new Error(`missing stream table [${tableIdx}] in component [${this.#componentIdx}] while getting stream`);
    }
    
    const stream = tbl.get(streamIdx);
    return stream;
  }
  
  removeStreamEnd(args) {
    _debugLog('[ComponentAsyncState#removeStreamEnd()] args', args);
    const { tableIdx, streamIdx } = args;
    if (tableIdx === undefined) { throw new Error("missing table idx while removing stream end"); }
    if (streamIdx === undefined) { throw new Error("missing stream idx while removing stream end"); }
    
    const tbl = this.#streams.get(tableIdx);
    if (!tbl) {
      throw new Error(`missing stream table [${tableIdx}] in component [${this.#componentIdx}] while removing stream end`);
    }
    
    const stream = tbl.get(streamIdx);
    if (!stream) { throw new Error(`component [${this.#componentIdx}] missing stream [${streamIdx}]`); }
    
    const removed = tbl.remove(streamIdx);
    if (!removed) {
      throw new Error(`missing stream [${streamIdx}] (table [${tableIdx}]) in component [${this.#componentIdx}] while removing stream end`);
    }
    
    return stream;
  }
}

const base64Compile = str => WebAssembly.compile(typeof Buffer !== 'undefined' ? Buffer.from(str, 'base64') : Uint8Array.from(atob(str), b => b.charCodeAt(0)));

const isNode = typeof process !== 'undefined' && process.versions && process.versions.node;
let _fs;
async function fetchCompile (url) {
  if (isNode) {
    _fs = _fs || await import('node:fs/promises');
    return WebAssembly.compile(await _fs.readFile(url));
  }
  return fetch(url).then(WebAssembly.compileStreaming);
}

const symbolCabiDispose = Symbol.for('cabiDispose');

const symbolRscHandle = Symbol('handle');

const symbolRscRep = Symbol.for('cabiRep');

const symbolDispose = Symbol.dispose || Symbol.for('dispose');

const handleTables = [];

class ComponentError extends Error {
  constructor (value) {
    const enumerable = typeof value !== 'string';
    super(enumerable ? `${String(value)} (see error.payload)` : value);
    Object.defineProperty(this, 'payload', { value, enumerable });
  }
}

function getErrorPayload(e) {
  if (e && hasOwnProperty.call(e, 'payload')) return e.payload;
  if (e instanceof Error) throw e;
  return e;
}

function throwInvalidBool() {
  throw new TypeError('invalid variant discriminant for bool');
}

const hasOwnProperty = Object.prototype.hasOwnProperty;

const instantiateCore = WebAssembly.instantiate;


let exports0;

let lowered_import_0_metadata = {
  qualifiedImportFn: 'wasi:cli/exit@0.2.6#exit',
  moduleIdx: null,
};


function trampoline8(arg0) {
  let variant0;
  switch (arg0) {
    case 0: {
      variant0= {
        tag: 'ok',
        val: undefined
      };
      break;
    }
    case 1: {
      variant0= {
        tag: 'err',
        val: undefined
      };
      break;
    }
    default: {
      throw new TypeError('invalid variant discriminant for expected');
    }
  }
  _debugLog('[iface="wasi:cli/exit@0.2.6", function="exit"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = exit?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'exit',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'none',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  let ret; exit(variant0);
  endCurrentTask(0);
  _debugLog('[iface="wasi:cli/exit@0.2.6", function="exit"][Instruction::Return]', {
    funcName: 'exit',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}


let lowered_import_1_metadata = {
  qualifiedImportFn: 'wasi:io/poll@0.2.6#[method]pollable.block',
  moduleIdx: null,
};

const handleTable0 = [T_FLAG, 0];
const captureTable0= new Map();
let captureCnt0 = 0;
handleTables[0] = handleTable0;

function trampoline9(arg0) {
  var handle1 = arg0;
  var rep2 = handleTable0[(handle1 << 1) + 1] & ~T_FLAG;
  var rsc0 = captureTable0.get(rep2);
  if (!rsc0) {
    rsc0 = Object.create(Pollable.prototype);
    Object.defineProperty(rsc0, symbolRscHandle, { writable: true, value: handle1});
    Object.defineProperty(rsc0, symbolRscRep, { writable: true, value: rep2});
  }
  curResourceBorrows.push(rsc0);
  _debugLog('[iface="wasi:io/poll@0.2.6", function="[method]pollable.block"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = rsc0.block?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'block',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'none',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  let ret; rsc0.block();
  for (const rsc of curResourceBorrows) {
    rsc[symbolRscHandle] = undefined;
  }
  curResourceBorrows = [];
  endCurrentTask(0);
  _debugLog('[iface="wasi:io/poll@0.2.6", function="[method]pollable.block"][Instruction::Return]', {
    funcName: '[method]pollable.block',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}


let lowered_import_2_metadata = {
  qualifiedImportFn: 'wasi:io/streams@0.2.6#[method]input-stream.subscribe',
  moduleIdx: null,
};

const handleTable2 = [T_FLAG, 0];
const captureTable2= new Map();
let captureCnt2 = 0;
handleTables[2] = handleTable2;

function trampoline10(arg0) {
  var handle1 = arg0;
  var rep2 = handleTable2[(handle1 << 1) + 1] & ~T_FLAG;
  var rsc0 = captureTable2.get(rep2);
  if (!rsc0) {
    rsc0 = Object.create(InputStream.prototype);
    Object.defineProperty(rsc0, symbolRscHandle, { writable: true, value: handle1});
    Object.defineProperty(rsc0, symbolRscRep, { writable: true, value: rep2});
  }
  curResourceBorrows.push(rsc0);
  _debugLog('[iface="wasi:io/streams@0.2.6", function="[method]input-stream.subscribe"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = rsc0.subscribe?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'subscribe',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'none',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  let ret =  rsc0.subscribe();
  for (const rsc of curResourceBorrows) {
    rsc[symbolRscHandle] = undefined;
  }
  curResourceBorrows = [];
  endCurrentTask(0);
  if (!(ret instanceof Pollable)) {
    throw new TypeError('Resource error: Not a valid "Pollable" resource.');
  }
  var handle3 = ret[symbolRscHandle];
  if (!handle3) {
    const rep = ret[symbolRscRep] || ++captureCnt0;
    captureTable0.set(rep, ret);
    handle3 = rscTableCreateOwn(handleTable0, rep);
  }
  _debugLog('[iface="wasi:io/streams@0.2.6", function="[method]input-stream.subscribe"][Instruction::Return]', {
    funcName: '[method]input-stream.subscribe',
    paramCount: 1,
    async: false,
    postReturn: false
  });
  return handle3;
}


let lowered_import_3_metadata = {
  qualifiedImportFn: 'wasi:io/streams@0.2.6#[method]output-stream.subscribe',
  moduleIdx: null,
};

const handleTable3 = [T_FLAG, 0];
const captureTable3= new Map();
let captureCnt3 = 0;
handleTables[3] = handleTable3;

function trampoline11(arg0) {
  var handle1 = arg0;
  var rep2 = handleTable3[(handle1 << 1) + 1] & ~T_FLAG;
  var rsc0 = captureTable3.get(rep2);
  if (!rsc0) {
    rsc0 = Object.create(OutputStream.prototype);
    Object.defineProperty(rsc0, symbolRscHandle, { writable: true, value: handle1});
    Object.defineProperty(rsc0, symbolRscRep, { writable: true, value: rep2});
  }
  curResourceBorrows.push(rsc0);
  _debugLog('[iface="wasi:io/streams@0.2.6", function="[method]output-stream.subscribe"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = rsc0.subscribe?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'subscribe',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'none',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  let ret =  rsc0.subscribe();
  for (const rsc of curResourceBorrows) {
    rsc[symbolRscHandle] = undefined;
  }
  curResourceBorrows = [];
  endCurrentTask(0);
  if (!(ret instanceof Pollable)) {
    throw new TypeError('Resource error: Not a valid "Pollable" resource.');
  }
  var handle3 = ret[symbolRscHandle];
  if (!handle3) {
    const rep = ret[symbolRscRep] || ++captureCnt0;
    captureTable0.set(rep, ret);
    handle3 = rscTableCreateOwn(handleTable0, rep);
  }
  _debugLog('[iface="wasi:io/streams@0.2.6", function="[method]output-stream.subscribe"][Instruction::Return]', {
    funcName: '[method]output-stream.subscribe',
    paramCount: 1,
    async: false,
    postReturn: false
  });
  return handle3;
}


let lowered_import_4_metadata = {
  qualifiedImportFn: 'wasi:cli/stdin@0.2.6#get-stdin',
  moduleIdx: null,
};


function trampoline12() {
  _debugLog('[iface="wasi:cli/stdin@0.2.6", function="get-stdin"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = getStdin?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'getStdin',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'none',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  let ret =  getStdin();
  endCurrentTask(0);
  if (!(ret instanceof InputStream)) {
    throw new TypeError('Resource error: Not a valid "InputStream" resource.');
  }
  var handle0 = ret[symbolRscHandle];
  if (!handle0) {
    const rep = ret[symbolRscRep] || ++captureCnt2;
    captureTable2.set(rep, ret);
    handle0 = rscTableCreateOwn(handleTable2, rep);
  }
  _debugLog('[iface="wasi:cli/stdin@0.2.6", function="get-stdin"][Instruction::Return]', {
    funcName: 'get-stdin',
    paramCount: 1,
    async: false,
    postReturn: false
  });
  return handle0;
}


let lowered_import_5_metadata = {
  qualifiedImportFn: 'wasi:cli/stdout@0.2.6#get-stdout',
  moduleIdx: null,
};


function trampoline13() {
  _debugLog('[iface="wasi:cli/stdout@0.2.6", function="get-stdout"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = getStdout?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'getStdout',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'none',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  let ret =  getStdout();
  endCurrentTask(0);
  if (!(ret instanceof OutputStream)) {
    throw new TypeError('Resource error: Not a valid "OutputStream" resource.');
  }
  var handle0 = ret[symbolRscHandle];
  if (!handle0) {
    const rep = ret[symbolRscRep] || ++captureCnt3;
    captureTable3.set(rep, ret);
    handle0 = rscTableCreateOwn(handleTable3, rep);
  }
  _debugLog('[iface="wasi:cli/stdout@0.2.6", function="get-stdout"][Instruction::Return]', {
    funcName: 'get-stdout',
    paramCount: 1,
    async: false,
    postReturn: false
  });
  return handle0;
}


let lowered_import_6_metadata = {
  qualifiedImportFn: 'wasi:cli/stderr@0.2.6#get-stderr',
  moduleIdx: null,
};


function trampoline14() {
  _debugLog('[iface="wasi:cli/stderr@0.2.6", function="get-stderr"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = getStderr?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'getStderr',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'none',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  let ret =  getStderr();
  endCurrentTask(0);
  if (!(ret instanceof OutputStream)) {
    throw new TypeError('Resource error: Not a valid "OutputStream" resource.');
  }
  var handle0 = ret[symbolRscHandle];
  if (!handle0) {
    const rep = ret[symbolRscRep] || ++captureCnt3;
    captureTable3.set(rep, ret);
    handle0 = rscTableCreateOwn(handleTable3, rep);
  }
  _debugLog('[iface="wasi:cli/stderr@0.2.6", function="get-stderr"][Instruction::Return]', {
    funcName: 'get-stderr',
    paramCount: 1,
    async: false,
    postReturn: false
  });
  return handle0;
}


let lowered_import_7_metadata = {
  qualifiedImportFn: 'wasi:clocks/monotonic-clock@0.2.6#now',
  moduleIdx: null,
};


function trampoline15() {
  _debugLog('[iface="wasi:clocks/monotonic-clock@0.2.6", function="now"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = now?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'now',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'none',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  let ret =  now();
  endCurrentTask(0);
  _debugLog('[iface="wasi:clocks/monotonic-clock@0.2.6", function="now"][Instruction::Return]', {
    funcName: 'now',
    paramCount: 1,
    async: false,
    postReturn: false
  });
  return toUint64(ret);
}

let exports1;
let memory0;
let realloc0;

let lowered_import_8_metadata = {
  qualifiedImportFn: 'wasi:random/insecure-seed@0.2.6#insecure-seed',
  moduleIdx: null,
};


function trampoline16(arg0) {
  _debugLog('[iface="wasi:random/insecure-seed@0.2.6", function="insecure-seed"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = insecureSeed?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'insecureSeed',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'none',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  let ret =  insecureSeed();
  endCurrentTask(0);
  var [tuple0_0, tuple0_1] = ret;
  dataView(memory0).setBigInt64(arg0 + 0, toUint64(tuple0_0), true);
  dataView(memory0).setBigInt64(arg0 + 8, toUint64(tuple0_1), true);
  _debugLog('[iface="wasi:random/insecure-seed@0.2.6", function="insecure-seed"][Instruction::Return]', {
    funcName: 'insecure-seed',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}


let lowered_import_9_metadata = {
  qualifiedImportFn: 'wasi:io/streams@0.2.6#[method]input-stream.blocking-read',
  moduleIdx: null,
};

const handleTable1 = [T_FLAG, 0];
const captureTable1= new Map();
let captureCnt1 = 0;
handleTables[1] = handleTable1;

function trampoline17(arg0, arg1, arg2) {
  var handle1 = arg0;
  var rep2 = handleTable2[(handle1 << 1) + 1] & ~T_FLAG;
  var rsc0 = captureTable2.get(rep2);
  if (!rsc0) {
    rsc0 = Object.create(InputStream.prototype);
    Object.defineProperty(rsc0, symbolRscHandle, { writable: true, value: handle1});
    Object.defineProperty(rsc0, symbolRscRep, { writable: true, value: rep2});
  }
  curResourceBorrows.push(rsc0);
  _debugLog('[iface="wasi:io/streams@0.2.6", function="[method]input-stream.blocking-read"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = rsc0.blockingRead?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'blockingRead',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'result-catch-handler',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  
  let ret;
  try {
    ret = { tag: 'ok', val:  rsc0.blockingRead(BigInt.asUintN(64, arg1))};
  } catch (e) {
    ret = { tag: 'err', val: getErrorPayload(e) };
  }
  
  for (const rsc of curResourceBorrows) {
    rsc[symbolRscHandle] = undefined;
  }
  curResourceBorrows = [];
  endCurrentTask(0);
  var variant6 = ret;
  switch (variant6.tag) {
    case 'ok': {
      const e = variant6.val;
      dataView(memory0).setInt8(arg2 + 0, 0, true);
      var val3 = e;
      var len3 = val3.byteLength;
      var ptr3 = realloc0(0, 0, 1, len3 * 1);
      
      let valData3;
      const valLenBytes3 = len3 * 1;
      if (Array.isArray(val3)) {
        // Regular array likely containing numbers, write values to memory
        let offset = 0;
        const dv3 = new DataView(memory0.buffer);
        for (const v of val3) {
          dv3.setUint8(ptr3+ offset, v, true);
          offset += 1;
        }
      } else {
        // TypedArray / ArrayBuffer-like, direct copy
        valData3 = new Uint8Array(val3.buffer || val3, val3.byteOffset, valLenBytes3);
        const out3 = new Uint8Array(memory0.buffer, ptr3,valLenBytes3);
        out3.set(valData3);
      }
      
      dataView(memory0).setUint32(arg2 + 8, len3, true);
      dataView(memory0).setUint32(arg2 + 4, ptr3, true);
      break;
    }
    case 'err': {
      const e = variant6.val;
      dataView(memory0).setInt8(arg2 + 0, 1, true);
      var variant5 = e;
      switch (variant5.tag) {
        case 'last-operation-failed': {
          const e = variant5.val;
          dataView(memory0).setInt8(arg2 + 4, 0, true);
          if (!(e instanceof Error$1)) {
            throw new TypeError('Resource error: Not a valid "Error" resource.');
          }
          var handle4 = e[symbolRscHandle];
          if (!handle4) {
            const rep = e[symbolRscRep] || ++captureCnt1;
            captureTable1.set(rep, e);
            handle4 = rscTableCreateOwn(handleTable1, rep);
          }
          dataView(memory0).setInt32(arg2 + 8, handle4, true);
          break;
        }
        case 'closed': {
          dataView(memory0).setInt8(arg2 + 4, 1, true);
          break;
        }
        default: {
          throw new TypeError(`invalid variant tag value \`${JSON.stringify(variant5.tag)}\` (received \`${variant5}\`) specified for \`StreamError\``);
        }
      }
      break;
    }
    default: {
      throw new TypeError('invalid variant specified for result');
    }
  }
  _debugLog('[iface="wasi:io/streams@0.2.6", function="[method]input-stream.blocking-read"][Instruction::Return]', {
    funcName: '[method]input-stream.blocking-read',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}


let lowered_import_10_metadata = {
  qualifiedImportFn: 'wasi:io/streams@0.2.6#[method]output-stream.check-write',
  moduleIdx: null,
};


function trampoline18(arg0, arg1) {
  var handle1 = arg0;
  var rep2 = handleTable3[(handle1 << 1) + 1] & ~T_FLAG;
  var rsc0 = captureTable3.get(rep2);
  if (!rsc0) {
    rsc0 = Object.create(OutputStream.prototype);
    Object.defineProperty(rsc0, symbolRscHandle, { writable: true, value: handle1});
    Object.defineProperty(rsc0, symbolRscRep, { writable: true, value: rep2});
  }
  curResourceBorrows.push(rsc0);
  _debugLog('[iface="wasi:io/streams@0.2.6", function="[method]output-stream.check-write"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = rsc0.checkWrite?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'checkWrite',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'result-catch-handler',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  
  let ret;
  try {
    ret = { tag: 'ok', val:  rsc0.checkWrite()};
  } catch (e) {
    ret = { tag: 'err', val: getErrorPayload(e) };
  }
  
  for (const rsc of curResourceBorrows) {
    rsc[symbolRscHandle] = undefined;
  }
  curResourceBorrows = [];
  endCurrentTask(0);
  var variant5 = ret;
  switch (variant5.tag) {
    case 'ok': {
      const e = variant5.val;
      dataView(memory0).setInt8(arg1 + 0, 0, true);
      dataView(memory0).setBigInt64(arg1 + 8, toUint64(e), true);
      break;
    }
    case 'err': {
      const e = variant5.val;
      dataView(memory0).setInt8(arg1 + 0, 1, true);
      var variant4 = e;
      switch (variant4.tag) {
        case 'last-operation-failed': {
          const e = variant4.val;
          dataView(memory0).setInt8(arg1 + 8, 0, true);
          if (!(e instanceof Error$1)) {
            throw new TypeError('Resource error: Not a valid "Error" resource.');
          }
          var handle3 = e[symbolRscHandle];
          if (!handle3) {
            const rep = e[symbolRscRep] || ++captureCnt1;
            captureTable1.set(rep, e);
            handle3 = rscTableCreateOwn(handleTable1, rep);
          }
          dataView(memory0).setInt32(arg1 + 12, handle3, true);
          break;
        }
        case 'closed': {
          dataView(memory0).setInt8(arg1 + 8, 1, true);
          break;
        }
        default: {
          throw new TypeError(`invalid variant tag value \`${JSON.stringify(variant4.tag)}\` (received \`${variant4}\`) specified for \`StreamError\``);
        }
      }
      break;
    }
    default: {
      throw new TypeError('invalid variant specified for result');
    }
  }
  _debugLog('[iface="wasi:io/streams@0.2.6", function="[method]output-stream.check-write"][Instruction::Return]', {
    funcName: '[method]output-stream.check-write',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}


let lowered_import_11_metadata = {
  qualifiedImportFn: 'wasi:io/streams@0.2.6#[method]output-stream.write',
  moduleIdx: null,
};


function trampoline19(arg0, arg1, arg2, arg3) {
  var handle1 = arg0;
  var rep2 = handleTable3[(handle1 << 1) + 1] & ~T_FLAG;
  var rsc0 = captureTable3.get(rep2);
  if (!rsc0) {
    rsc0 = Object.create(OutputStream.prototype);
    Object.defineProperty(rsc0, symbolRscHandle, { writable: true, value: handle1});
    Object.defineProperty(rsc0, symbolRscRep, { writable: true, value: rep2});
  }
  curResourceBorrows.push(rsc0);
  var ptr3 = arg1;
  var len3 = arg2;
  var result3 = new Uint8Array(memory0.buffer.slice(ptr3, ptr3 + len3 * 1));
  _debugLog('[iface="wasi:io/streams@0.2.6", function="[method]output-stream.write"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = rsc0.write?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'write',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'result-catch-handler',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  
  let ret;
  try {
    ret = { tag: 'ok', val:  rsc0.write(result3)};
  } catch (e) {
    ret = { tag: 'err', val: getErrorPayload(e) };
  }
  
  for (const rsc of curResourceBorrows) {
    rsc[symbolRscHandle] = undefined;
  }
  curResourceBorrows = [];
  endCurrentTask(0);
  var variant6 = ret;
  switch (variant6.tag) {
    case 'ok': {
      const e = variant6.val;
      dataView(memory0).setInt8(arg3 + 0, 0, true);
      break;
    }
    case 'err': {
      const e = variant6.val;
      dataView(memory0).setInt8(arg3 + 0, 1, true);
      var variant5 = e;
      switch (variant5.tag) {
        case 'last-operation-failed': {
          const e = variant5.val;
          dataView(memory0).setInt8(arg3 + 4, 0, true);
          if (!(e instanceof Error$1)) {
            throw new TypeError('Resource error: Not a valid "Error" resource.');
          }
          var handle4 = e[symbolRscHandle];
          if (!handle4) {
            const rep = e[symbolRscRep] || ++captureCnt1;
            captureTable1.set(rep, e);
            handle4 = rscTableCreateOwn(handleTable1, rep);
          }
          dataView(memory0).setInt32(arg3 + 8, handle4, true);
          break;
        }
        case 'closed': {
          dataView(memory0).setInt8(arg3 + 4, 1, true);
          break;
        }
        default: {
          throw new TypeError(`invalid variant tag value \`${JSON.stringify(variant5.tag)}\` (received \`${variant5}\`) specified for \`StreamError\``);
        }
      }
      break;
    }
    default: {
      throw new TypeError('invalid variant specified for result');
    }
  }
  _debugLog('[iface="wasi:io/streams@0.2.6", function="[method]output-stream.write"][Instruction::Return]', {
    funcName: '[method]output-stream.write',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}


let lowered_import_12_metadata = {
  qualifiedImportFn: 'wasi:io/streams@0.2.6#[method]output-stream.blocking-flush',
  moduleIdx: null,
};


function trampoline20(arg0, arg1) {
  var handle1 = arg0;
  var rep2 = handleTable3[(handle1 << 1) + 1] & ~T_FLAG;
  var rsc0 = captureTable3.get(rep2);
  if (!rsc0) {
    rsc0 = Object.create(OutputStream.prototype);
    Object.defineProperty(rsc0, symbolRscHandle, { writable: true, value: handle1});
    Object.defineProperty(rsc0, symbolRscRep, { writable: true, value: rep2});
  }
  curResourceBorrows.push(rsc0);
  _debugLog('[iface="wasi:io/streams@0.2.6", function="[method]output-stream.blocking-flush"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = rsc0.blockingFlush?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'blockingFlush',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'result-catch-handler',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  
  let ret;
  try {
    ret = { tag: 'ok', val:  rsc0.blockingFlush()};
  } catch (e) {
    ret = { tag: 'err', val: getErrorPayload(e) };
  }
  
  for (const rsc of curResourceBorrows) {
    rsc[symbolRscHandle] = undefined;
  }
  curResourceBorrows = [];
  endCurrentTask(0);
  var variant5 = ret;
  switch (variant5.tag) {
    case 'ok': {
      const e = variant5.val;
      dataView(memory0).setInt8(arg1 + 0, 0, true);
      break;
    }
    case 'err': {
      const e = variant5.val;
      dataView(memory0).setInt8(arg1 + 0, 1, true);
      var variant4 = e;
      switch (variant4.tag) {
        case 'last-operation-failed': {
          const e = variant4.val;
          dataView(memory0).setInt8(arg1 + 4, 0, true);
          if (!(e instanceof Error$1)) {
            throw new TypeError('Resource error: Not a valid "Error" resource.');
          }
          var handle3 = e[symbolRscHandle];
          if (!handle3) {
            const rep = e[symbolRscRep] || ++captureCnt1;
            captureTable1.set(rep, e);
            handle3 = rscTableCreateOwn(handleTable1, rep);
          }
          dataView(memory0).setInt32(arg1 + 8, handle3, true);
          break;
        }
        case 'closed': {
          dataView(memory0).setInt8(arg1 + 4, 1, true);
          break;
        }
        default: {
          throw new TypeError(`invalid variant tag value \`${JSON.stringify(variant4.tag)}\` (received \`${variant4}\`) specified for \`StreamError\``);
        }
      }
      break;
    }
    default: {
      throw new TypeError('invalid variant specified for result');
    }
  }
  _debugLog('[iface="wasi:io/streams@0.2.6", function="[method]output-stream.blocking-flush"][Instruction::Return]', {
    funcName: '[method]output-stream.blocking-flush',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}


let lowered_import_13_metadata = {
  qualifiedImportFn: 'wasi:filesystem/types@0.2.6#[method]descriptor.read-via-stream',
  moduleIdx: null,
};

const handleTable6 = [T_FLAG, 0];
const captureTable6= new Map();
let captureCnt6 = 0;
handleTables[6] = handleTable6;

function trampoline21(arg0, arg1, arg2) {
  var handle1 = arg0;
  var rep2 = handleTable6[(handle1 << 1) + 1] & ~T_FLAG;
  var rsc0 = captureTable6.get(rep2);
  if (!rsc0) {
    rsc0 = Object.create(Descriptor.prototype);
    Object.defineProperty(rsc0, symbolRscHandle, { writable: true, value: handle1});
    Object.defineProperty(rsc0, symbolRscRep, { writable: true, value: rep2});
  }
  curResourceBorrows.push(rsc0);
  _debugLog('[iface="wasi:filesystem/types@0.2.6", function="[method]descriptor.read-via-stream"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = rsc0.readViaStream?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'readViaStream',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'result-catch-handler',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  
  let ret;
  try {
    ret = { tag: 'ok', val:  rsc0.readViaStream(BigInt.asUintN(64, arg1))};
  } catch (e) {
    ret = { tag: 'err', val: getErrorPayload(e) };
  }
  
  for (const rsc of curResourceBorrows) {
    rsc[symbolRscHandle] = undefined;
  }
  curResourceBorrows = [];
  endCurrentTask(0);
  var variant5 = ret;
  switch (variant5.tag) {
    case 'ok': {
      const e = variant5.val;
      dataView(memory0).setInt8(arg2 + 0, 0, true);
      if (!(e instanceof InputStream)) {
        throw new TypeError('Resource error: Not a valid "InputStream" resource.');
      }
      var handle3 = e[symbolRscHandle];
      if (!handle3) {
        const rep = e[symbolRscRep] || ++captureCnt2;
        captureTable2.set(rep, e);
        handle3 = rscTableCreateOwn(handleTable2, rep);
      }
      dataView(memory0).setInt32(arg2 + 4, handle3, true);
      break;
    }
    case 'err': {
      const e = variant5.val;
      dataView(memory0).setInt8(arg2 + 0, 1, true);
      var val4 = e;
      let enum4;
      switch (val4) {
        case 'access': {
          enum4 = 0;
          break;
        }
        case 'would-block': {
          enum4 = 1;
          break;
        }
        case 'already': {
          enum4 = 2;
          break;
        }
        case 'bad-descriptor': {
          enum4 = 3;
          break;
        }
        case 'busy': {
          enum4 = 4;
          break;
        }
        case 'deadlock': {
          enum4 = 5;
          break;
        }
        case 'quota': {
          enum4 = 6;
          break;
        }
        case 'exist': {
          enum4 = 7;
          break;
        }
        case 'file-too-large': {
          enum4 = 8;
          break;
        }
        case 'illegal-byte-sequence': {
          enum4 = 9;
          break;
        }
        case 'in-progress': {
          enum4 = 10;
          break;
        }
        case 'interrupted': {
          enum4 = 11;
          break;
        }
        case 'invalid': {
          enum4 = 12;
          break;
        }
        case 'io': {
          enum4 = 13;
          break;
        }
        case 'is-directory': {
          enum4 = 14;
          break;
        }
        case 'loop': {
          enum4 = 15;
          break;
        }
        case 'too-many-links': {
          enum4 = 16;
          break;
        }
        case 'message-size': {
          enum4 = 17;
          break;
        }
        case 'name-too-long': {
          enum4 = 18;
          break;
        }
        case 'no-device': {
          enum4 = 19;
          break;
        }
        case 'no-entry': {
          enum4 = 20;
          break;
        }
        case 'no-lock': {
          enum4 = 21;
          break;
        }
        case 'insufficient-memory': {
          enum4 = 22;
          break;
        }
        case 'insufficient-space': {
          enum4 = 23;
          break;
        }
        case 'not-directory': {
          enum4 = 24;
          break;
        }
        case 'not-empty': {
          enum4 = 25;
          break;
        }
        case 'not-recoverable': {
          enum4 = 26;
          break;
        }
        case 'unsupported': {
          enum4 = 27;
          break;
        }
        case 'no-tty': {
          enum4 = 28;
          break;
        }
        case 'no-such-device': {
          enum4 = 29;
          break;
        }
        case 'overflow': {
          enum4 = 30;
          break;
        }
        case 'not-permitted': {
          enum4 = 31;
          break;
        }
        case 'pipe': {
          enum4 = 32;
          break;
        }
        case 'read-only': {
          enum4 = 33;
          break;
        }
        case 'invalid-seek': {
          enum4 = 34;
          break;
        }
        case 'text-file-busy': {
          enum4 = 35;
          break;
        }
        case 'cross-device': {
          enum4 = 36;
          break;
        }
        default: {
          if ((e) instanceof Error) {
            console.error(e);
          }
          
          throw new TypeError(`"${val4}" is not one of the cases of error-code`);
        }
      }
      dataView(memory0).setInt8(arg2 + 4, enum4, true);
      break;
    }
    default: {
      throw new TypeError('invalid variant specified for result');
    }
  }
  _debugLog('[iface="wasi:filesystem/types@0.2.6", function="[method]descriptor.read-via-stream"][Instruction::Return]', {
    funcName: '[method]descriptor.read-via-stream',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}


let lowered_import_14_metadata = {
  qualifiedImportFn: 'wasi:filesystem/types@0.2.6#[method]descriptor.write-via-stream',
  moduleIdx: null,
};


function trampoline22(arg0, arg1, arg2) {
  var handle1 = arg0;
  var rep2 = handleTable6[(handle1 << 1) + 1] & ~T_FLAG;
  var rsc0 = captureTable6.get(rep2);
  if (!rsc0) {
    rsc0 = Object.create(Descriptor.prototype);
    Object.defineProperty(rsc0, symbolRscHandle, { writable: true, value: handle1});
    Object.defineProperty(rsc0, symbolRscRep, { writable: true, value: rep2});
  }
  curResourceBorrows.push(rsc0);
  _debugLog('[iface="wasi:filesystem/types@0.2.6", function="[method]descriptor.write-via-stream"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = rsc0.writeViaStream?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'writeViaStream',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'result-catch-handler',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  
  let ret;
  try {
    ret = { tag: 'ok', val:  rsc0.writeViaStream(BigInt.asUintN(64, arg1))};
  } catch (e) {
    ret = { tag: 'err', val: getErrorPayload(e) };
  }
  
  for (const rsc of curResourceBorrows) {
    rsc[symbolRscHandle] = undefined;
  }
  curResourceBorrows = [];
  endCurrentTask(0);
  var variant5 = ret;
  switch (variant5.tag) {
    case 'ok': {
      const e = variant5.val;
      dataView(memory0).setInt8(arg2 + 0, 0, true);
      if (!(e instanceof OutputStream)) {
        throw new TypeError('Resource error: Not a valid "OutputStream" resource.');
      }
      var handle3 = e[symbolRscHandle];
      if (!handle3) {
        const rep = e[symbolRscRep] || ++captureCnt3;
        captureTable3.set(rep, e);
        handle3 = rscTableCreateOwn(handleTable3, rep);
      }
      dataView(memory0).setInt32(arg2 + 4, handle3, true);
      break;
    }
    case 'err': {
      const e = variant5.val;
      dataView(memory0).setInt8(arg2 + 0, 1, true);
      var val4 = e;
      let enum4;
      switch (val4) {
        case 'access': {
          enum4 = 0;
          break;
        }
        case 'would-block': {
          enum4 = 1;
          break;
        }
        case 'already': {
          enum4 = 2;
          break;
        }
        case 'bad-descriptor': {
          enum4 = 3;
          break;
        }
        case 'busy': {
          enum4 = 4;
          break;
        }
        case 'deadlock': {
          enum4 = 5;
          break;
        }
        case 'quota': {
          enum4 = 6;
          break;
        }
        case 'exist': {
          enum4 = 7;
          break;
        }
        case 'file-too-large': {
          enum4 = 8;
          break;
        }
        case 'illegal-byte-sequence': {
          enum4 = 9;
          break;
        }
        case 'in-progress': {
          enum4 = 10;
          break;
        }
        case 'interrupted': {
          enum4 = 11;
          break;
        }
        case 'invalid': {
          enum4 = 12;
          break;
        }
        case 'io': {
          enum4 = 13;
          break;
        }
        case 'is-directory': {
          enum4 = 14;
          break;
        }
        case 'loop': {
          enum4 = 15;
          break;
        }
        case 'too-many-links': {
          enum4 = 16;
          break;
        }
        case 'message-size': {
          enum4 = 17;
          break;
        }
        case 'name-too-long': {
          enum4 = 18;
          break;
        }
        case 'no-device': {
          enum4 = 19;
          break;
        }
        case 'no-entry': {
          enum4 = 20;
          break;
        }
        case 'no-lock': {
          enum4 = 21;
          break;
        }
        case 'insufficient-memory': {
          enum4 = 22;
          break;
        }
        case 'insufficient-space': {
          enum4 = 23;
          break;
        }
        case 'not-directory': {
          enum4 = 24;
          break;
        }
        case 'not-empty': {
          enum4 = 25;
          break;
        }
        case 'not-recoverable': {
          enum4 = 26;
          break;
        }
        case 'unsupported': {
          enum4 = 27;
          break;
        }
        case 'no-tty': {
          enum4 = 28;
          break;
        }
        case 'no-such-device': {
          enum4 = 29;
          break;
        }
        case 'overflow': {
          enum4 = 30;
          break;
        }
        case 'not-permitted': {
          enum4 = 31;
          break;
        }
        case 'pipe': {
          enum4 = 32;
          break;
        }
        case 'read-only': {
          enum4 = 33;
          break;
        }
        case 'invalid-seek': {
          enum4 = 34;
          break;
        }
        case 'text-file-busy': {
          enum4 = 35;
          break;
        }
        case 'cross-device': {
          enum4 = 36;
          break;
        }
        default: {
          if ((e) instanceof Error) {
            console.error(e);
          }
          
          throw new TypeError(`"${val4}" is not one of the cases of error-code`);
        }
      }
      dataView(memory0).setInt8(arg2 + 4, enum4, true);
      break;
    }
    default: {
      throw new TypeError('invalid variant specified for result');
    }
  }
  _debugLog('[iface="wasi:filesystem/types@0.2.6", function="[method]descriptor.write-via-stream"][Instruction::Return]', {
    funcName: '[method]descriptor.write-via-stream',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}


let lowered_import_15_metadata = {
  qualifiedImportFn: 'wasi:filesystem/types@0.2.6#[method]descriptor.append-via-stream',
  moduleIdx: null,
};


function trampoline23(arg0, arg1) {
  var handle1 = arg0;
  var rep2 = handleTable6[(handle1 << 1) + 1] & ~T_FLAG;
  var rsc0 = captureTable6.get(rep2);
  if (!rsc0) {
    rsc0 = Object.create(Descriptor.prototype);
    Object.defineProperty(rsc0, symbolRscHandle, { writable: true, value: handle1});
    Object.defineProperty(rsc0, symbolRscRep, { writable: true, value: rep2});
  }
  curResourceBorrows.push(rsc0);
  _debugLog('[iface="wasi:filesystem/types@0.2.6", function="[method]descriptor.append-via-stream"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = rsc0.appendViaStream?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'appendViaStream',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'result-catch-handler',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  
  let ret;
  try {
    ret = { tag: 'ok', val:  rsc0.appendViaStream()};
  } catch (e) {
    ret = { tag: 'err', val: getErrorPayload(e) };
  }
  
  for (const rsc of curResourceBorrows) {
    rsc[symbolRscHandle] = undefined;
  }
  curResourceBorrows = [];
  endCurrentTask(0);
  var variant5 = ret;
  switch (variant5.tag) {
    case 'ok': {
      const e = variant5.val;
      dataView(memory0).setInt8(arg1 + 0, 0, true);
      if (!(e instanceof OutputStream)) {
        throw new TypeError('Resource error: Not a valid "OutputStream" resource.');
      }
      var handle3 = e[symbolRscHandle];
      if (!handle3) {
        const rep = e[symbolRscRep] || ++captureCnt3;
        captureTable3.set(rep, e);
        handle3 = rscTableCreateOwn(handleTable3, rep);
      }
      dataView(memory0).setInt32(arg1 + 4, handle3, true);
      break;
    }
    case 'err': {
      const e = variant5.val;
      dataView(memory0).setInt8(arg1 + 0, 1, true);
      var val4 = e;
      let enum4;
      switch (val4) {
        case 'access': {
          enum4 = 0;
          break;
        }
        case 'would-block': {
          enum4 = 1;
          break;
        }
        case 'already': {
          enum4 = 2;
          break;
        }
        case 'bad-descriptor': {
          enum4 = 3;
          break;
        }
        case 'busy': {
          enum4 = 4;
          break;
        }
        case 'deadlock': {
          enum4 = 5;
          break;
        }
        case 'quota': {
          enum4 = 6;
          break;
        }
        case 'exist': {
          enum4 = 7;
          break;
        }
        case 'file-too-large': {
          enum4 = 8;
          break;
        }
        case 'illegal-byte-sequence': {
          enum4 = 9;
          break;
        }
        case 'in-progress': {
          enum4 = 10;
          break;
        }
        case 'interrupted': {
          enum4 = 11;
          break;
        }
        case 'invalid': {
          enum4 = 12;
          break;
        }
        case 'io': {
          enum4 = 13;
          break;
        }
        case 'is-directory': {
          enum4 = 14;
          break;
        }
        case 'loop': {
          enum4 = 15;
          break;
        }
        case 'too-many-links': {
          enum4 = 16;
          break;
        }
        case 'message-size': {
          enum4 = 17;
          break;
        }
        case 'name-too-long': {
          enum4 = 18;
          break;
        }
        case 'no-device': {
          enum4 = 19;
          break;
        }
        case 'no-entry': {
          enum4 = 20;
          break;
        }
        case 'no-lock': {
          enum4 = 21;
          break;
        }
        case 'insufficient-memory': {
          enum4 = 22;
          break;
        }
        case 'insufficient-space': {
          enum4 = 23;
          break;
        }
        case 'not-directory': {
          enum4 = 24;
          break;
        }
        case 'not-empty': {
          enum4 = 25;
          break;
        }
        case 'not-recoverable': {
          enum4 = 26;
          break;
        }
        case 'unsupported': {
          enum4 = 27;
          break;
        }
        case 'no-tty': {
          enum4 = 28;
          break;
        }
        case 'no-such-device': {
          enum4 = 29;
          break;
        }
        case 'overflow': {
          enum4 = 30;
          break;
        }
        case 'not-permitted': {
          enum4 = 31;
          break;
        }
        case 'pipe': {
          enum4 = 32;
          break;
        }
        case 'read-only': {
          enum4 = 33;
          break;
        }
        case 'invalid-seek': {
          enum4 = 34;
          break;
        }
        case 'text-file-busy': {
          enum4 = 35;
          break;
        }
        case 'cross-device': {
          enum4 = 36;
          break;
        }
        default: {
          if ((e) instanceof Error) {
            console.error(e);
          }
          
          throw new TypeError(`"${val4}" is not one of the cases of error-code`);
        }
      }
      dataView(memory0).setInt8(arg1 + 4, enum4, true);
      break;
    }
    default: {
      throw new TypeError('invalid variant specified for result');
    }
  }
  _debugLog('[iface="wasi:filesystem/types@0.2.6", function="[method]descriptor.append-via-stream"][Instruction::Return]', {
    funcName: '[method]descriptor.append-via-stream',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}


let lowered_import_16_metadata = {
  qualifiedImportFn: 'wasi:filesystem/types@0.2.6#[method]descriptor.get-flags',
  moduleIdx: null,
};


function trampoline24(arg0, arg1) {
  var handle1 = arg0;
  var rep2 = handleTable6[(handle1 << 1) + 1] & ~T_FLAG;
  var rsc0 = captureTable6.get(rep2);
  if (!rsc0) {
    rsc0 = Object.create(Descriptor.prototype);
    Object.defineProperty(rsc0, symbolRscHandle, { writable: true, value: handle1});
    Object.defineProperty(rsc0, symbolRscRep, { writable: true, value: rep2});
  }
  curResourceBorrows.push(rsc0);
  _debugLog('[iface="wasi:filesystem/types@0.2.6", function="[method]descriptor.get-flags"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = rsc0.getFlags?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'getFlags',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'result-catch-handler',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  
  let ret;
  try {
    ret = { tag: 'ok', val:  rsc0.getFlags()};
  } catch (e) {
    ret = { tag: 'err', val: getErrorPayload(e) };
  }
  
  for (const rsc of curResourceBorrows) {
    rsc[symbolRscHandle] = undefined;
  }
  curResourceBorrows = [];
  endCurrentTask(0);
  var variant5 = ret;
  switch (variant5.tag) {
    case 'ok': {
      const e = variant5.val;
      dataView(memory0).setInt8(arg1 + 0, 0, true);
      let flags3 = 0;
      if (typeof e === 'object' && e !== null) {
        flags3 = Boolean(e.read) << 0 | Boolean(e.write) << 1 | Boolean(e.fileIntegritySync) << 2 | Boolean(e.dataIntegritySync) << 3 | Boolean(e.requestedWriteSync) << 4 | Boolean(e.mutateDirectory) << 5;
      } else if (e !== null && e!== undefined) {
        throw new TypeError('only an object, undefined or null can be converted to flags');
      }
      dataView(memory0).setInt8(arg1 + 1, flags3, true);
      break;
    }
    case 'err': {
      const e = variant5.val;
      dataView(memory0).setInt8(arg1 + 0, 1, true);
      var val4 = e;
      let enum4;
      switch (val4) {
        case 'access': {
          enum4 = 0;
          break;
        }
        case 'would-block': {
          enum4 = 1;
          break;
        }
        case 'already': {
          enum4 = 2;
          break;
        }
        case 'bad-descriptor': {
          enum4 = 3;
          break;
        }
        case 'busy': {
          enum4 = 4;
          break;
        }
        case 'deadlock': {
          enum4 = 5;
          break;
        }
        case 'quota': {
          enum4 = 6;
          break;
        }
        case 'exist': {
          enum4 = 7;
          break;
        }
        case 'file-too-large': {
          enum4 = 8;
          break;
        }
        case 'illegal-byte-sequence': {
          enum4 = 9;
          break;
        }
        case 'in-progress': {
          enum4 = 10;
          break;
        }
        case 'interrupted': {
          enum4 = 11;
          break;
        }
        case 'invalid': {
          enum4 = 12;
          break;
        }
        case 'io': {
          enum4 = 13;
          break;
        }
        case 'is-directory': {
          enum4 = 14;
          break;
        }
        case 'loop': {
          enum4 = 15;
          break;
        }
        case 'too-many-links': {
          enum4 = 16;
          break;
        }
        case 'message-size': {
          enum4 = 17;
          break;
        }
        case 'name-too-long': {
          enum4 = 18;
          break;
        }
        case 'no-device': {
          enum4 = 19;
          break;
        }
        case 'no-entry': {
          enum4 = 20;
          break;
        }
        case 'no-lock': {
          enum4 = 21;
          break;
        }
        case 'insufficient-memory': {
          enum4 = 22;
          break;
        }
        case 'insufficient-space': {
          enum4 = 23;
          break;
        }
        case 'not-directory': {
          enum4 = 24;
          break;
        }
        case 'not-empty': {
          enum4 = 25;
          break;
        }
        case 'not-recoverable': {
          enum4 = 26;
          break;
        }
        case 'unsupported': {
          enum4 = 27;
          break;
        }
        case 'no-tty': {
          enum4 = 28;
          break;
        }
        case 'no-such-device': {
          enum4 = 29;
          break;
        }
        case 'overflow': {
          enum4 = 30;
          break;
        }
        case 'not-permitted': {
          enum4 = 31;
          break;
        }
        case 'pipe': {
          enum4 = 32;
          break;
        }
        case 'read-only': {
          enum4 = 33;
          break;
        }
        case 'invalid-seek': {
          enum4 = 34;
          break;
        }
        case 'text-file-busy': {
          enum4 = 35;
          break;
        }
        case 'cross-device': {
          enum4 = 36;
          break;
        }
        default: {
          if ((e) instanceof Error) {
            console.error(e);
          }
          
          throw new TypeError(`"${val4}" is not one of the cases of error-code`);
        }
      }
      dataView(memory0).setInt8(arg1 + 1, enum4, true);
      break;
    }
    default: {
      throw new TypeError('invalid variant specified for result');
    }
  }
  _debugLog('[iface="wasi:filesystem/types@0.2.6", function="[method]descriptor.get-flags"][Instruction::Return]', {
    funcName: '[method]descriptor.get-flags',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}


let lowered_import_17_metadata = {
  qualifiedImportFn: 'wasi:filesystem/types@0.2.6#[method]descriptor.read-directory',
  moduleIdx: null,
};

const handleTable7 = [T_FLAG, 0];
const captureTable7= new Map();
let captureCnt7 = 0;
handleTables[7] = handleTable7;

function trampoline25(arg0, arg1) {
  var handle1 = arg0;
  var rep2 = handleTable6[(handle1 << 1) + 1] & ~T_FLAG;
  var rsc0 = captureTable6.get(rep2);
  if (!rsc0) {
    rsc0 = Object.create(Descriptor.prototype);
    Object.defineProperty(rsc0, symbolRscHandle, { writable: true, value: handle1});
    Object.defineProperty(rsc0, symbolRscRep, { writable: true, value: rep2});
  }
  curResourceBorrows.push(rsc0);
  _debugLog('[iface="wasi:filesystem/types@0.2.6", function="[method]descriptor.read-directory"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = rsc0.readDirectory?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'readDirectory',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'result-catch-handler',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  
  let ret;
  try {
    ret = { tag: 'ok', val:  rsc0.readDirectory()};
  } catch (e) {
    ret = { tag: 'err', val: getErrorPayload(e) };
  }
  
  for (const rsc of curResourceBorrows) {
    rsc[symbolRscHandle] = undefined;
  }
  curResourceBorrows = [];
  endCurrentTask(0);
  var variant5 = ret;
  switch (variant5.tag) {
    case 'ok': {
      const e = variant5.val;
      dataView(memory0).setInt8(arg1 + 0, 0, true);
      if (!(e instanceof DirectoryEntryStream)) {
        throw new TypeError('Resource error: Not a valid "DirectoryEntryStream" resource.');
      }
      var handle3 = e[symbolRscHandle];
      if (!handle3) {
        const rep = e[symbolRscRep] || ++captureCnt7;
        captureTable7.set(rep, e);
        handle3 = rscTableCreateOwn(handleTable7, rep);
      }
      dataView(memory0).setInt32(arg1 + 4, handle3, true);
      break;
    }
    case 'err': {
      const e = variant5.val;
      dataView(memory0).setInt8(arg1 + 0, 1, true);
      var val4 = e;
      let enum4;
      switch (val4) {
        case 'access': {
          enum4 = 0;
          break;
        }
        case 'would-block': {
          enum4 = 1;
          break;
        }
        case 'already': {
          enum4 = 2;
          break;
        }
        case 'bad-descriptor': {
          enum4 = 3;
          break;
        }
        case 'busy': {
          enum4 = 4;
          break;
        }
        case 'deadlock': {
          enum4 = 5;
          break;
        }
        case 'quota': {
          enum4 = 6;
          break;
        }
        case 'exist': {
          enum4 = 7;
          break;
        }
        case 'file-too-large': {
          enum4 = 8;
          break;
        }
        case 'illegal-byte-sequence': {
          enum4 = 9;
          break;
        }
        case 'in-progress': {
          enum4 = 10;
          break;
        }
        case 'interrupted': {
          enum4 = 11;
          break;
        }
        case 'invalid': {
          enum4 = 12;
          break;
        }
        case 'io': {
          enum4 = 13;
          break;
        }
        case 'is-directory': {
          enum4 = 14;
          break;
        }
        case 'loop': {
          enum4 = 15;
          break;
        }
        case 'too-many-links': {
          enum4 = 16;
          break;
        }
        case 'message-size': {
          enum4 = 17;
          break;
        }
        case 'name-too-long': {
          enum4 = 18;
          break;
        }
        case 'no-device': {
          enum4 = 19;
          break;
        }
        case 'no-entry': {
          enum4 = 20;
          break;
        }
        case 'no-lock': {
          enum4 = 21;
          break;
        }
        case 'insufficient-memory': {
          enum4 = 22;
          break;
        }
        case 'insufficient-space': {
          enum4 = 23;
          break;
        }
        case 'not-directory': {
          enum4 = 24;
          break;
        }
        case 'not-empty': {
          enum4 = 25;
          break;
        }
        case 'not-recoverable': {
          enum4 = 26;
          break;
        }
        case 'unsupported': {
          enum4 = 27;
          break;
        }
        case 'no-tty': {
          enum4 = 28;
          break;
        }
        case 'no-such-device': {
          enum4 = 29;
          break;
        }
        case 'overflow': {
          enum4 = 30;
          break;
        }
        case 'not-permitted': {
          enum4 = 31;
          break;
        }
        case 'pipe': {
          enum4 = 32;
          break;
        }
        case 'read-only': {
          enum4 = 33;
          break;
        }
        case 'invalid-seek': {
          enum4 = 34;
          break;
        }
        case 'text-file-busy': {
          enum4 = 35;
          break;
        }
        case 'cross-device': {
          enum4 = 36;
          break;
        }
        default: {
          if ((e) instanceof Error) {
            console.error(e);
          }
          
          throw new TypeError(`"${val4}" is not one of the cases of error-code`);
        }
      }
      dataView(memory0).setInt8(arg1 + 4, enum4, true);
      break;
    }
    default: {
      throw new TypeError('invalid variant specified for result');
    }
  }
  _debugLog('[iface="wasi:filesystem/types@0.2.6", function="[method]descriptor.read-directory"][Instruction::Return]', {
    funcName: '[method]descriptor.read-directory',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}


let lowered_import_18_metadata = {
  qualifiedImportFn: 'wasi:filesystem/types@0.2.6#[method]descriptor.stat',
  moduleIdx: null,
};


function trampoline26(arg0, arg1) {
  var handle1 = arg0;
  var rep2 = handleTable6[(handle1 << 1) + 1] & ~T_FLAG;
  var rsc0 = captureTable6.get(rep2);
  if (!rsc0) {
    rsc0 = Object.create(Descriptor.prototype);
    Object.defineProperty(rsc0, symbolRscHandle, { writable: true, value: handle1});
    Object.defineProperty(rsc0, symbolRscRep, { writable: true, value: rep2});
  }
  curResourceBorrows.push(rsc0);
  _debugLog('[iface="wasi:filesystem/types@0.2.6", function="[method]descriptor.stat"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = rsc0.stat?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'stat',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'result-catch-handler',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  
  let ret;
  try {
    ret = { tag: 'ok', val:  rsc0.stat()};
  } catch (e) {
    ret = { tag: 'err', val: getErrorPayload(e) };
  }
  
  for (const rsc of curResourceBorrows) {
    rsc[symbolRscHandle] = undefined;
  }
  curResourceBorrows = [];
  endCurrentTask(0);
  var variant12 = ret;
  switch (variant12.tag) {
    case 'ok': {
      const e = variant12.val;
      dataView(memory0).setInt8(arg1 + 0, 0, true);
      var {type: v3_0, linkCount: v3_1, size: v3_2, dataAccessTimestamp: v3_3, dataModificationTimestamp: v3_4, statusChangeTimestamp: v3_5 } = e;
      var val4 = v3_0;
      let enum4;
      switch (val4) {
        case 'unknown': {
          enum4 = 0;
          break;
        }
        case 'block-device': {
          enum4 = 1;
          break;
        }
        case 'character-device': {
          enum4 = 2;
          break;
        }
        case 'directory': {
          enum4 = 3;
          break;
        }
        case 'fifo': {
          enum4 = 4;
          break;
        }
        case 'symbolic-link': {
          enum4 = 5;
          break;
        }
        case 'regular-file': {
          enum4 = 6;
          break;
        }
        case 'socket': {
          enum4 = 7;
          break;
        }
        default: {
          if ((v3_0) instanceof Error) {
            console.error(v3_0);
          }
          
          throw new TypeError(`"${val4}" is not one of the cases of descriptor-type`);
        }
      }
      dataView(memory0).setInt8(arg1 + 8, enum4, true);
      dataView(memory0).setBigInt64(arg1 + 16, toUint64(v3_1), true);
      dataView(memory0).setBigInt64(arg1 + 24, toUint64(v3_2), true);
      var variant6 = v3_3;
      if (variant6 === null || variant6=== undefined) {
        dataView(memory0).setInt8(arg1 + 32, 0, true);
      } else {
        const e = variant6;
        dataView(memory0).setInt8(arg1 + 32, 1, true);
        var {seconds: v5_0, nanoseconds: v5_1 } = e;
        dataView(memory0).setBigInt64(arg1 + 40, toUint64(v5_0), true);
        dataView(memory0).setInt32(arg1 + 48, toUint32(v5_1), true);
      }
      var variant8 = v3_4;
      if (variant8 === null || variant8=== undefined) {
        dataView(memory0).setInt8(arg1 + 56, 0, true);
      } else {
        const e = variant8;
        dataView(memory0).setInt8(arg1 + 56, 1, true);
        var {seconds: v7_0, nanoseconds: v7_1 } = e;
        dataView(memory0).setBigInt64(arg1 + 64, toUint64(v7_0), true);
        dataView(memory0).setInt32(arg1 + 72, toUint32(v7_1), true);
      }
      var variant10 = v3_5;
      if (variant10 === null || variant10=== undefined) {
        dataView(memory0).setInt8(arg1 + 80, 0, true);
      } else {
        const e = variant10;
        dataView(memory0).setInt8(arg1 + 80, 1, true);
        var {seconds: v9_0, nanoseconds: v9_1 } = e;
        dataView(memory0).setBigInt64(arg1 + 88, toUint64(v9_0), true);
        dataView(memory0).setInt32(arg1 + 96, toUint32(v9_1), true);
      }
      break;
    }
    case 'err': {
      const e = variant12.val;
      dataView(memory0).setInt8(arg1 + 0, 1, true);
      var val11 = e;
      let enum11;
      switch (val11) {
        case 'access': {
          enum11 = 0;
          break;
        }
        case 'would-block': {
          enum11 = 1;
          break;
        }
        case 'already': {
          enum11 = 2;
          break;
        }
        case 'bad-descriptor': {
          enum11 = 3;
          break;
        }
        case 'busy': {
          enum11 = 4;
          break;
        }
        case 'deadlock': {
          enum11 = 5;
          break;
        }
        case 'quota': {
          enum11 = 6;
          break;
        }
        case 'exist': {
          enum11 = 7;
          break;
        }
        case 'file-too-large': {
          enum11 = 8;
          break;
        }
        case 'illegal-byte-sequence': {
          enum11 = 9;
          break;
        }
        case 'in-progress': {
          enum11 = 10;
          break;
        }
        case 'interrupted': {
          enum11 = 11;
          break;
        }
        case 'invalid': {
          enum11 = 12;
          break;
        }
        case 'io': {
          enum11 = 13;
          break;
        }
        case 'is-directory': {
          enum11 = 14;
          break;
        }
        case 'loop': {
          enum11 = 15;
          break;
        }
        case 'too-many-links': {
          enum11 = 16;
          break;
        }
        case 'message-size': {
          enum11 = 17;
          break;
        }
        case 'name-too-long': {
          enum11 = 18;
          break;
        }
        case 'no-device': {
          enum11 = 19;
          break;
        }
        case 'no-entry': {
          enum11 = 20;
          break;
        }
        case 'no-lock': {
          enum11 = 21;
          break;
        }
        case 'insufficient-memory': {
          enum11 = 22;
          break;
        }
        case 'insufficient-space': {
          enum11 = 23;
          break;
        }
        case 'not-directory': {
          enum11 = 24;
          break;
        }
        case 'not-empty': {
          enum11 = 25;
          break;
        }
        case 'not-recoverable': {
          enum11 = 26;
          break;
        }
        case 'unsupported': {
          enum11 = 27;
          break;
        }
        case 'no-tty': {
          enum11 = 28;
          break;
        }
        case 'no-such-device': {
          enum11 = 29;
          break;
        }
        case 'overflow': {
          enum11 = 30;
          break;
        }
        case 'not-permitted': {
          enum11 = 31;
          break;
        }
        case 'pipe': {
          enum11 = 32;
          break;
        }
        case 'read-only': {
          enum11 = 33;
          break;
        }
        case 'invalid-seek': {
          enum11 = 34;
          break;
        }
        case 'text-file-busy': {
          enum11 = 35;
          break;
        }
        case 'cross-device': {
          enum11 = 36;
          break;
        }
        default: {
          if ((e) instanceof Error) {
            console.error(e);
          }
          
          throw new TypeError(`"${val11}" is not one of the cases of error-code`);
        }
      }
      dataView(memory0).setInt8(arg1 + 8, enum11, true);
      break;
    }
    default: {
      throw new TypeError('invalid variant specified for result');
    }
  }
  _debugLog('[iface="wasi:filesystem/types@0.2.6", function="[method]descriptor.stat"][Instruction::Return]', {
    funcName: '[method]descriptor.stat',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}


let lowered_import_19_metadata = {
  qualifiedImportFn: 'wasi:filesystem/types@0.2.6#[method]descriptor.open-at',
  moduleIdx: null,
};


function trampoline27(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
  var handle1 = arg0;
  var rep2 = handleTable6[(handle1 << 1) + 1] & ~T_FLAG;
  var rsc0 = captureTable6.get(rep2);
  if (!rsc0) {
    rsc0 = Object.create(Descriptor.prototype);
    Object.defineProperty(rsc0, symbolRscHandle, { writable: true, value: handle1});
    Object.defineProperty(rsc0, symbolRscRep, { writable: true, value: rep2});
  }
  curResourceBorrows.push(rsc0);
  if ((arg1 & 4294967294) !== 0) {
    throw new TypeError('flags have extraneous bits set');
  }
  var flags3 = {
    symlinkFollow: Boolean(arg1 & 1),
  };
  var ptr4 = arg2;
  var len4 = arg3;
  var result4 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr4, len4));
  if ((arg4 & 4294967280) !== 0) {
    throw new TypeError('flags have extraneous bits set');
  }
  var flags5 = {
    create: Boolean(arg4 & 1),
    directory: Boolean(arg4 & 2),
    exclusive: Boolean(arg4 & 4),
    truncate: Boolean(arg4 & 8),
  };
  if ((arg5 & 4294967232) !== 0) {
    throw new TypeError('flags have extraneous bits set');
  }
  var flags6 = {
    read: Boolean(arg5 & 1),
    write: Boolean(arg5 & 2),
    fileIntegritySync: Boolean(arg5 & 4),
    dataIntegritySync: Boolean(arg5 & 8),
    requestedWriteSync: Boolean(arg5 & 16),
    mutateDirectory: Boolean(arg5 & 32),
  };
  _debugLog('[iface="wasi:filesystem/types@0.2.6", function="[method]descriptor.open-at"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = rsc0.openAt?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'openAt',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'result-catch-handler',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  
  let ret;
  try {
    ret = { tag: 'ok', val:  rsc0.openAt(flags3, result4, flags5, flags6)};
  } catch (e) {
    ret = { tag: 'err', val: getErrorPayload(e) };
  }
  
  for (const rsc of curResourceBorrows) {
    rsc[symbolRscHandle] = undefined;
  }
  curResourceBorrows = [];
  endCurrentTask(0);
  var variant9 = ret;
  switch (variant9.tag) {
    case 'ok': {
      const e = variant9.val;
      dataView(memory0).setInt8(arg6 + 0, 0, true);
      if (!(e instanceof Descriptor)) {
        throw new TypeError('Resource error: Not a valid "Descriptor" resource.');
      }
      var handle7 = e[symbolRscHandle];
      if (!handle7) {
        const rep = e[symbolRscRep] || ++captureCnt6;
        captureTable6.set(rep, e);
        handle7 = rscTableCreateOwn(handleTable6, rep);
      }
      dataView(memory0).setInt32(arg6 + 4, handle7, true);
      break;
    }
    case 'err': {
      const e = variant9.val;
      dataView(memory0).setInt8(arg6 + 0, 1, true);
      var val8 = e;
      let enum8;
      switch (val8) {
        case 'access': {
          enum8 = 0;
          break;
        }
        case 'would-block': {
          enum8 = 1;
          break;
        }
        case 'already': {
          enum8 = 2;
          break;
        }
        case 'bad-descriptor': {
          enum8 = 3;
          break;
        }
        case 'busy': {
          enum8 = 4;
          break;
        }
        case 'deadlock': {
          enum8 = 5;
          break;
        }
        case 'quota': {
          enum8 = 6;
          break;
        }
        case 'exist': {
          enum8 = 7;
          break;
        }
        case 'file-too-large': {
          enum8 = 8;
          break;
        }
        case 'illegal-byte-sequence': {
          enum8 = 9;
          break;
        }
        case 'in-progress': {
          enum8 = 10;
          break;
        }
        case 'interrupted': {
          enum8 = 11;
          break;
        }
        case 'invalid': {
          enum8 = 12;
          break;
        }
        case 'io': {
          enum8 = 13;
          break;
        }
        case 'is-directory': {
          enum8 = 14;
          break;
        }
        case 'loop': {
          enum8 = 15;
          break;
        }
        case 'too-many-links': {
          enum8 = 16;
          break;
        }
        case 'message-size': {
          enum8 = 17;
          break;
        }
        case 'name-too-long': {
          enum8 = 18;
          break;
        }
        case 'no-device': {
          enum8 = 19;
          break;
        }
        case 'no-entry': {
          enum8 = 20;
          break;
        }
        case 'no-lock': {
          enum8 = 21;
          break;
        }
        case 'insufficient-memory': {
          enum8 = 22;
          break;
        }
        case 'insufficient-space': {
          enum8 = 23;
          break;
        }
        case 'not-directory': {
          enum8 = 24;
          break;
        }
        case 'not-empty': {
          enum8 = 25;
          break;
        }
        case 'not-recoverable': {
          enum8 = 26;
          break;
        }
        case 'unsupported': {
          enum8 = 27;
          break;
        }
        case 'no-tty': {
          enum8 = 28;
          break;
        }
        case 'no-such-device': {
          enum8 = 29;
          break;
        }
        case 'overflow': {
          enum8 = 30;
          break;
        }
        case 'not-permitted': {
          enum8 = 31;
          break;
        }
        case 'pipe': {
          enum8 = 32;
          break;
        }
        case 'read-only': {
          enum8 = 33;
          break;
        }
        case 'invalid-seek': {
          enum8 = 34;
          break;
        }
        case 'text-file-busy': {
          enum8 = 35;
          break;
        }
        case 'cross-device': {
          enum8 = 36;
          break;
        }
        default: {
          if ((e) instanceof Error) {
            console.error(e);
          }
          
          throw new TypeError(`"${val8}" is not one of the cases of error-code`);
        }
      }
      dataView(memory0).setInt8(arg6 + 4, enum8, true);
      break;
    }
    default: {
      throw new TypeError('invalid variant specified for result');
    }
  }
  _debugLog('[iface="wasi:filesystem/types@0.2.6", function="[method]descriptor.open-at"][Instruction::Return]', {
    funcName: '[method]descriptor.open-at',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}


let lowered_import_20_metadata = {
  qualifiedImportFn: 'wasi:filesystem/types@0.2.6#[method]descriptor.metadata-hash',
  moduleIdx: null,
};


function trampoline28(arg0, arg1) {
  var handle1 = arg0;
  var rep2 = handleTable6[(handle1 << 1) + 1] & ~T_FLAG;
  var rsc0 = captureTable6.get(rep2);
  if (!rsc0) {
    rsc0 = Object.create(Descriptor.prototype);
    Object.defineProperty(rsc0, symbolRscHandle, { writable: true, value: handle1});
    Object.defineProperty(rsc0, symbolRscRep, { writable: true, value: rep2});
  }
  curResourceBorrows.push(rsc0);
  _debugLog('[iface="wasi:filesystem/types@0.2.6", function="[method]descriptor.metadata-hash"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = rsc0.metadataHash?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'metadataHash',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'result-catch-handler',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  
  let ret;
  try {
    ret = { tag: 'ok', val:  rsc0.metadataHash()};
  } catch (e) {
    ret = { tag: 'err', val: getErrorPayload(e) };
  }
  
  for (const rsc of curResourceBorrows) {
    rsc[symbolRscHandle] = undefined;
  }
  curResourceBorrows = [];
  endCurrentTask(0);
  var variant5 = ret;
  switch (variant5.tag) {
    case 'ok': {
      const e = variant5.val;
      dataView(memory0).setInt8(arg1 + 0, 0, true);
      var {lower: v3_0, upper: v3_1 } = e;
      dataView(memory0).setBigInt64(arg1 + 8, toUint64(v3_0), true);
      dataView(memory0).setBigInt64(arg1 + 16, toUint64(v3_1), true);
      break;
    }
    case 'err': {
      const e = variant5.val;
      dataView(memory0).setInt8(arg1 + 0, 1, true);
      var val4 = e;
      let enum4;
      switch (val4) {
        case 'access': {
          enum4 = 0;
          break;
        }
        case 'would-block': {
          enum4 = 1;
          break;
        }
        case 'already': {
          enum4 = 2;
          break;
        }
        case 'bad-descriptor': {
          enum4 = 3;
          break;
        }
        case 'busy': {
          enum4 = 4;
          break;
        }
        case 'deadlock': {
          enum4 = 5;
          break;
        }
        case 'quota': {
          enum4 = 6;
          break;
        }
        case 'exist': {
          enum4 = 7;
          break;
        }
        case 'file-too-large': {
          enum4 = 8;
          break;
        }
        case 'illegal-byte-sequence': {
          enum4 = 9;
          break;
        }
        case 'in-progress': {
          enum4 = 10;
          break;
        }
        case 'interrupted': {
          enum4 = 11;
          break;
        }
        case 'invalid': {
          enum4 = 12;
          break;
        }
        case 'io': {
          enum4 = 13;
          break;
        }
        case 'is-directory': {
          enum4 = 14;
          break;
        }
        case 'loop': {
          enum4 = 15;
          break;
        }
        case 'too-many-links': {
          enum4 = 16;
          break;
        }
        case 'message-size': {
          enum4 = 17;
          break;
        }
        case 'name-too-long': {
          enum4 = 18;
          break;
        }
        case 'no-device': {
          enum4 = 19;
          break;
        }
        case 'no-entry': {
          enum4 = 20;
          break;
        }
        case 'no-lock': {
          enum4 = 21;
          break;
        }
        case 'insufficient-memory': {
          enum4 = 22;
          break;
        }
        case 'insufficient-space': {
          enum4 = 23;
          break;
        }
        case 'not-directory': {
          enum4 = 24;
          break;
        }
        case 'not-empty': {
          enum4 = 25;
          break;
        }
        case 'not-recoverable': {
          enum4 = 26;
          break;
        }
        case 'unsupported': {
          enum4 = 27;
          break;
        }
        case 'no-tty': {
          enum4 = 28;
          break;
        }
        case 'no-such-device': {
          enum4 = 29;
          break;
        }
        case 'overflow': {
          enum4 = 30;
          break;
        }
        case 'not-permitted': {
          enum4 = 31;
          break;
        }
        case 'pipe': {
          enum4 = 32;
          break;
        }
        case 'read-only': {
          enum4 = 33;
          break;
        }
        case 'invalid-seek': {
          enum4 = 34;
          break;
        }
        case 'text-file-busy': {
          enum4 = 35;
          break;
        }
        case 'cross-device': {
          enum4 = 36;
          break;
        }
        default: {
          if ((e) instanceof Error) {
            console.error(e);
          }
          
          throw new TypeError(`"${val4}" is not one of the cases of error-code`);
        }
      }
      dataView(memory0).setInt8(arg1 + 8, enum4, true);
      break;
    }
    default: {
      throw new TypeError('invalid variant specified for result');
    }
  }
  _debugLog('[iface="wasi:filesystem/types@0.2.6", function="[method]descriptor.metadata-hash"][Instruction::Return]', {
    funcName: '[method]descriptor.metadata-hash',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}


let lowered_import_21_metadata = {
  qualifiedImportFn: 'wasi:filesystem/types@0.2.6#[method]descriptor.metadata-hash-at',
  moduleIdx: null,
};


function trampoline29(arg0, arg1, arg2, arg3, arg4) {
  var handle1 = arg0;
  var rep2 = handleTable6[(handle1 << 1) + 1] & ~T_FLAG;
  var rsc0 = captureTable6.get(rep2);
  if (!rsc0) {
    rsc0 = Object.create(Descriptor.prototype);
    Object.defineProperty(rsc0, symbolRscHandle, { writable: true, value: handle1});
    Object.defineProperty(rsc0, symbolRscRep, { writable: true, value: rep2});
  }
  curResourceBorrows.push(rsc0);
  if ((arg1 & 4294967294) !== 0) {
    throw new TypeError('flags have extraneous bits set');
  }
  var flags3 = {
    symlinkFollow: Boolean(arg1 & 1),
  };
  var ptr4 = arg2;
  var len4 = arg3;
  var result4 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr4, len4));
  _debugLog('[iface="wasi:filesystem/types@0.2.6", function="[method]descriptor.metadata-hash-at"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = rsc0.metadataHashAt?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'metadataHashAt',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'result-catch-handler',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  
  let ret;
  try {
    ret = { tag: 'ok', val:  rsc0.metadataHashAt(flags3, result4)};
  } catch (e) {
    ret = { tag: 'err', val: getErrorPayload(e) };
  }
  
  for (const rsc of curResourceBorrows) {
    rsc[symbolRscHandle] = undefined;
  }
  curResourceBorrows = [];
  endCurrentTask(0);
  var variant7 = ret;
  switch (variant7.tag) {
    case 'ok': {
      const e = variant7.val;
      dataView(memory0).setInt8(arg4 + 0, 0, true);
      var {lower: v5_0, upper: v5_1 } = e;
      dataView(memory0).setBigInt64(arg4 + 8, toUint64(v5_0), true);
      dataView(memory0).setBigInt64(arg4 + 16, toUint64(v5_1), true);
      break;
    }
    case 'err': {
      const e = variant7.val;
      dataView(memory0).setInt8(arg4 + 0, 1, true);
      var val6 = e;
      let enum6;
      switch (val6) {
        case 'access': {
          enum6 = 0;
          break;
        }
        case 'would-block': {
          enum6 = 1;
          break;
        }
        case 'already': {
          enum6 = 2;
          break;
        }
        case 'bad-descriptor': {
          enum6 = 3;
          break;
        }
        case 'busy': {
          enum6 = 4;
          break;
        }
        case 'deadlock': {
          enum6 = 5;
          break;
        }
        case 'quota': {
          enum6 = 6;
          break;
        }
        case 'exist': {
          enum6 = 7;
          break;
        }
        case 'file-too-large': {
          enum6 = 8;
          break;
        }
        case 'illegal-byte-sequence': {
          enum6 = 9;
          break;
        }
        case 'in-progress': {
          enum6 = 10;
          break;
        }
        case 'interrupted': {
          enum6 = 11;
          break;
        }
        case 'invalid': {
          enum6 = 12;
          break;
        }
        case 'io': {
          enum6 = 13;
          break;
        }
        case 'is-directory': {
          enum6 = 14;
          break;
        }
        case 'loop': {
          enum6 = 15;
          break;
        }
        case 'too-many-links': {
          enum6 = 16;
          break;
        }
        case 'message-size': {
          enum6 = 17;
          break;
        }
        case 'name-too-long': {
          enum6 = 18;
          break;
        }
        case 'no-device': {
          enum6 = 19;
          break;
        }
        case 'no-entry': {
          enum6 = 20;
          break;
        }
        case 'no-lock': {
          enum6 = 21;
          break;
        }
        case 'insufficient-memory': {
          enum6 = 22;
          break;
        }
        case 'insufficient-space': {
          enum6 = 23;
          break;
        }
        case 'not-directory': {
          enum6 = 24;
          break;
        }
        case 'not-empty': {
          enum6 = 25;
          break;
        }
        case 'not-recoverable': {
          enum6 = 26;
          break;
        }
        case 'unsupported': {
          enum6 = 27;
          break;
        }
        case 'no-tty': {
          enum6 = 28;
          break;
        }
        case 'no-such-device': {
          enum6 = 29;
          break;
        }
        case 'overflow': {
          enum6 = 30;
          break;
        }
        case 'not-permitted': {
          enum6 = 31;
          break;
        }
        case 'pipe': {
          enum6 = 32;
          break;
        }
        case 'read-only': {
          enum6 = 33;
          break;
        }
        case 'invalid-seek': {
          enum6 = 34;
          break;
        }
        case 'text-file-busy': {
          enum6 = 35;
          break;
        }
        case 'cross-device': {
          enum6 = 36;
          break;
        }
        default: {
          if ((e) instanceof Error) {
            console.error(e);
          }
          
          throw new TypeError(`"${val6}" is not one of the cases of error-code`);
        }
      }
      dataView(memory0).setInt8(arg4 + 8, enum6, true);
      break;
    }
    default: {
      throw new TypeError('invalid variant specified for result');
    }
  }
  _debugLog('[iface="wasi:filesystem/types@0.2.6", function="[method]descriptor.metadata-hash-at"][Instruction::Return]', {
    funcName: '[method]descriptor.metadata-hash-at',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}


let lowered_import_22_metadata = {
  qualifiedImportFn: 'wasi:filesystem/types@0.2.6#[method]directory-entry-stream.read-directory-entry',
  moduleIdx: null,
};


function trampoline30(arg0, arg1) {
  var handle1 = arg0;
  var rep2 = handleTable7[(handle1 << 1) + 1] & ~T_FLAG;
  var rsc0 = captureTable7.get(rep2);
  if (!rsc0) {
    rsc0 = Object.create(DirectoryEntryStream.prototype);
    Object.defineProperty(rsc0, symbolRscHandle, { writable: true, value: handle1});
    Object.defineProperty(rsc0, symbolRscRep, { writable: true, value: rep2});
  }
  curResourceBorrows.push(rsc0);
  _debugLog('[iface="wasi:filesystem/types@0.2.6", function="[method]directory-entry-stream.read-directory-entry"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = rsc0.readDirectoryEntry?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'readDirectoryEntry',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'result-catch-handler',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  
  let ret;
  try {
    ret = { tag: 'ok', val:  rsc0.readDirectoryEntry()};
  } catch (e) {
    ret = { tag: 'err', val: getErrorPayload(e) };
  }
  
  for (const rsc of curResourceBorrows) {
    rsc[symbolRscHandle] = undefined;
  }
  curResourceBorrows = [];
  endCurrentTask(0);
  var variant8 = ret;
  switch (variant8.tag) {
    case 'ok': {
      const e = variant8.val;
      dataView(memory0).setInt8(arg1 + 0, 0, true);
      var variant6 = e;
      if (variant6 === null || variant6=== undefined) {
        dataView(memory0).setInt8(arg1 + 4, 0, true);
      } else {
        const e = variant6;
        dataView(memory0).setInt8(arg1 + 4, 1, true);
        var {type: v3_0, name: v3_1 } = e;
        var val4 = v3_0;
        let enum4;
        switch (val4) {
          case 'unknown': {
            enum4 = 0;
            break;
          }
          case 'block-device': {
            enum4 = 1;
            break;
          }
          case 'character-device': {
            enum4 = 2;
            break;
          }
          case 'directory': {
            enum4 = 3;
            break;
          }
          case 'fifo': {
            enum4 = 4;
            break;
          }
          case 'symbolic-link': {
            enum4 = 5;
            break;
          }
          case 'regular-file': {
            enum4 = 6;
            break;
          }
          case 'socket': {
            enum4 = 7;
            break;
          }
          default: {
            if ((v3_0) instanceof Error) {
              console.error(v3_0);
            }
            
            throw new TypeError(`"${val4}" is not one of the cases of descriptor-type`);
          }
        }
        dataView(memory0).setInt8(arg1 + 8, enum4, true);
        
        var encodeRes = _utf8AllocateAndEncode(v3_1, realloc0, memory0);
        var ptr5= encodeRes.ptr;
        var len5 = encodeRes.len;
        
        dataView(memory0).setUint32(arg1 + 16, len5, true);
        dataView(memory0).setUint32(arg1 + 12, ptr5, true);
      }
      break;
    }
    case 'err': {
      const e = variant8.val;
      dataView(memory0).setInt8(arg1 + 0, 1, true);
      var val7 = e;
      let enum7;
      switch (val7) {
        case 'access': {
          enum7 = 0;
          break;
        }
        case 'would-block': {
          enum7 = 1;
          break;
        }
        case 'already': {
          enum7 = 2;
          break;
        }
        case 'bad-descriptor': {
          enum7 = 3;
          break;
        }
        case 'busy': {
          enum7 = 4;
          break;
        }
        case 'deadlock': {
          enum7 = 5;
          break;
        }
        case 'quota': {
          enum7 = 6;
          break;
        }
        case 'exist': {
          enum7 = 7;
          break;
        }
        case 'file-too-large': {
          enum7 = 8;
          break;
        }
        case 'illegal-byte-sequence': {
          enum7 = 9;
          break;
        }
        case 'in-progress': {
          enum7 = 10;
          break;
        }
        case 'interrupted': {
          enum7 = 11;
          break;
        }
        case 'invalid': {
          enum7 = 12;
          break;
        }
        case 'io': {
          enum7 = 13;
          break;
        }
        case 'is-directory': {
          enum7 = 14;
          break;
        }
        case 'loop': {
          enum7 = 15;
          break;
        }
        case 'too-many-links': {
          enum7 = 16;
          break;
        }
        case 'message-size': {
          enum7 = 17;
          break;
        }
        case 'name-too-long': {
          enum7 = 18;
          break;
        }
        case 'no-device': {
          enum7 = 19;
          break;
        }
        case 'no-entry': {
          enum7 = 20;
          break;
        }
        case 'no-lock': {
          enum7 = 21;
          break;
        }
        case 'insufficient-memory': {
          enum7 = 22;
          break;
        }
        case 'insufficient-space': {
          enum7 = 23;
          break;
        }
        case 'not-directory': {
          enum7 = 24;
          break;
        }
        case 'not-empty': {
          enum7 = 25;
          break;
        }
        case 'not-recoverable': {
          enum7 = 26;
          break;
        }
        case 'unsupported': {
          enum7 = 27;
          break;
        }
        case 'no-tty': {
          enum7 = 28;
          break;
        }
        case 'no-such-device': {
          enum7 = 29;
          break;
        }
        case 'overflow': {
          enum7 = 30;
          break;
        }
        case 'not-permitted': {
          enum7 = 31;
          break;
        }
        case 'pipe': {
          enum7 = 32;
          break;
        }
        case 'read-only': {
          enum7 = 33;
          break;
        }
        case 'invalid-seek': {
          enum7 = 34;
          break;
        }
        case 'text-file-busy': {
          enum7 = 35;
          break;
        }
        case 'cross-device': {
          enum7 = 36;
          break;
        }
        default: {
          if ((e) instanceof Error) {
            console.error(e);
          }
          
          throw new TypeError(`"${val7}" is not one of the cases of error-code`);
        }
      }
      dataView(memory0).setInt8(arg1 + 4, enum7, true);
      break;
    }
    default: {
      throw new TypeError('invalid variant specified for result');
    }
  }
  _debugLog('[iface="wasi:filesystem/types@0.2.6", function="[method]directory-entry-stream.read-directory-entry"][Instruction::Return]', {
    funcName: '[method]directory-entry-stream.read-directory-entry',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}


let lowered_import_23_metadata = {
  qualifiedImportFn: 'wasi:cli/environment@0.2.6#get-environment',
  moduleIdx: null,
};


function trampoline31(arg0) {
  _debugLog('[iface="wasi:cli/environment@0.2.6", function="get-environment"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = getEnvironment?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'getEnvironment',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'none',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  let ret =  getEnvironment();
  endCurrentTask(0);
  var vec3 = ret;
  var len3 = vec3.length;
  var result3 = realloc0(0, 0, 4, len3 * 16);
  for (let i = 0; i < vec3.length; i++) {
    const e = vec3[i];
    const base = result3 + i * 16;var [tuple0_0, tuple0_1] = e;
    
    var encodeRes = _utf8AllocateAndEncode(tuple0_0, realloc0, memory0);
    var ptr1= encodeRes.ptr;
    var len1 = encodeRes.len;
    
    dataView(memory0).setUint32(base + 4, len1, true);
    dataView(memory0).setUint32(base + 0, ptr1, true);
    
    var encodeRes = _utf8AllocateAndEncode(tuple0_1, realloc0, memory0);
    var ptr2= encodeRes.ptr;
    var len2 = encodeRes.len;
    
    dataView(memory0).setUint32(base + 12, len2, true);
    dataView(memory0).setUint32(base + 8, ptr2, true);
  }
  dataView(memory0).setUint32(arg0 + 4, len3, true);
  dataView(memory0).setUint32(arg0 + 0, result3, true);
  _debugLog('[iface="wasi:cli/environment@0.2.6", function="get-environment"][Instruction::Return]', {
    funcName: 'get-environment',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}


let lowered_import_24_metadata = {
  qualifiedImportFn: 'wasi:cli/terminal-stdin@0.2.6#get-terminal-stdin',
  moduleIdx: null,
};

const handleTable4 = [T_FLAG, 0];
const captureTable4= new Map();
let captureCnt4 = 0;
handleTables[4] = handleTable4;

function trampoline32(arg0) {
  _debugLog('[iface="wasi:cli/terminal-stdin@0.2.6", function="get-terminal-stdin"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = getTerminalStdin?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'getTerminalStdin',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'none',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  let ret =  getTerminalStdin();
  endCurrentTask(0);
  var variant1 = ret;
  if (variant1 === null || variant1=== undefined) {
    dataView(memory0).setInt8(arg0 + 0, 0, true);
  } else {
    const e = variant1;
    dataView(memory0).setInt8(arg0 + 0, 1, true);
    if (!(e instanceof TerminalInput)) {
      throw new TypeError('Resource error: Not a valid "TerminalInput" resource.');
    }
    var handle0 = e[symbolRscHandle];
    if (!handle0) {
      const rep = e[symbolRscRep] || ++captureCnt4;
      captureTable4.set(rep, e);
      handle0 = rscTableCreateOwn(handleTable4, rep);
    }
    dataView(memory0).setInt32(arg0 + 4, handle0, true);
  }
  _debugLog('[iface="wasi:cli/terminal-stdin@0.2.6", function="get-terminal-stdin"][Instruction::Return]', {
    funcName: 'get-terminal-stdin',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}


let lowered_import_25_metadata = {
  qualifiedImportFn: 'wasi:cli/terminal-stdout@0.2.6#get-terminal-stdout',
  moduleIdx: null,
};

const handleTable5 = [T_FLAG, 0];
const captureTable5= new Map();
let captureCnt5 = 0;
handleTables[5] = handleTable5;

function trampoline33(arg0) {
  _debugLog('[iface="wasi:cli/terminal-stdout@0.2.6", function="get-terminal-stdout"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = getTerminalStdout?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'getTerminalStdout',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'none',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  let ret =  getTerminalStdout();
  endCurrentTask(0);
  var variant1 = ret;
  if (variant1 === null || variant1=== undefined) {
    dataView(memory0).setInt8(arg0 + 0, 0, true);
  } else {
    const e = variant1;
    dataView(memory0).setInt8(arg0 + 0, 1, true);
    if (!(e instanceof TerminalOutput)) {
      throw new TypeError('Resource error: Not a valid "TerminalOutput" resource.');
    }
    var handle0 = e[symbolRscHandle];
    if (!handle0) {
      const rep = e[symbolRscRep] || ++captureCnt5;
      captureTable5.set(rep, e);
      handle0 = rscTableCreateOwn(handleTable5, rep);
    }
    dataView(memory0).setInt32(arg0 + 4, handle0, true);
  }
  _debugLog('[iface="wasi:cli/terminal-stdout@0.2.6", function="get-terminal-stdout"][Instruction::Return]', {
    funcName: 'get-terminal-stdout',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}


let lowered_import_26_metadata = {
  qualifiedImportFn: 'wasi:cli/terminal-stderr@0.2.6#get-terminal-stderr',
  moduleIdx: null,
};


function trampoline34(arg0) {
  _debugLog('[iface="wasi:cli/terminal-stderr@0.2.6", function="get-terminal-stderr"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = getTerminalStderr?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'getTerminalStderr',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'none',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  let ret =  getTerminalStderr();
  endCurrentTask(0);
  var variant1 = ret;
  if (variant1 === null || variant1=== undefined) {
    dataView(memory0).setInt8(arg0 + 0, 0, true);
  } else {
    const e = variant1;
    dataView(memory0).setInt8(arg0 + 0, 1, true);
    if (!(e instanceof TerminalOutput)) {
      throw new TypeError('Resource error: Not a valid "TerminalOutput" resource.');
    }
    var handle0 = e[symbolRscHandle];
    if (!handle0) {
      const rep = e[symbolRscRep] || ++captureCnt5;
      captureTable5.set(rep, e);
      handle0 = rscTableCreateOwn(handleTable5, rep);
    }
    dataView(memory0).setInt32(arg0 + 4, handle0, true);
  }
  _debugLog('[iface="wasi:cli/terminal-stderr@0.2.6", function="get-terminal-stderr"][Instruction::Return]', {
    funcName: 'get-terminal-stderr',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}


let lowered_import_27_metadata = {
  qualifiedImportFn: 'wasi:clocks/wall-clock@0.2.6#now',
  moduleIdx: null,
};


function trampoline35(arg0) {
  _debugLog('[iface="wasi:clocks/wall-clock@0.2.6", function="now"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = now$1?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'now$1',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'none',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  let ret =  now$1();
  endCurrentTask(0);
  var {seconds: v0_0, nanoseconds: v0_1 } = ret;
  dataView(memory0).setBigInt64(arg0 + 0, toUint64(v0_0), true);
  dataView(memory0).setInt32(arg0 + 8, toUint32(v0_1), true);
  _debugLog('[iface="wasi:clocks/wall-clock@0.2.6", function="now"][Instruction::Return]', {
    funcName: 'now',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}


let lowered_import_28_metadata = {
  qualifiedImportFn: 'wasi:filesystem/preopens@0.2.6#get-directories',
  moduleIdx: null,
};


function trampoline36(arg0) {
  _debugLog('[iface="wasi:filesystem/preopens@0.2.6", function="get-directories"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = false;
  hostProvided = getDirectories?._isHostProvided;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: 0,
      isAsync: false,
      entryFnName: 'getDirectories',
      getCallbackFn: () => null,
      callbackFnName: 'null',
      errHandling: 'none',
      callingWasmExport: false,
    });
    task = results[0];
  };
  
  taskCreation: {
    parentTask = getCurrentTask(0)?.task;
    if (!parentTask) {
      createTask();
      break taskCreation;
    }
    
    createTask();
    
    const isHostAsyncImport = hostProvided && false;
    if (isHostAsyncImport) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error("Missing subtask for host import, has the import been lowered? (ensure asyncImports are set properly)");
      }
      subtask.setChildTask(task);
      task.setParentSubtask(subtask);
    }
  }
  
  let ret =  getDirectories();
  endCurrentTask(0);
  var vec3 = ret;
  var len3 = vec3.length;
  var result3 = realloc0(0, 0, 4, len3 * 12);
  for (let i = 0; i < vec3.length; i++) {
    const e = vec3[i];
    const base = result3 + i * 12;var [tuple0_0, tuple0_1] = e;
    if (!(tuple0_0 instanceof Descriptor)) {
      throw new TypeError('Resource error: Not a valid "Descriptor" resource.');
    }
    var handle1 = tuple0_0[symbolRscHandle];
    if (!handle1) {
      const rep = tuple0_0[symbolRscRep] || ++captureCnt6;
      captureTable6.set(rep, tuple0_0);
      handle1 = rscTableCreateOwn(handleTable6, rep);
    }
    dataView(memory0).setInt32(base + 0, handle1, true);
    
    var encodeRes = _utf8AllocateAndEncode(tuple0_1, realloc0, memory0);
    var ptr2= encodeRes.ptr;
    var len2 = encodeRes.len;
    
    dataView(memory0).setUint32(base + 8, len2, true);
    dataView(memory0).setUint32(base + 4, ptr2, true);
  }
  dataView(memory0).setUint32(arg0 + 4, len3, true);
  dataView(memory0).setUint32(arg0 + 0, result3, true);
  _debugLog('[iface="wasi:filesystem/preopens@0.2.6", function="get-directories"][Instruction::Return]', {
    funcName: 'get-directories',
    paramCount: 0,
    async: false,
    postReturn: false
  });
}

let exports2;
let postReturn0;
let postReturn1;
let postReturn2;
let postReturn3;
function trampoline0(handle) {
  const handleEntry = rscTableRemove(handleTable1, handle);
  if (handleEntry.own) {
    
    const rsc = captureTable1.get(handleEntry.rep);
    if (rsc) {
      if (rsc[symbolDispose]) rsc[symbolDispose]();
      captureTable1.delete(handleEntry.rep);
    } else if (Error$1[symbolCabiDispose]) {
      Error$1[symbolCabiDispose](handleEntry.rep);
    }
  }
}
function trampoline1(handle) {
  const handleEntry = rscTableRemove(handleTable0, handle);
  if (handleEntry.own) {
    
    const rsc = captureTable0.get(handleEntry.rep);
    if (rsc) {
      if (rsc[symbolDispose]) rsc[symbolDispose]();
      captureTable0.delete(handleEntry.rep);
    } else if (Pollable[symbolCabiDispose]) {
      Pollable[symbolCabiDispose](handleEntry.rep);
    }
  }
}
function trampoline2(handle) {
  const handleEntry = rscTableRemove(handleTable2, handle);
  if (handleEntry.own) {
    
    const rsc = captureTable2.get(handleEntry.rep);
    if (rsc) {
      if (rsc[symbolDispose]) rsc[symbolDispose]();
      captureTable2.delete(handleEntry.rep);
    } else if (InputStream[symbolCabiDispose]) {
      InputStream[symbolCabiDispose](handleEntry.rep);
    }
  }
}
function trampoline3(handle) {
  const handleEntry = rscTableRemove(handleTable3, handle);
  if (handleEntry.own) {
    
    const rsc = captureTable3.get(handleEntry.rep);
    if (rsc) {
      if (rsc[symbolDispose]) rsc[symbolDispose]();
      captureTable3.delete(handleEntry.rep);
    } else if (OutputStream[symbolCabiDispose]) {
      OutputStream[symbolCabiDispose](handleEntry.rep);
    }
  }
}
function trampoline4(handle) {
  const handleEntry = rscTableRemove(handleTable4, handle);
  if (handleEntry.own) {
    
    const rsc = captureTable4.get(handleEntry.rep);
    if (rsc) {
      if (rsc[symbolDispose]) rsc[symbolDispose]();
      captureTable4.delete(handleEntry.rep);
    } else if (TerminalInput[symbolCabiDispose]) {
      TerminalInput[symbolCabiDispose](handleEntry.rep);
    }
  }
}
function trampoline5(handle) {
  const handleEntry = rscTableRemove(handleTable5, handle);
  if (handleEntry.own) {
    
    const rsc = captureTable5.get(handleEntry.rep);
    if (rsc) {
      if (rsc[symbolDispose]) rsc[symbolDispose]();
      captureTable5.delete(handleEntry.rep);
    } else if (TerminalOutput[symbolCabiDispose]) {
      TerminalOutput[symbolCabiDispose](handleEntry.rep);
    }
  }
}
function trampoline6(handle) {
  const handleEntry = rscTableRemove(handleTable6, handle);
  if (handleEntry.own) {
    
    const rsc = captureTable6.get(handleEntry.rep);
    if (rsc) {
      if (rsc[symbolDispose]) rsc[symbolDispose]();
      captureTable6.delete(handleEntry.rep);
    } else if (Descriptor[symbolCabiDispose]) {
      Descriptor[symbolCabiDispose](handleEntry.rep);
    }
  }
}
function trampoline7(handle) {
  const handleEntry = rscTableRemove(handleTable7, handle);
  if (handleEntry.own) {
    
    const rsc = captureTable7.get(handleEntry.rep);
    if (rsc) {
      if (rsc[symbolDispose]) rsc[symbolDispose]();
      captureTable7.delete(handleEntry.rep);
    } else if (DirectoryEntryStream[symbolCabiDispose]) {
      DirectoryEntryStream[symbolCabiDispose](handleEntry.rep);
    }
  }
}

GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_0_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_0_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 8,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [_liftFlatResult([['ok', null, null],['error', null, null],])],
    metadata: lowered_import_0_metadata,
    resultLowerFns: [],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: null,
    getMemoryFn: () => null,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_1_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_1_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 9,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [_liftFlatBorrow.bind(null, 0)],
    metadata: lowered_import_1_metadata,
    resultLowerFns: [],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: null,
    getMemoryFn: () => null,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_2_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_2_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 10,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [_liftFlatBorrow.bind(null, 2)],
    metadata: lowered_import_2_metadata,
    resultLowerFns: [_lowerFlatOwn.bind(null, 0)],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: null,
    getMemoryFn: () => null,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_3_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_3_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 11,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [_liftFlatBorrow.bind(null, 3)],
    metadata: lowered_import_3_metadata,
    resultLowerFns: [_lowerFlatOwn.bind(null, 0)],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: null,
    getMemoryFn: () => null,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_4_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_4_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 12,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [],
    metadata: lowered_import_4_metadata,
    resultLowerFns: [_lowerFlatOwn.bind(null, 2)],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: null,
    getMemoryFn: () => null,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_5_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_5_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 13,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [],
    metadata: lowered_import_5_metadata,
    resultLowerFns: [_lowerFlatOwn.bind(null, 3)],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: null,
    getMemoryFn: () => null,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_6_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_6_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 14,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [],
    metadata: lowered_import_6_metadata,
    resultLowerFns: [_lowerFlatOwn.bind(null, 3)],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: null,
    getMemoryFn: () => null,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_7_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_7_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 15,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [],
    metadata: lowered_import_7_metadata,
    resultLowerFns: [_lowerFlatU64],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: null,
    getMemoryFn: () => null,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_8_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_8_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 16,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [],
    metadata: lowered_import_8_metadata,
    resultLowerFns: [_lowerFlatTuple.bind(null, 34)],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: 0,
    getMemoryFn: () => memory0,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_9_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_9_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 17,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [_liftFlatBorrow.bind(null, 2),_liftFlatU64],
    metadata: lowered_import_9_metadata,
    resultLowerFns: [_lowerFlatResult([{ discriminant: 0, tag: 'ok', lowerFn: _lowerFlatList({ elemLowerFn: _lowerFlatU8, typeIdx: 4 }), align32: 4 },{ discriminant: 1, tag: 'error', lowerFn: _lowerFlatVariant({ discriminantSizeBytes: 1, lowerMetas: [{ discriminant: 0, tag: 'last-operation-failed', lowerFn: _lowerFlatOwn.bind(null, 1), align32: 4, },{ discriminant: 1, tag: 'closed', lowerFn: null, align32: null, },] }), align32: 4 },])],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: 0,
    getMemoryFn: () => memory0,
    getReallocFn: () => realloc0,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_10_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_10_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 18,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [_liftFlatBorrow.bind(null, 3)],
    metadata: lowered_import_10_metadata,
    resultLowerFns: [_lowerFlatResult([{ discriminant: 0, tag: 'ok', lowerFn: _lowerFlatU64, align32: 8 },{ discriminant: 1, tag: 'error', lowerFn: _lowerFlatVariant({ discriminantSizeBytes: 1, lowerMetas: [{ discriminant: 0, tag: 'last-operation-failed', lowerFn: _lowerFlatOwn.bind(null, 1), align32: 4, },{ discriminant: 1, tag: 'closed', lowerFn: null, align32: null, },] }), align32: 4 },])],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: 0,
    getMemoryFn: () => memory0,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_11_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_11_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 19,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [_liftFlatBorrow.bind(null, 3),_liftFlatList.bind(null, 4)],
    metadata: lowered_import_11_metadata,
    resultLowerFns: [_lowerFlatResult([{ discriminant: 0, tag: 'ok', lowerFn: null, align32: null },{ discriminant: 1, tag: 'error', lowerFn: _lowerFlatVariant({ discriminantSizeBytes: 1, lowerMetas: [{ discriminant: 0, tag: 'last-operation-failed', lowerFn: _lowerFlatOwn.bind(null, 1), align32: 4, },{ discriminant: 1, tag: 'closed', lowerFn: null, align32: null, },] }), align32: 4 },])],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: 0,
    getMemoryFn: () => memory0,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_12_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_12_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 20,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [_liftFlatBorrow.bind(null, 3)],
    metadata: lowered_import_12_metadata,
    resultLowerFns: [_lowerFlatResult([{ discriminant: 0, tag: 'ok', lowerFn: null, align32: null },{ discriminant: 1, tag: 'error', lowerFn: _lowerFlatVariant({ discriminantSizeBytes: 1, lowerMetas: [{ discriminant: 0, tag: 'last-operation-failed', lowerFn: _lowerFlatOwn.bind(null, 1), align32: 4, },{ discriminant: 1, tag: 'closed', lowerFn: null, align32: null, },] }), align32: 4 },])],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: 0,
    getMemoryFn: () => memory0,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_13_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_13_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 21,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [_liftFlatBorrow.bind(null, 6),_liftFlatU64],
    metadata: lowered_import_13_metadata,
    resultLowerFns: [_lowerFlatResult([{ discriminant: 0, tag: 'ok', lowerFn: _lowerFlatOwn.bind(null, 2), align32: 4 },{ discriminant: 1, tag: 'error', lowerFn: _lowerFlatEnum.bind(null, 0), align32: 1 },])],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: 0,
    getMemoryFn: () => memory0,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_14_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_14_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 22,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [_liftFlatBorrow.bind(null, 6),_liftFlatU64],
    metadata: lowered_import_14_metadata,
    resultLowerFns: [_lowerFlatResult([{ discriminant: 0, tag: 'ok', lowerFn: _lowerFlatOwn.bind(null, 3), align32: 4 },{ discriminant: 1, tag: 'error', lowerFn: _lowerFlatEnum.bind(null, 0), align32: 1 },])],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: 0,
    getMemoryFn: () => memory0,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_15_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_15_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 23,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [_liftFlatBorrow.bind(null, 6)],
    metadata: lowered_import_15_metadata,
    resultLowerFns: [_lowerFlatResult([{ discriminant: 0, tag: 'ok', lowerFn: _lowerFlatOwn.bind(null, 3), align32: 4 },{ discriminant: 1, tag: 'error', lowerFn: _lowerFlatEnum.bind(null, 0), align32: 1 },])],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: 0,
    getMemoryFn: () => memory0,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_16_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_16_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 24,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [_liftFlatBorrow.bind(null, 6)],
    metadata: lowered_import_16_metadata,
    resultLowerFns: [_lowerFlatResult([{ discriminant: 0, tag: 'ok', lowerFn: _lowerFlatFlags.bind(null, 0), align32: 1 },{ discriminant: 1, tag: 'error', lowerFn: _lowerFlatEnum.bind(null, 0), align32: 1 },])],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: 0,
    getMemoryFn: () => memory0,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_17_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_17_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 25,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [_liftFlatBorrow.bind(null, 6)],
    metadata: lowered_import_17_metadata,
    resultLowerFns: [_lowerFlatResult([{ discriminant: 0, tag: 'ok', lowerFn: _lowerFlatOwn.bind(null, 7), align32: 4 },{ discriminant: 1, tag: 'error', lowerFn: _lowerFlatEnum.bind(null, 0), align32: 1 },])],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: 0,
    getMemoryFn: () => memory0,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_18_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_18_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 26,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [_liftFlatBorrow.bind(null, 6)],
    metadata: lowered_import_18_metadata,
    resultLowerFns: [_lowerFlatResult([{ discriminant: 0, tag: 'ok', lowerFn: _lowerFlatRecord([{ field: 'type', lowerFn: _lowerFlatEnum.bind(null, 1), align32: 8 },{ field: 'link-count', lowerFn: _lowerFlatU64, align32: 8 },{ field: 'size', lowerFn: _lowerFlatU64, align32: 8 },{ field: 'data-access-timestamp', lowerFn: _lowerFlatOption.bind(null, 3), align32: 8 },{ field: 'data-modification-timestamp', lowerFn: _lowerFlatOption.bind(null, 3), align32: 8 },{ field: 'status-change-timestamp', lowerFn: _lowerFlatOption.bind(null, 3), align32: 8 },]), align32: 8 },{ discriminant: 1, tag: 'error', lowerFn: _lowerFlatEnum.bind(null, 0), align32: 1 },])],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: 0,
    getMemoryFn: () => memory0,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_19_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_19_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 27,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [_liftFlatBorrow.bind(null, 6),_liftFlatFlags.bind(null, 1),_liftFlatStringUTF8,_liftFlatFlags.bind(null, 2),_liftFlatFlags.bind(null, 0)],
    metadata: lowered_import_19_metadata,
    resultLowerFns: [_lowerFlatResult([{ discriminant: 0, tag: 'ok', lowerFn: _lowerFlatOwn.bind(null, 6), align32: 4 },{ discriminant: 1, tag: 'error', lowerFn: _lowerFlatEnum.bind(null, 0), align32: 1 },])],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: 0,
    getMemoryFn: () => memory0,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_20_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_20_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 28,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [_liftFlatBorrow.bind(null, 6)],
    metadata: lowered_import_20_metadata,
    resultLowerFns: [_lowerFlatResult([{ discriminant: 0, tag: 'ok', lowerFn: _lowerFlatRecord([{ field: 'lower', lowerFn: _lowerFlatU64, align32: 8 },{ field: 'upper', lowerFn: _lowerFlatU64, align32: 8 },]), align32: 8 },{ discriminant: 1, tag: 'error', lowerFn: _lowerFlatEnum.bind(null, 0), align32: 1 },])],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: 0,
    getMemoryFn: () => memory0,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_21_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_21_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 29,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [_liftFlatBorrow.bind(null, 6),_liftFlatFlags.bind(null, 1),_liftFlatStringUTF8],
    metadata: lowered_import_21_metadata,
    resultLowerFns: [_lowerFlatResult([{ discriminant: 0, tag: 'ok', lowerFn: _lowerFlatRecord([{ field: 'lower', lowerFn: _lowerFlatU64, align32: 8 },{ field: 'upper', lowerFn: _lowerFlatU64, align32: 8 },]), align32: 8 },{ discriminant: 1, tag: 'error', lowerFn: _lowerFlatEnum.bind(null, 0), align32: 1 },])],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: 0,
    getMemoryFn: () => memory0,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_22_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_22_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 30,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [_liftFlatBorrow.bind(null, 7)],
    metadata: lowered_import_22_metadata,
    resultLowerFns: [_lowerFlatResult([{ discriminant: 0, tag: 'ok', lowerFn: _lowerFlatOption.bind(null, 4), align32: 4 },{ discriminant: 1, tag: 'error', lowerFn: _lowerFlatEnum.bind(null, 0), align32: 1 },])],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: 0,
    getMemoryFn: () => memory0,
    getReallocFn: () => realloc0,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_23_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_23_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 31,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [],
    metadata: lowered_import_23_metadata,
    resultLowerFns: [_lowerFlatList({ elemLowerFn: _lowerFlatTuple.bind(null, 11), typeIdx: 5 })],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: 0,
    getMemoryFn: () => memory0,
    getReallocFn: () => realloc0,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_24_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_24_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 32,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [],
    metadata: lowered_import_24_metadata,
    resultLowerFns: [_lowerFlatOption.bind(null, 1)],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: 0,
    getMemoryFn: () => memory0,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_25_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_25_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 33,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [],
    metadata: lowered_import_25_metadata,
    resultLowerFns: [_lowerFlatOption.bind(null, 2)],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: 0,
    getMemoryFn: () => memory0,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_26_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_26_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 34,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [],
    metadata: lowered_import_26_metadata,
    resultLowerFns: [_lowerFlatOption.bind(null, 2)],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: 0,
    getMemoryFn: () => memory0,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_27_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_27_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 35,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [],
    metadata: lowered_import_27_metadata,
    resultLowerFns: [_lowerFlatRecord([{ field: 'seconds', lowerFn: _lowerFlatU64, align32: 8 },{ field: 'nanoseconds', lowerFn: _lowerFlatU32, align32: 8 },])],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: 0,
    getMemoryFn: () => memory0,
    getReallocFn: () => null,
  },
  ),
});


GlobalComponentAsyncLowers.define({
  componentIdx: lowered_import_28_metadata.moduleIdx,
  qualifiedImportFn: lowered_import_28_metadata.qualifiedImportFn,
  fn: _lowerImport.bind(
  null,
  {
    trampolineIdx: 36,
    componentIdx: 0,
    isAsync: false,
    paramLiftFns: [],
    metadata: lowered_import_28_metadata,
    resultLowerFns: [_lowerFlatList({ elemLowerFn: _lowerFlatTuple.bind(null, 32), typeIdx: 6 })],
    getCallbackFn: () => null,
    getPostReturnFn: () => null,
    isCancellable: false,
    memoryIdx: 0,
    getMemoryFn: () => memory0,
    getReallocFn: () => realloc0,
  },
  ),
});

let adapter010Id;

function id() {
  _debugLog('[iface="pulseengine:rivet/adapter@0.1.0", function="id"][Instruction::CallWasm] enter', {
    funcName: 'id',
    paramCount: 0,
    async: false,
    postReturn: true,
  });
  const hostProvided = false;
  
  const [task, _wasm_call_currentTaskID] = createNewCurrentTask({
    componentIdx: 0,
    isAsync: false,
    entryFnName: 'adapter010Id',
    getCallbackFn: () => null,
    callbackFnName: 'null',
    errHandling: 'none',
    callingWasmExport: true,
  });
  
  let ret = adapter010Id();
  endCurrentTask(0);
  var ptr0 = dataView(memory0).getUint32(ret + 0, true);
  var len0 = dataView(memory0).getUint32(ret + 4, true);
  var result0 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr0, len0));
  _debugLog('[iface="pulseengine:rivet/adapter@0.1.0", function="id"][Instruction::Return]', {
    funcName: 'id',
    paramCount: 1,
    async: false,
    postReturn: true
  });
  const retCopy = result0;
  
  let cstate = getOrCreateAsyncState(0);
  cstate.mayLeave = false;
  postReturn0(ret);
  cstate.mayLeave = true;
  return retCopy;
  
}
let adapter010Name;

function name() {
  _debugLog('[iface="pulseengine:rivet/adapter@0.1.0", function="name"][Instruction::CallWasm] enter', {
    funcName: 'name',
    paramCount: 0,
    async: false,
    postReturn: true,
  });
  const hostProvided = false;
  
  const [task, _wasm_call_currentTaskID] = createNewCurrentTask({
    componentIdx: 0,
    isAsync: false,
    entryFnName: 'adapter010Name',
    getCallbackFn: () => null,
    callbackFnName: 'null',
    errHandling: 'none',
    callingWasmExport: true,
  });
  
  let ret = adapter010Name();
  endCurrentTask(0);
  var ptr0 = dataView(memory0).getUint32(ret + 0, true);
  var len0 = dataView(memory0).getUint32(ret + 4, true);
  var result0 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr0, len0));
  _debugLog('[iface="pulseengine:rivet/adapter@0.1.0", function="name"][Instruction::Return]', {
    funcName: 'name',
    paramCount: 1,
    async: false,
    postReturn: true
  });
  const retCopy = result0;
  
  let cstate = getOrCreateAsyncState(0);
  cstate.mayLeave = false;
  postReturn0(ret);
  cstate.mayLeave = true;
  return retCopy;
  
}
let adapter010SupportedTypes;

function supportedTypes() {
  _debugLog('[iface="pulseengine:rivet/adapter@0.1.0", function="supported-types"][Instruction::CallWasm] enter', {
    funcName: 'supported-types',
    paramCount: 0,
    async: false,
    postReturn: true,
  });
  const hostProvided = false;
  
  const [task, _wasm_call_currentTaskID] = createNewCurrentTask({
    componentIdx: 0,
    isAsync: false,
    entryFnName: 'adapter010SupportedTypes',
    getCallbackFn: () => null,
    callbackFnName: 'null',
    errHandling: 'none',
    callingWasmExport: true,
  });
  
  let ret = adapter010SupportedTypes();
  endCurrentTask(0);
  var len1 = dataView(memory0).getUint32(ret + 4, true);
  var base1 = dataView(memory0).getUint32(ret + 0, true);
  var result1 = [];
  for (let i = 0; i < len1; i++) {
    const base = base1 + i * 8;
    var ptr0 = dataView(memory0).getUint32(base + 0, true);
    var len0 = dataView(memory0).getUint32(base + 4, true);
    var result0 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr0, len0));
    result1.push(result0);
  }
  _debugLog('[iface="pulseengine:rivet/adapter@0.1.0", function="supported-types"][Instruction::Return]', {
    funcName: 'supported-types',
    paramCount: 1,
    async: false,
    postReturn: true
  });
  const retCopy = result1;
  
  let cstate = getOrCreateAsyncState(0);
  cstate.mayLeave = false;
  postReturn1(ret);
  cstate.mayLeave = true;
  return retCopy;
  
}
let adapter010Import;

function _import(arg0, arg1) {
  var val0 = arg0;
  var len0 = val0.byteLength;
  var ptr0 = realloc0(0, 0, 1, len0 * 1);
  
  let valData0;
  const valLenBytes0 = len0 * 1;
  if (Array.isArray(val0)) {
    // Regular array likely containing numbers, write values to memory
    let offset = 0;
    const dv0 = new DataView(memory0.buffer);
    for (const v of val0) {
      dv0.setUint8(ptr0+ offset, v, true);
      offset += 1;
    }
  } else {
    // TypedArray / ArrayBuffer-like, direct copy
    valData0 = new Uint8Array(val0.buffer || val0, val0.byteOffset, valLenBytes0);
    const out0 = new Uint8Array(memory0.buffer, ptr0,valLenBytes0);
    out0.set(valData0);
  }
  
  var {entries: v1_0 } = arg1;
  var vec5 = v1_0;
  var len5 = vec5.length;
  var result5 = realloc0(0, 0, 4, len5 * 16);
  for (let i = 0; i < vec5.length; i++) {
    const e = vec5[i];
    const base = result5 + i * 16;var {key: v2_0, value: v2_1 } = e;
    
    var encodeRes = _utf8AllocateAndEncode(v2_0, realloc0, memory0);
    var ptr3= encodeRes.ptr;
    var len3 = encodeRes.len;
    
    dataView(memory0).setUint32(base + 4, len3, true);
    dataView(memory0).setUint32(base + 0, ptr3, true);
    
    var encodeRes = _utf8AllocateAndEncode(v2_1, realloc0, memory0);
    var ptr4= encodeRes.ptr;
    var len4 = encodeRes.len;
    
    dataView(memory0).setUint32(base + 12, len4, true);
    dataView(memory0).setUint32(base + 8, ptr4, true);
  }
  _debugLog('[iface="pulseengine:rivet/adapter@0.1.0", function="import"][Instruction::CallWasm] enter', {
    funcName: 'import',
    paramCount: 4,
    async: false,
    postReturn: true,
  });
  const hostProvided = false;
  
  const [task, _wasm_call_currentTaskID] = createNewCurrentTask({
    componentIdx: 0,
    isAsync: false,
    entryFnName: 'adapter010Import',
    getCallbackFn: () => null,
    callbackFnName: 'null',
    errHandling: 'throw-result-err',
    callingWasmExport: true,
  });
  
  let ret = adapter010Import(ptr0, len0, result5, len5);
  endCurrentTask(0);
  let variant31;
  switch (dataView(memory0).getUint8(ret + 0, true)) {
    case 0: {
      var len25 = dataView(memory0).getUint32(ret + 8, true);
      var base25 = dataView(memory0).getUint32(ret + 4, true);
      var result25 = [];
      for (let i = 0; i < len25; i++) {
        const base = base25 + i * 72;
        var ptr6 = dataView(memory0).getUint32(base + 0, true);
        var len6 = dataView(memory0).getUint32(base + 4, true);
        var result6 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr6, len6));
        var ptr7 = dataView(memory0).getUint32(base + 8, true);
        var len7 = dataView(memory0).getUint32(base + 12, true);
        var result7 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr7, len7));
        var ptr8 = dataView(memory0).getUint32(base + 16, true);
        var len8 = dataView(memory0).getUint32(base + 20, true);
        var result8 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr8, len8));
        let variant10;
        switch (dataView(memory0).getUint8(base + 24, true)) {
          case 0: {
            variant10 = undefined;
            break;
          }
          case 1: {
            var ptr9 = dataView(memory0).getUint32(base + 28, true);
            var len9 = dataView(memory0).getUint32(base + 32, true);
            var result9 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr9, len9));
            variant10 = result9;
            break;
          }
          default: {
            throw new TypeError('invalid variant discriminant for option');
          }
        }
        let variant12;
        switch (dataView(memory0).getUint8(base + 36, true)) {
          case 0: {
            variant12 = undefined;
            break;
          }
          case 1: {
            var ptr11 = dataView(memory0).getUint32(base + 40, true);
            var len11 = dataView(memory0).getUint32(base + 44, true);
            var result11 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr11, len11));
            variant12 = result11;
            break;
          }
          default: {
            throw new TypeError('invalid variant discriminant for option');
          }
        }
        var len14 = dataView(memory0).getUint32(base + 52, true);
        var base14 = dataView(memory0).getUint32(base + 48, true);
        var result14 = [];
        for (let i = 0; i < len14; i++) {
          const base = base14 + i * 8;
          var ptr13 = dataView(memory0).getUint32(base + 0, true);
          var len13 = dataView(memory0).getUint32(base + 4, true);
          var result13 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr13, len13));
          result14.push(result13);
        }
        var len17 = dataView(memory0).getUint32(base + 60, true);
        var base17 = dataView(memory0).getUint32(base + 56, true);
        var result17 = [];
        for (let i = 0; i < len17; i++) {
          const base = base17 + i * 16;
          var ptr15 = dataView(memory0).getUint32(base + 0, true);
          var len15 = dataView(memory0).getUint32(base + 4, true);
          var result15 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr15, len15));
          var ptr16 = dataView(memory0).getUint32(base + 8, true);
          var len16 = dataView(memory0).getUint32(base + 12, true);
          var result16 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr16, len16));
          result17.push({
            linkType: result15,
            target: result16,
          });
        }
        var len24 = dataView(memory0).getUint32(base + 68, true);
        var base24 = dataView(memory0).getUint32(base + 64, true);
        var result24 = [];
        for (let i = 0; i < len24; i++) {
          const base = base24 + i * 24;
          var ptr18 = dataView(memory0).getUint32(base + 0, true);
          var len18 = dataView(memory0).getUint32(base + 4, true);
          var result18 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr18, len18));
          let variant23;
          switch (dataView(memory0).getUint8(base + 8, true)) {
            case 0: {
              var ptr19 = dataView(memory0).getUint32(base + 16, true);
              var len19 = dataView(memory0).getUint32(base + 20, true);
              var result19 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr19, len19));
              variant23= {
                tag: 'text',
                val: result19
              };
              break;
            }
            case 1: {
              variant23= {
                tag: 'number',
                val: dataView(memory0).getFloat64(base + 16, true)
              };
              break;
            }
            case 2: {
              var bool20 = dataView(memory0).getUint8(base + 16, true);
              variant23= {
                tag: 'boolean',
                val: bool20 == 0 ? false : (bool20 == 1 ? true : throwInvalidBool())
              };
              break;
            }
            case 3: {
              var len22 = dataView(memory0).getUint32(base + 20, true);
              var base22 = dataView(memory0).getUint32(base + 16, true);
              var result22 = [];
              for (let i = 0; i < len22; i++) {
                const base = base22 + i * 8;
                var ptr21 = dataView(memory0).getUint32(base + 0, true);
                var len21 = dataView(memory0).getUint32(base + 4, true);
                var result21 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr21, len21));
                result22.push(result21);
              }
              variant23= {
                tag: 'text-list',
                val: result22
              };
              break;
            }
            default: {
              throw new TypeError('invalid variant discriminant for FieldValue');
            }
          }
          result24.push({
            key: result18,
            value: variant23,
          });
        }
        result25.push({
          id: result6,
          artifactType: result7,
          title: result8,
          description: variant10,
          status: variant12,
          tags: result14,
          links: result17,
          fields: result24,
        });
      }
      variant31= {
        tag: 'ok',
        val: result25
      };
      break;
    }
    case 1: {
      let variant30;
      switch (dataView(memory0).getUint8(ret + 4, true)) {
        case 0: {
          var ptr26 = dataView(memory0).getUint32(ret + 8, true);
          var len26 = dataView(memory0).getUint32(ret + 12, true);
          var result26 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr26, len26));
          variant30= {
            tag: 'parse-error',
            val: result26
          };
          break;
        }
        case 1: {
          var ptr27 = dataView(memory0).getUint32(ret + 8, true);
          var len27 = dataView(memory0).getUint32(ret + 12, true);
          var result27 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr27, len27));
          variant30= {
            tag: 'validation-error',
            val: result27
          };
          break;
        }
        case 2: {
          var ptr28 = dataView(memory0).getUint32(ret + 8, true);
          var len28 = dataView(memory0).getUint32(ret + 12, true);
          var result28 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr28, len28));
          variant30= {
            tag: 'io-error',
            val: result28
          };
          break;
        }
        case 3: {
          var ptr29 = dataView(memory0).getUint32(ret + 8, true);
          var len29 = dataView(memory0).getUint32(ret + 12, true);
          var result29 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr29, len29));
          variant30= {
            tag: 'not-supported',
            val: result29
          };
          break;
        }
        default: {
          throw new TypeError('invalid variant discriminant for AdapterError');
        }
      }
      variant31= {
        tag: 'err',
        val: variant30
      };
      break;
    }
    default: {
      throw new TypeError('invalid variant discriminant for expected');
    }
  }
  _debugLog('[iface="pulseengine:rivet/adapter@0.1.0", function="import"][Instruction::Return]', {
    funcName: 'import',
    paramCount: 1,
    async: false,
    postReturn: true
  });
  const retCopy = variant31;
  
  let cstate = getOrCreateAsyncState(0);
  cstate.mayLeave = false;
  postReturn2(ret);
  cstate.mayLeave = true;
  
  
  
  if (typeof retCopy === 'object' && retCopy.tag === 'err') {
    throw new ComponentError(retCopy.val);
  }
  return retCopy.val;
  
}
let adapter010Export;

function _export(arg0, arg1) {
  var vec21 = arg0;
  var len21 = vec21.length;
  var result21 = realloc0(0, 0, 4, len21 * 72);
  for (let i = 0; i < vec21.length; i++) {
    const e = vec21[i];
    const base = result21 + i * 72;var {id: v0_0, artifactType: v0_1, title: v0_2, description: v0_3, status: v0_4, tags: v0_5, links: v0_6, fields: v0_7 } = e;
    
    var encodeRes = _utf8AllocateAndEncode(v0_0, realloc0, memory0);
    var ptr1= encodeRes.ptr;
    var len1 = encodeRes.len;
    
    dataView(memory0).setUint32(base + 4, len1, true);
    dataView(memory0).setUint32(base + 0, ptr1, true);
    
    var encodeRes = _utf8AllocateAndEncode(v0_1, realloc0, memory0);
    var ptr2= encodeRes.ptr;
    var len2 = encodeRes.len;
    
    dataView(memory0).setUint32(base + 12, len2, true);
    dataView(memory0).setUint32(base + 8, ptr2, true);
    
    var encodeRes = _utf8AllocateAndEncode(v0_2, realloc0, memory0);
    var ptr3= encodeRes.ptr;
    var len3 = encodeRes.len;
    
    dataView(memory0).setUint32(base + 20, len3, true);
    dataView(memory0).setUint32(base + 16, ptr3, true);
    var variant5 = v0_3;
    if (variant5 === null || variant5=== undefined) {
      dataView(memory0).setInt8(base + 24, 0, true);
    } else {
      const e = variant5;
      dataView(memory0).setInt8(base + 24, 1, true);
      
      var encodeRes = _utf8AllocateAndEncode(e, realloc0, memory0);
      var ptr4= encodeRes.ptr;
      var len4 = encodeRes.len;
      
      dataView(memory0).setUint32(base + 32, len4, true);
      dataView(memory0).setUint32(base + 28, ptr4, true);
    }
    var variant7 = v0_4;
    if (variant7 === null || variant7=== undefined) {
      dataView(memory0).setInt8(base + 36, 0, true);
    } else {
      const e = variant7;
      dataView(memory0).setInt8(base + 36, 1, true);
      
      var encodeRes = _utf8AllocateAndEncode(e, realloc0, memory0);
      var ptr6= encodeRes.ptr;
      var len6 = encodeRes.len;
      
      dataView(memory0).setUint32(base + 44, len6, true);
      dataView(memory0).setUint32(base + 40, ptr6, true);
    }
    var vec9 = v0_5;
    var len9 = vec9.length;
    var result9 = realloc0(0, 0, 4, len9 * 8);
    for (let i = 0; i < vec9.length; i++) {
      const e = vec9[i];
      const base = result9 + i * 8;
      var encodeRes = _utf8AllocateAndEncode(e, realloc0, memory0);
      var ptr8= encodeRes.ptr;
      var len8 = encodeRes.len;
      
      dataView(memory0).setUint32(base + 4, len8, true);
      dataView(memory0).setUint32(base + 0, ptr8, true);
    }
    dataView(memory0).setUint32(base + 52, len9, true);
    dataView(memory0).setUint32(base + 48, result9, true);
    var vec13 = v0_6;
    var len13 = vec13.length;
    var result13 = realloc0(0, 0, 4, len13 * 16);
    for (let i = 0; i < vec13.length; i++) {
      const e = vec13[i];
      const base = result13 + i * 16;var {linkType: v10_0, target: v10_1 } = e;
      
      var encodeRes = _utf8AllocateAndEncode(v10_0, realloc0, memory0);
      var ptr11= encodeRes.ptr;
      var len11 = encodeRes.len;
      
      dataView(memory0).setUint32(base + 4, len11, true);
      dataView(memory0).setUint32(base + 0, ptr11, true);
      
      var encodeRes = _utf8AllocateAndEncode(v10_1, realloc0, memory0);
      var ptr12= encodeRes.ptr;
      var len12 = encodeRes.len;
      
      dataView(memory0).setUint32(base + 12, len12, true);
      dataView(memory0).setUint32(base + 8, ptr12, true);
    }
    dataView(memory0).setUint32(base + 60, len13, true);
    dataView(memory0).setUint32(base + 56, result13, true);
    var vec20 = v0_7;
    var len20 = vec20.length;
    var result20 = realloc0(0, 0, 8, len20 * 24);
    for (let i = 0; i < vec20.length; i++) {
      const e = vec20[i];
      const base = result20 + i * 24;var {key: v14_0, value: v14_1 } = e;
      
      var encodeRes = _utf8AllocateAndEncode(v14_0, realloc0, memory0);
      var ptr15= encodeRes.ptr;
      var len15 = encodeRes.len;
      
      dataView(memory0).setUint32(base + 4, len15, true);
      dataView(memory0).setUint32(base + 0, ptr15, true);
      var variant19 = v14_1;
      switch (variant19.tag) {
        case 'text': {
          const e = variant19.val;
          dataView(memory0).setInt8(base + 8, 0, true);
          
          var encodeRes = _utf8AllocateAndEncode(e, realloc0, memory0);
          var ptr16= encodeRes.ptr;
          var len16 = encodeRes.len;
          
          dataView(memory0).setUint32(base + 20, len16, true);
          dataView(memory0).setUint32(base + 16, ptr16, true);
          break;
        }
        case 'number': {
          const e = variant19.val;
          dataView(memory0).setInt8(base + 8, 1, true);
          dataView(memory0).setFloat64(base + 16, +e, true);
          break;
        }
        case 'boolean': {
          const e = variant19.val;
          dataView(memory0).setInt8(base + 8, 2, true);
          dataView(memory0).setInt8(base + 16, e ? 1 : 0, true);
          break;
        }
        case 'text-list': {
          const e = variant19.val;
          dataView(memory0).setInt8(base + 8, 3, true);
          var vec18 = e;
          var len18 = vec18.length;
          var result18 = realloc0(0, 0, 4, len18 * 8);
          for (let i = 0; i < vec18.length; i++) {
            const e = vec18[i];
            const base = result18 + i * 8;
            var encodeRes = _utf8AllocateAndEncode(e, realloc0, memory0);
            var ptr17= encodeRes.ptr;
            var len17 = encodeRes.len;
            
            dataView(memory0).setUint32(base + 4, len17, true);
            dataView(memory0).setUint32(base + 0, ptr17, true);
          }
          dataView(memory0).setUint32(base + 20, len18, true);
          dataView(memory0).setUint32(base + 16, result18, true);
          break;
        }
        default: {
          throw new TypeError(`invalid variant tag value \`${JSON.stringify(variant19.tag)}\` (received \`${variant19}\`) specified for \`FieldValue\``);
        }
      }
    }
    dataView(memory0).setUint32(base + 68, len20, true);
    dataView(memory0).setUint32(base + 64, result20, true);
  }
  var {entries: v22_0 } = arg1;
  var vec26 = v22_0;
  var len26 = vec26.length;
  var result26 = realloc0(0, 0, 4, len26 * 16);
  for (let i = 0; i < vec26.length; i++) {
    const e = vec26[i];
    const base = result26 + i * 16;var {key: v23_0, value: v23_1 } = e;
    
    var encodeRes = _utf8AllocateAndEncode(v23_0, realloc0, memory0);
    var ptr24= encodeRes.ptr;
    var len24 = encodeRes.len;
    
    dataView(memory0).setUint32(base + 4, len24, true);
    dataView(memory0).setUint32(base + 0, ptr24, true);
    
    var encodeRes = _utf8AllocateAndEncode(v23_1, realloc0, memory0);
    var ptr25= encodeRes.ptr;
    var len25 = encodeRes.len;
    
    dataView(memory0).setUint32(base + 12, len25, true);
    dataView(memory0).setUint32(base + 8, ptr25, true);
  }
  _debugLog('[iface="pulseengine:rivet/adapter@0.1.0", function="export"][Instruction::CallWasm] enter', {
    funcName: 'export',
    paramCount: 4,
    async: false,
    postReturn: true,
  });
  const hostProvided = false;
  
  const [task, _wasm_call_currentTaskID] = createNewCurrentTask({
    componentIdx: 0,
    isAsync: false,
    entryFnName: 'adapter010Export',
    getCallbackFn: () => null,
    callbackFnName: 'null',
    errHandling: 'throw-result-err',
    callingWasmExport: true,
  });
  
  let ret = adapter010Export(result21, len21, result26, len26);
  endCurrentTask(0);
  let variant33;
  switch (dataView(memory0).getUint8(ret + 0, true)) {
    case 0: {
      var ptr27 = dataView(memory0).getUint32(ret + 4, true);
      var len27 = dataView(memory0).getUint32(ret + 8, true);
      var result27 = new Uint8Array(memory0.buffer.slice(ptr27, ptr27 + len27 * 1));
      variant33= {
        tag: 'ok',
        val: result27
      };
      break;
    }
    case 1: {
      let variant32;
      switch (dataView(memory0).getUint8(ret + 4, true)) {
        case 0: {
          var ptr28 = dataView(memory0).getUint32(ret + 8, true);
          var len28 = dataView(memory0).getUint32(ret + 12, true);
          var result28 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr28, len28));
          variant32= {
            tag: 'parse-error',
            val: result28
          };
          break;
        }
        case 1: {
          var ptr29 = dataView(memory0).getUint32(ret + 8, true);
          var len29 = dataView(memory0).getUint32(ret + 12, true);
          var result29 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr29, len29));
          variant32= {
            tag: 'validation-error',
            val: result29
          };
          break;
        }
        case 2: {
          var ptr30 = dataView(memory0).getUint32(ret + 8, true);
          var len30 = dataView(memory0).getUint32(ret + 12, true);
          var result30 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr30, len30));
          variant32= {
            tag: 'io-error',
            val: result30
          };
          break;
        }
        case 3: {
          var ptr31 = dataView(memory0).getUint32(ret + 8, true);
          var len31 = dataView(memory0).getUint32(ret + 12, true);
          var result31 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr31, len31));
          variant32= {
            tag: 'not-supported',
            val: result31
          };
          break;
        }
        default: {
          throw new TypeError('invalid variant discriminant for AdapterError');
        }
      }
      variant33= {
        tag: 'err',
        val: variant32
      };
      break;
    }
    default: {
      throw new TypeError('invalid variant discriminant for expected');
    }
  }
  _debugLog('[iface="pulseengine:rivet/adapter@0.1.0", function="export"][Instruction::Return]', {
    funcName: 'export',
    paramCount: 1,
    async: false,
    postReturn: true
  });
  const retCopy = variant33;
  
  let cstate = getOrCreateAsyncState(0);
  cstate.mayLeave = false;
  postReturn3(ret);
  cstate.mayLeave = true;
  
  
  
  if (typeof retCopy === 'object' && retCopy.tag === 'err') {
    throw new ComponentError(retCopy.val);
  }
  return retCopy.val;
  
}
let renderer010Render;

function render(arg0, arg1) {
  
  var encodeRes = _utf8AllocateAndEncode(arg0, realloc0, memory0);
  var ptr0= encodeRes.ptr;
  var len0 = encodeRes.len;
  
  var vec2 = arg1;
  var len2 = vec2.length;
  var result2 = realloc0(0, 0, 4, len2 * 8);
  for (let i = 0; i < vec2.length; i++) {
    const e = vec2[i];
    const base = result2 + i * 8;
    var encodeRes = _utf8AllocateAndEncode(e, realloc0, memory0);
    var ptr1= encodeRes.ptr;
    var len1 = encodeRes.len;
    
    dataView(memory0).setUint32(base + 4, len1, true);
    dataView(memory0).setUint32(base + 0, ptr1, true);
  }
  _debugLog('[iface="pulseengine:rivet/renderer@0.1.0", function="render"][Instruction::CallWasm] enter', {
    funcName: 'render',
    paramCount: 4,
    async: false,
    postReturn: true,
  });
  const hostProvided = false;
  
  const [task, _wasm_call_currentTaskID] = createNewCurrentTask({
    componentIdx: 0,
    isAsync: false,
    entryFnName: 'renderer010Render',
    getCallbackFn: () => null,
    callbackFnName: 'null',
    errHandling: 'throw-result-err',
    callingWasmExport: true,
  });
  
  let ret = renderer010Render(ptr0, len0, result2, len2);
  endCurrentTask(0);
  let variant8;
  switch (dataView(memory0).getUint8(ret + 0, true)) {
    case 0: {
      var ptr3 = dataView(memory0).getUint32(ret + 4, true);
      var len3 = dataView(memory0).getUint32(ret + 8, true);
      var result3 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr3, len3));
      variant8= {
        tag: 'ok',
        val: result3
      };
      break;
    }
    case 1: {
      let variant7;
      switch (dataView(memory0).getUint8(ret + 4, true)) {
        case 0: {
          var ptr4 = dataView(memory0).getUint32(ret + 8, true);
          var len4 = dataView(memory0).getUint32(ret + 12, true);
          var result4 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr4, len4));
          variant7= {
            tag: 'parse-error',
            val: result4
          };
          break;
        }
        case 1: {
          var ptr5 = dataView(memory0).getUint32(ret + 8, true);
          var len5 = dataView(memory0).getUint32(ret + 12, true);
          var result5 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr5, len5));
          variant7= {
            tag: 'no-root',
            val: result5
          };
          break;
        }
        case 2: {
          var ptr6 = dataView(memory0).getUint32(ret + 8, true);
          var len6 = dataView(memory0).getUint32(ret + 12, true);
          var result6 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr6, len6));
          variant7= {
            tag: 'layout-error',
            val: result6
          };
          break;
        }
        default: {
          throw new TypeError('invalid variant discriminant for RenderError');
        }
      }
      variant8= {
        tag: 'err',
        val: variant7
      };
      break;
    }
    default: {
      throw new TypeError('invalid variant discriminant for expected');
    }
  }
  _debugLog('[iface="pulseengine:rivet/renderer@0.1.0", function="render"][Instruction::Return]', {
    funcName: 'render',
    paramCount: 1,
    async: false,
    postReturn: true
  });
  const retCopy = variant8;
  
  let cstate = getOrCreateAsyncState(0);
  cstate.mayLeave = false;
  postReturn3(ret);
  cstate.mayLeave = true;
  
  
  
  if (typeof retCopy === 'object' && retCopy.tag === 'err') {
    throw new ComponentError(retCopy.val);
  }
  return retCopy.val;
  
}

const $init = (() => {
  let gen = (function* _initGenerator () {
    const module0 = fetchCompile(new URL('./spar_wasm.core.wasm', import.meta.url));
    const module1 = base64Compile('AGFzbQEAAAABKQZgAX8AYAN/fn8AYAJ/fwBgBH9/f38AYAd/f39/f39/AGAFf39/f38AAxYVAAECAwIBAQICAgIEAgUCAAAAAAAABAUBcAEVFQdrFgEwAAABMQABATIAAgEzAAMBNAAEATUABQE2AAYBNwAHATgACAE5AAkCMTAACgIxMQALAjEyAAwCMTMADQIxNAAOAjE1AA8CMTYAEAIxNwARAjE4ABICMTkAEwIyMAAUCCRpbXBvcnRzAQAKiQIVCQAgAEEAEQAACw0AIAAgASACQQERAQALCwAgACABQQIRAgALDwAgACABIAIgA0EDEQMACwsAIAAgAUEEEQIACw0AIAAgASACQQURAQALDQAgACABIAJBBhEBAAsLACAAIAFBBxECAAsLACAAIAFBCBECAAsLACAAIAFBCRECAAsLACAAIAFBChECAAsVACAAIAEgAiADIAQgBSAGQQsRBAALCwAgACABQQwRAgALEQAgACABIAIgAyAEQQ0RBQALCwAgACABQQ4RAgALCQAgAEEPEQAACwkAIABBEBEAAAsJACAAQRERAAALCQAgAEESEQAACwkAIABBExEAAAsJACAAQRQRAAALAC8JcHJvZHVjZXJzAQxwcm9jZXNzZWQtYnkBDXdpdC1jb21wb25lbnQHMC4yNDUuMQ');
    const module2 = base64Compile('AGFzbQEAAAABKQZgAX8AYAN/fn8AYAJ/fwBgBH9/f38AYAd/f39/f39/AGAFf39/f38AAoQBFgABMAAAAAExAAEAATIAAgABMwADAAE0AAIAATUAAQABNgABAAE3AAIAATgAAgABOQACAAIxMAACAAIxMQAEAAIxMgACAAIxMwAFAAIxNAACAAIxNQAAAAIxNgAAAAIxNwAAAAIxOAAAAAIxOQAAAAIyMAAAAAgkaW1wb3J0cwFwARUVCRsBAEEACxUAAQIDBAUGBwgJCgsMDQ4PEBESExQALwlwcm9kdWNlcnMBDHByb2Nlc3NlZC1ieQENd2l0LWNvbXBvbmVudAcwLjI0NS4x');
    ({ exports: exports0 } = yield instantiateCore(yield module1));
    ({ exports: exports1 } = yield instantiateCore(yield module0, {
      'wasi:cli/environment@0.2.0': {
        'get-environment': exports0['15'],
      },
      'wasi:cli/exit@0.2.0': {
        exit: trampoline8,
      },
      'wasi:cli/stderr@0.2.0': {
        'get-stderr': trampoline14,
      },
      'wasi:cli/stdin@0.2.0': {
        'get-stdin': trampoline12,
      },
      'wasi:cli/stdout@0.2.0': {
        'get-stdout': trampoline13,
      },
      'wasi:cli/terminal-input@0.2.0': {
        '[resource-drop]terminal-input': trampoline4,
      },
      'wasi:cli/terminal-output@0.2.0': {
        '[resource-drop]terminal-output': trampoline5,
      },
      'wasi:cli/terminal-stderr@0.2.0': {
        'get-terminal-stderr': exports0['18'],
      },
      'wasi:cli/terminal-stdin@0.2.0': {
        'get-terminal-stdin': exports0['16'],
      },
      'wasi:cli/terminal-stdout@0.2.0': {
        'get-terminal-stdout': exports0['17'],
      },
      'wasi:clocks/monotonic-clock@0.2.0': {
        now: trampoline15,
      },
      'wasi:clocks/wall-clock@0.2.0': {
        now: exports0['19'],
      },
      'wasi:filesystem/preopens@0.2.0': {
        'get-directories': exports0['20'],
      },
      'wasi:filesystem/types@0.2.0': {
        '[method]descriptor.append-via-stream': exports0['7'],
        '[method]descriptor.get-flags': exports0['8'],
        '[method]descriptor.metadata-hash': exports0['12'],
        '[method]descriptor.metadata-hash-at': exports0['13'],
        '[method]descriptor.open-at': exports0['11'],
        '[method]descriptor.read-directory': exports0['9'],
        '[method]descriptor.read-via-stream': exports0['5'],
        '[method]descriptor.stat': exports0['10'],
        '[method]descriptor.write-via-stream': exports0['6'],
        '[method]directory-entry-stream.read-directory-entry': exports0['14'],
        '[resource-drop]descriptor': trampoline6,
        '[resource-drop]directory-entry-stream': trampoline7,
      },
      'wasi:io/error@0.2.0': {
        '[resource-drop]error': trampoline0,
      },
      'wasi:io/poll@0.2.0': {
        '[method]pollable.block': trampoline9,
        '[resource-drop]pollable': trampoline1,
      },
      'wasi:io/streams@0.2.0': {
        '[method]input-stream.blocking-read': exports0['1'],
        '[method]input-stream.subscribe': trampoline10,
        '[method]output-stream.blocking-flush': exports0['4'],
        '[method]output-stream.check-write': exports0['2'],
        '[method]output-stream.subscribe': trampoline11,
        '[method]output-stream.write': exports0['3'],
        '[resource-drop]input-stream': trampoline2,
        '[resource-drop]output-stream': trampoline3,
      },
      'wasi:random/insecure-seed@0.2.4': {
        'insecure-seed': exports0['0'],
      },
    }));
    memory0 = exports1.memory;
    GlobalComponentMemories.save({ idx: 0, componentIdx: 1, memory: memory0 });
    realloc0 = exports1.cabi_realloc;
    ({ exports: exports2 } = yield instantiateCore(yield module2, {
      '': {
        $imports: exports0.$imports,
        '0': trampoline16,
        '1': trampoline17,
        '10': trampoline26,
        '11': trampoline27,
        '12': trampoline28,
        '13': trampoline29,
        '14': trampoline30,
        '15': trampoline31,
        '16': trampoline32,
        '17': trampoline33,
        '18': trampoline34,
        '19': trampoline35,
        '2': trampoline18,
        '20': trampoline36,
        '3': trampoline19,
        '4': trampoline20,
        '5': trampoline21,
        '6': trampoline22,
        '7': trampoline23,
        '8': trampoline24,
        '9': trampoline25,
      },
    }));
    postReturn0 = exports1['cabi_post_pulseengine:rivet/adapter@0.1.0#id'];
    postReturn1 = exports1['cabi_post_pulseengine:rivet/adapter@0.1.0#supported-types'];
    postReturn2 = exports1['cabi_post_pulseengine:rivet/adapter@0.1.0#import'];
    postReturn3 = exports1['cabi_post_pulseengine:rivet/adapter@0.1.0#export'];
    adapter010Id = exports1['pulseengine:rivet/adapter@0.1.0#id'];
    adapter010Name = exports1['pulseengine:rivet/adapter@0.1.0#name'];
    adapter010SupportedTypes = exports1['pulseengine:rivet/adapter@0.1.0#supported-types'];
    adapter010Import = exports1['pulseengine:rivet/adapter@0.1.0#import'];
    adapter010Export = exports1['pulseengine:rivet/adapter@0.1.0#export'];
    renderer010Render = exports1['pulseengine:rivet/renderer@0.1.0#render'];
  })();
  let promise, resolve, reject;
  function runNext (value) {
    try {
      let done;
      do {
        ({ value, done } = gen.next(value));
      } while (!(value instanceof Promise) && !done);
      if (done) {
        if (resolve) resolve(value);
        else return value;
      }
      if (!promise) promise = new Promise((_resolve, _reject) => (resolve = _resolve, reject = _reject));
      value.then(runNext, reject);
    }
    catch (e) {
      if (reject) reject(e);
      else throw e;
    }
  }
  const maybeSyncReturn = runNext(null);
  return promise || maybeSyncReturn;
})();

await $init;
const adapter010 = {
  'export': _export,
  id: id,
  'import': _import,
  name: name,
  supportedTypes: supportedTypes,
  
};
const renderer010 = {
  render: render,
  
};

export { adapter010 as adapter, renderer010 as renderer, adapter010 as 'pulseengine:rivet/adapter@0.1.0', renderer010 as 'pulseengine:rivet/renderer@0.1.0',  }