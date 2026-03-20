module.exports.instantiate = function instantiate(getCoreModule, imports, instantiateCore = WebAssembly.instantiate) {
  
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
  
  const _debugLog = (...args) => {
    if (!globalThis?.process?.env?.JCO_DEBUG) { return; }
    console.debug(...args);
  };
  const ASYNC_DETERMINISM = 'random';
  const GLOBAL_COMPONENT_MEMORY_MAP = new Map();
  const CURRENT_TASK_META = {};
  
  function _getGlobalCurrentTaskMeta(componentIdx) {
    const v = CURRENT_TASK_META[componentIdx];
    if (v === undefined) { return v; }
    return { ...v };
  }
  
  function _setGlobalCurrentTaskMeta(args) {
    if (!args) { throw new TypeError('args missing'); }
    if (args.taskID === undefined) { throw new TypeError('missing task ID'); }
    if (args.componentIdx === undefined) { throw new TypeError('missing component idx'); }
    const { taskID, componentIdx } = args;
    return CURRENT_TASK_META[componentIdx] = { taskID, componentIdx };
  }
  
  function _withGlobalCurrentTaskMeta(args) {
    _debugLog('[_withGlobalCurrentTaskMeta()] args', args);
    if (!args) { throw new TypeError('args missing'); }
    if (args.taskID === undefined) { throw new TypeError('missing task ID'); }
    if (args.componentIdx === undefined) { throw new TypeError('missing component idx'); }
    if (!args.fn) { throw new TypeError('missing fn'); }
    const { taskID, componentIdx, fn } = args;
    
    try {
      CURRENT_TASK_META[componentIdx] = { taskID, componentIdx };
      return fn();
    } catch (err) {
      _debugLog("error while executing sync callee/callback", {
        ...args,
        err,
      });
      throw err;
    } finally {
      CURRENT_TASK_META[componentIdx] = null;
    }
  }
  
  async function _withGlobalCurrentTaskMetaAsync(args) {
    _debugLog('[_withGlobalCurrentTaskMetaAsync()] args', args);
    if (!args) { throw new TypeError('args missing'); }
    if (args.taskID === undefined) { throw new TypeError('missing task ID'); }
    if (args.componentIdx === undefined) { throw new TypeError('missing component idx'); }
    if (!args.fn) { throw new TypeError('missing fn'); }
    const { taskID, componentIdx, fn } = args;
    
    // If there is already an async task executing, we must wait for it
    // to complete before we can can run the closure we were given
    //
    let current = CURRENT_TASK_META[componentIdx];
    let cstate;
    if (current && current.taskID !== taskID) {
      cstate = getOrCreateAsyncState(componentIdx);
      while (current && current.taskID !== taskID) {
        const { promise, resolve } = Promise.withResolvers();
        cstate.onNextExclusiveRelease(resolve);
        await promise;
        current = CURRENT_TASK_META[componentIdx];
      }
      
      // Since we've just waited for the component to not be locked, re-lock
      // exclusivity so we can run the fn below (likely a callee/callback)
      cstate.exclusiveLock();
    }
    
    try {
      CURRENT_TASK_META[componentIdx] = { taskID, componentIdx };
      return await fn();
    } catch (err) {
      _debugLog("error while executing async callee/callback", {
        ...args,
        err,
      });
      throw err;
    } finally {
      CURRENT_TASK_META[componentIdx] = null;
    }
  }
  
  async function _clearCurrentTask(args) {
    _debugLog('[_clearCurrentTask()] args', args);
    if (!args) { throw new TypeError('args missing'); }
    if (args.taskID === undefined) { throw new TypeError('missing task ID'); }
    if (args.componentIdx === undefined) { throw new TypeError('missing component idx'); }
    const { taskID, componentIdx } = args;
    
    const meta = CURRENT_TASK_META[componentIdx];
    if (!meta) { throw new Error(`missing current task meta for component idx [${componentIdx}]`); }
    
    if (meta.taskID !== taskID) {
      throw new Error(`task ID [${meta.taskID}] != requested ID [${taskID}]`);
    }
    if (meta.componentIdx !== componentIdx) {
      throw new Error(`component idx [${meta.componentIdx}] != requested idx [${componentIdx}]`);
    }
    
    CURRENT_TASK_META[componentIdx] = null;
  }
  
  function lookupMemoriesForComponent(args) {
    const { componentIdx } = args ?? {};
    if (args.componentIdx === undefined) { throw new TypeError("missing component idx"); }
    
    const metas = GLOBAL_COMPONENT_MEMORY_MAP.get(componentIdx);
    if (!metas) { return []; }
    
    if (args.memoryIdx === undefined) {
      return Object.values(metas);
    }
    
    const meta = metas[args.memoryIdx];
    return meta?.memory;
  }
  
  function registerGlobalMemoryForComponent(args) {
    const { componentIdx, memory, memoryIdx } = args ?? {};
    if (componentIdx === undefined) { throw new TypeError('missing component idx'); }
    if (memory === undefined && memoryIdx === undefined) { throw new TypeError('missing both memory & memory idx'); }
    let inner = GLOBAL_COMPONENT_MEMORY_MAP.get(componentIdx);
    if (!inner) {
      inner = {};
      GLOBAL_COMPONENT_MEMORY_MAP.set(componentIdx, inner);
    }
    
    inner[memoryIdx] = { memory, memoryIdx, componentIdx };
  }
  
  class RepTable {
    #data = [0, null];
    #target;
    
    constructor(args) {
      this.target = args?.target;
    }
    
    data() { return this.#data; }
    
    insert(val) {
      _debugLog('[RepTable#insert()] args', { val, target: this.target });
      const freeIdx = this.#data[0];
      if (freeIdx === 0) {
        this.#data.push(val);
        this.#data.push(null);
        const rep = (this.#data.length >> 1) - 1;
        _debugLog('[RepTable#insert()] inserted', { val, target: this.target, rep });
        return rep;
      }
      this.#data[0] = this.#data[freeIdx << 1];
      const placementIdx = freeIdx << 1;
      this.#data[placementIdx] = val;
      this.#data[placementIdx + 1] = null;
      _debugLog('[RepTable#insert()] inserted', { val, target: this.target, rep: freeIdx });
      return freeIdx;
    }
    
    get(rep) {
      _debugLog('[RepTable#get()] args', { rep, target: this.target });
      if (rep === 0) { throw new Error('invalid resource rep during get, (cannot be 0)'); }
      
      const baseIdx = rep << 1;
      const val = this.#data[baseIdx];
      return val;
    }
    
    contains(rep) {
      _debugLog('[RepTable#contains()] args', { rep, target: this.target });
      if (rep === 0) { throw new Error('invalid resource rep during contains, (cannot be 0)'); }
      
      const baseIdx = rep << 1;
      return !!this.#data[baseIdx];
    }
    
    remove(rep) {
      _debugLog('[RepTable#remove()] args', { rep, target: this.target });
      if (rep === 0) { throw new Error('invalid resource rep during remove, (cannot be 0)'); }
      if (this.#data.length === 2) { throw new Error('invalid'); }
      
      const baseIdx = rep << 1;
      const val = this.#data[baseIdx];
      
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
  
  function clearCurrentTask(componentIdx, taskID) {
    _debugLog('[clearCurrentTask()] args', { componentIdx, taskID });
    
    if (componentIdx === undefined || componentIdx === null) {
      throw new Error('missing/invalid component instance index while ending current task');
    }
    
    const tasks = ASYNC_TASKS_BY_COMPONENT_IDX.get(componentIdx);
    if (!tasks || !Array.isArray(tasks)) {
      throw new Error('missing/invalid tasks for component instance while ending task');
    }
    if (tasks.length == 0) {
      throw new Error(`no current tasks for component instance [${componentIdx}] while ending task`);
    }
    
    if (taskID !== undefined) {
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
  const CURRENT_TASK_MAY_BLOCK = new WebAssembly.Global({ value: 'i32', mutable: true }, 0);
  const ASYNC_CURRENT_TASK_IDS = [];
  const ASYNC_CURRENT_COMPONENT_IDXS = [];
  
  function unpackCallbackResult(result) {
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
  
  class AsyncSubtask {
    static _ID = 0n;
    
    static State = {
      STARTING: 0,
      STARTED: 1,
      RETURNED: 2,
      CANCELLED_BEFORE_STARTED: 3,
      CANCELLED_BEFORE_RETURNED: 4,
    };
    
    #id;
    #state = AsyncSubtask.State.STARTING;
    #componentIdx;
    
    #parentTask;
    #childTask = null;
    
    #dropped = false;
    #cancelRequested = false;
    
    #memoryIdx = null;
    #lenders = null;
    
    #waitable = null;
    
    #callbackFn = null;
    #callbackFnName = null;
    
    #postReturnFn = null;
    #onProgressFn = null;
    #pendingEventFn = null;
    
    #callMetadata = {};
    
    #resolved = false;
    
    #onResolveHandlers = [];
    #onStartHandlers = [];
    
    #result = null;
    #resultSet = false;
    
    fnName;
    target;
    isAsync;
    isManualAsync;
    
    constructor(args) {
      if (typeof args.componentIdx !== 'number') {
        throw new Error('invalid componentIdx for subtask creation');
      }
      this.#componentIdx = args.componentIdx;
      
      this.#id = ++AsyncSubtask._ID;
      this.fnName = args.fnName;
      
      if (!args.parentTask) { throw new Error('missing parent task during subtask creation'); }
      this.#parentTask = args.parentTask;
      
      if (args.childTask) { this.#childTask = args.childTask; }
      
      if (args.memoryIdx) { this.#memoryIdx = args.memoryIdx; }
      
      if (!args.waitable) { throw new Error("missing/invalid waitable"); }
      this.#waitable = args.waitable;
      
      if (args.callMetadata) { this.#callMetadata = args.callMetadata; }
      
      this.#lenders = [];
      this.target = args.target;
      this.isAsync = args.isAsync;
      this.isManualAsync = args.isManualAsync;
    }
    
    id() { return this.#id; }
    parentTaskID() { return this.#parentTask?.id(); }
    childTaskID() { return this.#childTask?.id(); }
    state() { return this.#state; }
    
    waitable() { return this.#waitable; }
    waitableRep() { return this.#waitable.idx(); }
    
    join() { return this.#waitable.join(...arguments); }
    getPendingEvent() { return this.#waitable.getPendingEvent(...arguments); }
    hasPendingEvent() { return this.#waitable.hasPendingEvent(...arguments); }
    setPendingEvent() { return this.#waitable.setPendingEvent(...arguments); }
    
    setTarget(tgt) { this.target = tgt; }
    
    getResult() {
      if (!this.#resultSet) { throw new Error("subtask result has not been set") }
      return this.#result;
    }
    setResult(v) {
      if (this.#resultSet) { throw new Error("subtask result has already been set"); }
      this.#result = v;
      this.#resultSet = true;
    }
    
    componentIdx() { return this.#componentIdx; }
    
    setChildTask(t) {
      if (!t) { throw new Error('cannot set missing/invalid child task on subtask'); }
      if (this.#childTask) { throw new Error('child task is already set on subtask'); }
      if (this.#parentTask === t) { throw new Error("parent cannot be child"); }
      this.#childTask = t;
    }
    getChildTask(t) { return this.#childTask; }
    
    getParentTask() { return this.#parentTask; }
    
    setCallbackFn(f, name) {
      if (!f) { return; }
      if (this.#callbackFn) { throw new Error('callback fn can only be set once'); }
      this.#callbackFn = f;
      this.#callbackFnName = name;
    }
    
    getCallbackFnName() {
      if (!this.#callbackFn) { return undefined; }
      return this.#callbackFn.name;
    }
    
    setPostReturnFn(f) {
      if (!f) { return; }
      if (this.#postReturnFn) { throw new Error('postReturn fn can only be set once'); }
      this.#postReturnFn = f;
    }
    
    setOnProgressFn(f) {
      if (this.#onProgressFn) { throw new Error('on progress fn can only be set once'); }
      this.#onProgressFn = f;
    }
    
    isNotStarted() {
      return this.#state == AsyncSubtask.State.STARTING;
    }
    
    registerOnStartHandler(f) {
      this.#onStartHandlers.push(f);
    }
    
    onStart(args) {
      _debugLog('[AsyncSubtask#onStart()] args', {
        componentIdx: this.#componentIdx,
        subtaskID: this.#id,
        parentTaskID: this.parentTaskID(),
        fnName: this.fnName,
      });
      
      if (this.#onProgressFn) { this.#onProgressFn(); }
      
      this.#state = AsyncSubtask.State.STARTED;
      
      let result;
      
      // If we have been provided a helper start function as a result of
      // component fusion performed by wasmtime tooling, then we can call that helper and lifts/lowers will
      // be performed for us.
      //
      // See also documentation on `HostIntrinsic::PrepareCall`
      //
      if (this.#callMetadata.startFn) {
        result = this.#callMetadata.startFn.apply(null, args?.startFnParams ?? []);
      }
      
      return result;
    }
    
    
    registerOnResolveHandler(f) {
      this.#onResolveHandlers.push(f);
    }
    
    reject(subtaskErr) {
      this.#childTask?.reject(subtaskErr);
    }
    
    onResolve(subtaskValue) {
      _debugLog('[AsyncSubtask#onResolve()] args', {
        componentIdx: this.#componentIdx,
        subtaskID: this.#id,
        isAsync: this.isAsync,
        childTaskID: this.childTaskID(),
        parentTaskID: this.parentTaskID(),
        parentTaskFnName: this.#parentTask?.entryFnName(),
        fnName: this.fnName,
      });
      
      if (this.#resolved) {
        throw new Error('subtask has already been resolved');
      }
      
      if (this.#onProgressFn) { this.#onProgressFn(); }
      
      if (subtaskValue === null) {
        if (this.#cancelRequested) {
          throw new Error('cancel was not requested, but no value present at return');
        }
        
        if (this.#state === AsyncSubtask.State.STARTING) {
          this.#state = AsyncSubtask.State.CANCELLED_BEFORE_STARTED;
        } else {
          if (this.#state !== AsyncSubtask.State.STARTED) {
            throw new Error('resolved subtask must have been started before cancellation');
          }
          this.#state = AsyncSubtask.State.CANCELLED_BEFORE_RETURNED;
        }
      } else {
        if (this.#state !== AsyncSubtask.State.STARTED) {
          throw new Error('resolved subtask must have been started before completion');
        }
        this.#state = AsyncSubtask.State.RETURNED;
      }
      
      this.setResult(subtaskValue);
      
      for (const f of this.#onResolveHandlers) {
        try {
          f(subtaskValue);
        } catch (err) {
          console.error("error during subtask resolve handler", err);
          throw err;
        }
      }
      
      const callMetadata = this.getCallMetadata();
      
      // TODO(fix): we should be able to easily have the caller's meomry
      // to lower into here, but it's not present in PrepareCall
      const memory = callMetadata.memory ?? this.#parentTask?.getReturnMemory() ?? lookupMemoriesForComponent({ componentIdx: this.#parentTask?.componentIdx() })[0];
      if (callMetadata && !callMetadata.returnFn && this.isAsync && callMetadata.resultPtr && memory) {
        const { resultPtr, realloc } = callMetadata;
        const lowers = callMetadata.lowers; // may have been updated in task.return of the child
        if (lowers && lowers.length > 0) {
          lowers[0]({
            componentIdx: this.#componentIdx,
            memory,
            realloc,
            vals: [subtaskValue],
            storagePtr: resultPtr,
          });
        }
      }
      
      this.#resolved = true;
      this.#parentTask.removeSubtask(this);
    }
    
    getStateNumber() { return this.#state; }
    isReturned() { return this.#state === AsyncSubtask.State.RETURNED; }
    
    getCallMetadata() { return this.#callMetadata; }
    
    isResolved() {
      if (this.#state === AsyncSubtask.State.STARTING
      || this.#state === AsyncSubtask.State.STARTED) {
        return false;
      }
      if (this.#state === AsyncSubtask.State.RETURNED
      || this.#state === AsyncSubtask.State.CANCELLED_BEFORE_STARTED
      || this.#state === AsyncSubtask.State.CANCELLED_BEFORE_RETURNED) {
        return true;
      }
      throw new Error('unrecognized internal Subtask state [' + this.#state + ']');
    }
    
    addLender(handle) {
      _debugLog('[AsyncSubtask#addLender()] args', { handle });
      if (!Number.isNumber(handle)) { throw new Error('missing/invalid lender handle [' + handle + ']'); }
      
      if (this.#lenders.length === 0 || this.isResolved()) {
        throw new Error('subtask has no lendors or has already been resolved');
      }
      
      handle.lends++;
      this.#lenders.push(handle);
    }
    
    deliverResolve() {
      _debugLog('[AsyncSubtask#deliverResolve()] args', {
        lenders: this.#lenders,
        parentTaskID: this.parentTaskID(),
        subtaskID: this.#id,
        childTaskID: this.childTaskID(),
        resolved: this.isResolved(),
        resolveDelivered: this.resolveDelivered(),
      });
      
      const cannotDeliverResolve = this.resolveDelivered() || !this.isResolved();
      if (cannotDeliverResolve) {
        throw new Error('subtask cannot deliver resolution twice, and the subtask must be resolved');
      }
      
      for (const lender of this.#lenders) {
        lender.lends--;
      }
      
      this.#lenders = null;
    }
    
    resolveDelivered() {
      _debugLog('[AsyncSubtask#resolveDelivered()] args', { });
      if (this.#lenders === null && !this.isResolved()) {
        throw new Error('invalid subtask state, lenders missing and subtask has not been resolved');
      }
      return this.#lenders === null;
    }
    
    drop() {
      _debugLog('[AsyncSubtask#drop()] args', {
        componentIdx: this.#componentIdx,
        parentTaskID: this.#parentTask?.id(),
        parentTaskFnName: this.#parentTask?.entryFnName(),
        childTaskID: this.#childTask?.id(),
        childTaskFnName: this.#childTask?.entryFnName(),
        subtaskFnName: this.fnName,
      });
      if (!this.#waitable) { throw new Error('missing/invalid inner waitable'); }
      if (!this.resolveDelivered()) {
        throw new Error('cannot drop subtask before resolve is delivered');
      }
      if (this.#waitable) { this.#waitable.drop() }
      this.#dropped = true;
    }
    
    #getComponentState() {
      const state = getOrCreateAsyncState(this.#componentIdx);
      if (!state) {
        throw new Error('invalid/missing async state for component [' + componentIdx + ']');
      }
      return state;
    }
    
    getWaitableHandleIdx() {
      _debugLog('[AsyncSubtask#getWaitableHandleIdx()] args', { });
      if (!this.#waitable) { throw new Error('missing/invalid waitable'); }
      return this.waitableRep();
    }
  }
  
  function _prepareCall(
  memoryIdx,
  getMemoryFn,
  startFn,
  returnFn,
  callerComponentIdx,
  calleeComponentIdx,
  taskReturnTypeIdx,
  calleeIsAsyncInt,
  stringEncoding,
  resultCountOrAsync,
  ) {
    _debugLog('[_prepareCall()]', {
      memoryIdx,
      callerComponentIdx,
      calleeComponentIdx,
      taskReturnTypeIdx,
      calleeIsAsyncInt,
      stringEncoding,
      resultCountOrAsync,
    });
    const argArray = [...arguments];
    
    // value passed in *may* be as large as u32::MAX which may be mangled into -2
    resultCountOrAsync >>>= 0;
    
    let isAsync = false;
    let hasResultPointer = false;
    if (resultCountOrAsync === 2**32 - 1) {
      // prepare async with no result (u32::MAX)
      isAsync = true;
      hasResultPointer = false;
    } else if (resultCountOrAsync === 2**32 - 2) {
      // prepare async with result (u32::MAX - 1)
      isAsync = true;
      hasResultPointer = true;
    }
    
    const currentCallerTaskMeta = getCurrentTask(callerComponentIdx);
    if (!currentCallerTaskMeta) {
      throw new Error('invalid/missing current task for caller during prepare call');
    }
    
    const currentCallerTask = currentCallerTaskMeta.task;
    if (!currentCallerTask) {
      throw new Error('unexpectedly missing task in meta for caller during prepare call');
    }
    
    if (currentCallerTask.componentIdx() !== callerComponentIdx) {
      throw new Error(`task component idx [${ currentCallerTask.componentIdx() }] !== [${ callerComponentIdx }] (callee ${ calleeComponentIdx })`);
    }
    
    let getCalleeParamsFn;
    let resultPtr = null;
    let directParamsArr;
    if (hasResultPointer) {
      directParamsArr = argArray.slice(10, argArray.length - 1);
      getCalleeParamsFn = () => directParamsArr;
      resultPtr = argArray[argArray.length - 1];
    } else {
      directParamsArr = argArray.slice(10);
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
    
    const subtask = currentCallerTask.createSubtask({
      componentIdx: callerComponentIdx,
      parentTask: currentCallerTask,
      isAsync,
      callMetadata: {
        getMemoryFn,
        memoryIdx,
        resultPtr,
        returnFn,
        startFn,
      }
    });
    
    const [newTask, newTaskID] = createNewCurrentTask({
      componentIdx: calleeComponentIdx,
      isAsync,
      getCalleeParamsFn,
      entryFnName: [
      'task',
      subtask.getParentTask().id(),
      'subtask',
      subtask.id(),
      'new-prepared-async-task'
      ].join('/'),
      stringEncoding,
    });
    newTask.setParentSubtask(subtask);
    newTask.setReturnMemoryIdx(memoryIdx);
    newTask.setReturnMemory(getMemoryFn);
    subtask.setChildTask(newTask);
    
    newTask.subtaskMeta = {
      subtask,
      calleeComponentIdx,
      callerComponentIdx,
      getCalleeParamsFn,
      stringEncoding,
      isAsync,
    };
    
    _setGlobalCurrentTaskMeta({
      taskID: newTask.id(),
      componentIdx: newTask.componentIdx(),
    });
  }
  
  function _asyncStartCall(args, callee, paramCount, resultCount, flags) {
    const componentIdx = ASYNC_CURRENT_COMPONENT_IDXS.at(-1);
    
    const globalTaskMeta = _getGlobalCurrentTaskMeta(componentIdx);
    if (!globalTaskMeta) { throw new Error('missing global current task globalTaskMeta'); }
    const taskID = globalTaskMeta.taskID;
    
    _debugLog('[_asyncStartCall()] args', { args, componentIdx });
    const { getCallbackFn, callbackIdx, getPostReturnFn, postReturnIdx } = args;
    
    const preparedTaskMeta = getCurrentTask(componentIdx, taskID);
    if (!preparedTaskMeta) { throw new Error('unexpectedly missing current task'); }
    
    const preparedTask = preparedTaskMeta.task;
    if (!preparedTask) { throw new Error('unexpectedly missing current task'); }
    if (!preparedTask.subtaskMeta) { throw new Error('missing subtask meta from prepare'); }
    
    const {
      subtask,
      returnMemoryIdx,
      getReturnMemoryFn,
      callerComponentIdx,
      calleeComponentIdx,
      getCalleeParamsFn,
      isAsync,
      stringEncoding,
    } = preparedTask.subtaskMeta;
    if (!subtask) { throw new Error("missing subtask from cstate during async start call"); }
    if (calleeComponentIdx !== preparedTask.componentIdx()) {
      throw new Error(`meta callee idx [${calleeComponentIdx}] != current task idx [${preparedTask.componentIdx()}] during async start call`);
    }
    if (calleeComponentIdx !== componentIdx) {
      throw new Error("mismatched componentIdx for async start call (does not match prepare)");
    }
    
    const argArray = [...arguments];
    
    if (resultCount < 0 || resultCount > 1) { throw new Error('invalid/unsupported result count'); }
    
    const callbackFnName = 'callback_' + callbackIdx;
    const callbackFn = getCallbackFn();
    preparedTask.setCallbackFn(callbackFn, callbackFnName);
    preparedTask.setPostReturnFn(getPostReturnFn());
    
    if (resultCount < 0 || resultCount > 1) {
      throw new Error(`unsupported result count [${ resultCount }]`);
    }
    
    const params = preparedTask.getCalleeParams();
    if (paramCount !== params.length) {
      throw new Error(`unexpected callee param count [${ params.length }], _asyncStartCall invocation expected [${ paramCount }]`);
    }
    
    const callerComponentState = getOrCreateAsyncState(subtask.componentIdx());
    
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
      if (subtaskCallMeta && subtaskCallMeta.returnFn) {
        _debugLog('[_asyncStartCall()] return function present while handling subtask result, returning early (skipping lower)');
        
        // TODO: centralize calling of returnFn to *one place* (if possible)
        if (subtaskCallMeta.returnFnCalled) { return; }
        
        subtaskCallMeta.returnFn.apply(null, [subtaskCallMeta.resultPtr]);
        return;
      }
      
      // If there is no where to lower the results, exit early
      if (!subtaskCallMeta.resultPtr) {
        _debugLog('[_asyncStartCall()] no result ptr during subtask result handling, returning early (skipping lower)');
        return;
      }
      
      let callerMemory;
      if (callerMemoryIdx !== null && callerMemoryIdx !== undefined) {
        callerMemory = lookupMemoriesForComponent({ componentIdx: callerComponentIdx, memoryIdx: callerMemoryIdx });
      } else {
        const callerMemories = lookupMemoriesForComponent({ componentIdx: callerComponentIdx });
        if (callerMemories.length !== 1) { throw new Error(`unsupported amount of caller memories`); }
        callerMemory = callerMemories[0];
      }
      
      if (!callerMemory) {
        _debugLog('[_asyncStartCall()] missing memory', { subtaskID: subtask.id(), res });
        throw new Error(`missing memory for to guest->guest call result (subtask [${subtask.id()}])`);
      }
      
      const lowerFns = calleeTask.getReturnLowerFns();
      if (!lowerFns || lowerFns.length === 0) {
        _debugLog('[_asyncStartCall()] missing result lower metadata for guest->guest call', { subtaskID: subtask.id() });
        throw new Error(`missing result lower metadata for guest->guest call (subtask [${subtask.id()}])`);
      }
      
      if (lowerFns.length !== 1) {
        _debugLog('[_asyncStartCall()] only single result reportetd for guest->guest call', { subtaskID: subtask.id() });
        throw new Error(`only single result supported for guest->guest calls (subtask [${subtask.id()}])`);
      }
      
      _debugLog('[_asyncStartCall()] lowering results', { subtaskID: subtask.id() });
      lowerFns[0]({
        realloc: undefined,
        memory: callerMemory,
        vals: [res],
        storagePtr: subtaskCallMeta.resultPtr,
        componentIdx: callerComponentIdx
      });
      
    });
    
    subtask.setOnProgressFn(() => {
      subtask.setPendingEvent(() => {
        if (subtask.isResolved()) { subtask.deliverResolve(); }
        const event = {
          code: ASYNC_EVENT_CODE.SUBTASK,
          payload0: subtask.waitableRep(),
          payload1: subtask.getStateNumber(),
        };
        return event;
      });
    });
    
    // Start the (event) driver loop that will resolve the task
    queueMicrotask(async () => {
      let startRes = subtask.onStart({ startFnParams: params });
      startRes = Array.isArray(startRes) ? startRes : [startRes];
      
      await calleeComponentState.suspendTask({
        task: preparedTask,
        readyFn: () => !calleeComponentState.isExclusivelyLocked(),
      });
      
      const started = await preparedTask.enter();
      if (!started) {
        _debugLog('[_asyncStartCall()] task failed early', {
          taskID: preparedTask.id(),
          subtaskID: subtask.id(),
        });
        throw new Error("task failed to start");
        return;
      }
      
      let callbackResult;
      try {
        let jspiCallee = WebAssembly.promising(callee);
        callbackResult = await _withGlobalCurrentTaskMetaAsync({
          taskID: preparedTask.id(),
          componentIdx: preparedTask.componentIdx(),
          fn: () => {
            return jspiCallee.apply(null, startRes);
          }
        });
      } catch(err) {
        _debugLog("[_asyncStartCall()] initial subtask callee run failed", err);
        // NOTE: a good place to rejectt the parent task, if rejection API is enabled
        // subtask.reject(err);
        // subtask.getParentTask().reject(err);
        
        subtask.getParentTask().setErrored(err);
        
        return;
      }
      
      // If there was no callback function, we're dealing with a sync function
      // that was lifted as async without one, there is only the callee.
      if (!callbackFn) {
        _debugLog("[_asyncStartCall()] no callback, resolving w/ callee result", {
          taskID: preparedTask.id(),
          componentIdx: preparedTask.componentIdx(),
          preparedTask,
          stateNumber: preparedTask.taskState(),
          isResolved: preparedTask.isResolved(),
          callbackFn,
        });
        preparedTask.resolve([callbackResult]);
        return;
      }
      
      let fnName = callbackFn.fnName;
      if (!fnName) {
        fnName = [
        '<task ',
        subtask.parentTaskID(),
        '/subtask ',
        subtask.id(),
        '/task ',
        preparedTask.id(),
        '>',
        ].join("");
      }
      
      try {
        _debugLog("[_asyncStartCall()] starting driver loop", {
          fnName,
          componentIdx: preparedTask.componentIdx(),
          subtaskID: subtask.id(),
          childTaskID: subtask.childTaskID(),
          parentTaskID: subtask.parentTaskID(),
        });
        
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
    
    const subtaskState = subtask.getStateNumber();
    if (subtaskState < 0 || subtaskState > 2**5) {
      throw new Error('invalid subtask state, out of valid range');
    }
    
    _debugLog('[_asyncStartCall()] returning subtask rep & state', {
      subtask: {
        rep: subtask.waitableRep(),
        state: subtaskState,
      }
    });
    
    return Number(subtask.waitableRep()) << 4 | subtaskState;
  }
  
  function _syncStartCall(callbackIdx) {
    _debugLog('[_syncStartCall()] args', { callbackIdx });
    throw new Error('synchronous start call not implemented!');
  }
  
  class Waitable {
    #componentIdx;
    
    #pendingEventFn = null;
    
    #promise;
    #resolve;
    #reject;
    
    #waitableSet = null;
    
    #idx = null; // to component-global waitables
    
    target;
    
    constructor(args) {
      const { componentIdx, target } = args;
      this.#componentIdx = componentIdx;
      this.target = args.target;
      this.#resetPromise();
    }
    
    componentIdx() { return this.#componentIdx; }
    isInSet() { return this.#waitableSet !== null; }
    
    idx() { return this.#idx; }
    setIdx(idx) {
      if (idx === 0) { throw new Error("waitable idx cannot be zero"); }
      this.#idx = idx;
    }
    
    setTarget(tgt) { this.target = tgt; }
    
    #resetPromise() {
      const { promise, resolve, reject } = promiseWithResolvers()
      this.#promise = promise;
      this.#resolve = resolve;
      this.#reject = reject;
    }
    
    resolve() { this.#resolve(); }
    reject(err) { this.#reject(err); }
    promise() { return this.#promise; }
    
    hasPendingEvent() {
      // _debugLog('[Waitable#hasPendingEvent()]', {
        //     componentIdx: this.#componentIdx,
        //     waitable: this,
        //     waitableSet: this.#waitableSet,
        //     hasPendingEvent: this.#pendingEventFn !== null,
        // });
        return this.#pendingEventFn !== null;
      }
      
      setPendingEvent(fn) {
        _debugLog('[Waitable#setPendingEvent()] args', {
          waitable: this,
          inSet: this.#waitableSet,
        });
        this.#pendingEventFn = fn;
      }
      
      getPendingEvent() {
        _debugLog('[Waitable#getPendingEvent()] args', {
          waitable: this,
          inSet: this.#waitableSet,
          hasPendingEvent: this.#pendingEventFn !== null,
        });
        if (this.#pendingEventFn === null) { return null; }
        const eventFn = this.#pendingEventFn;
        this.#pendingEventFn = null;
        const e = eventFn();
        this.#resetPromise();
        return e;
      }
      
      join(waitableSet) {
        _debugLog('[Waitable#join()] args', {
          waitable: this,
          waitableSet: waitableSet,
        });
        if (this.#waitableSet) { this.#waitableSet.removeWaitable(this); }
        if (!waitableSet) {
          this.#waitableSet = null;
          return;
        }
        waitableSet.addWaitable(this);
        this.#waitableSet = waitableSet;
      }
      
      drop() {
        _debugLog('[Waitable#drop()] args', {
          componentIdx: this.#componentIdx,
          waitable: this,
        });
        if (this.hasPendingEvent()) {
          throw new Error('waitables with pending events cannot be dropped');
        }
        this.join(null);
      }
      
    }
    
    const ERR_CTX_TABLES = {};
    
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
      const res = { ptr, len: buf.length, codepoints: [...s].length };
      return res;
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
    
    function getCurrentTask(componentIdx, taskID) {
      let usedGlobal = false;
      if (componentIdx === undefined || componentIdx === null) {
        throw new Error('missing component idx'); // TODO(fix)
        // componentIdx = ASYNC_CURRENT_COMPONENT_IDXS.at(-1);
        // usedGlobal = true;
      }
      
      const taskMetas = ASYNC_TASKS_BY_COMPONENT_IDX.get(componentIdx);
      if (taskMetas === undefined || taskMetas.length === 0) { return undefined; }
      
      if (taskID) {
        return taskMetas.find(meta => meta.task.id() === taskID);
      }
      
      const taskMeta = taskMetas[taskMetas.length - 1];
      if (!taskMeta || !taskMeta.task) { return undefined; }
      
      return taskMeta;
    }
    
    function createNewCurrentTask(args) {
      _debugLog('[createNewCurrentTask()] args', args);
      const {
        componentIdx,
        isAsync,
        isManualAsync,
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
      let taskMetas = ASYNC_TASKS_BY_COMPONENT_IDX.get(componentIdx);
      const callbackFn = getCallbackFn ? getCallbackFn() : null;
      
      const newTask = new AsyncTask({
        componentIdx,
        isAsync,
        isManualAsync,
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
      
      // NOTE: do not track host tasks
      ASYNC_CURRENT_TASK_IDS.push(newTaskID);
      ASYNC_CURRENT_COMPONENT_IDXS.push(componentIdx);
      
      if (!taskMetas) {
        taskMetas = [newTaskMeta];
        ASYNC_TASKS_BY_COMPONENT_IDX.set(componentIdx, [newTaskMeta]);
      } else {
        taskMetas.push(newTaskMeta);
      }
      
      return [newTask, newTaskID];
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
      #isManualAsync;
      #entryFnName = null;
      
      #onResolveHandlers = [];
      #completionPromise = null;
      #rejected = false;
      
      #exitPromise = null;
      #onExitHandlers = [];
      
      #memoryIdx = null;
      #memory = null;
      
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
      
      #subtasks = [];
      
      #entered = false;
      #exited = false;
      #errored = null;
      
      cancelled = false;
      cancelRequested = false;
      alwaysTaskReturn = false;
      
      returnCalls =  0;
      storage = [0, 0];
      borrowedHandles = {};
      
      constructor(opts) {
        this.#id = ++AsyncTask._ID;
        
        if (opts?.componentIdx === undefined) {
          throw new TypeError('missing component id during task creation');
        }
        this.#componentIdx = opts.componentIdx;
        
        this.#state = AsyncTask.State.INITIAL;
        this.#isAsync = opts?.isAsync ?? false;
        this.#isManualAsync = opts?.isManualAsync ?? false;
        this.#entryFnName = opts.entryFnName;
        
        const {
          promise: completionPromise,
          resolve: resolveCompletionPromise,
          reject: rejectCompletionPromise,
        } = promiseWithResolvers();
        this.#completionPromise = completionPromise;
        
        this.#onResolveHandlers.push((results) => {
          if (this.#errored !== null) {
            rejectCompletionPromise(this.#errored);
            return;
          } else if (this.#rejected) {
            rejectCompletionPromise(results);
            return;
          }
          resolveCompletionPromise(results);
        });
        
        const {
          promise: exitPromise,
          resolve: resolveExitPromise,
          reject: rejectExitPromise,
        } = promiseWithResolvers();
        this.#exitPromise = exitPromise;
        
        this.#onExitHandlers.push(() => {
          resolveExitPromise();
        });
        
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
      entryFnName() { return this.#entryFnName; }
      
      completionPromise() { return this.#completionPromise; }
      exitPromise() { return this.#exitPromise; }
      
      isAsync() { return this.#isAsync; }
      isSync() { return !this.isAsync(); }
      
      getErrHandling() { return this.#errHandling; }
      
      hasCallback() { return this.#callbackFn !== null; }
      
      getReturnMemoryIdx() { return this.#memoryIdx; }
      setReturnMemoryIdx(idx) {
        if (idx === null) { return; }
        this.#memoryIdx = idx;
      }
      
      getReturnMemory() { return this.#memory; }
      setReturnMemory(m) {
        if (m === null) { return; }
        this.#memory = m;
      }
      
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
      
      async runCallbackFn(...args) {
        if (!this.#callbackFn) { throw new Error('on callback function has been set for task'); }
        return await this.#callbackFn.apply(null, args);
      }
      
      getCalleeParams() {
        if (!this.#getCalleeParamsFn) { throw new Error('missing/invalid getCalleeParamsFn'); }
        return this.#getCalleeParamsFn();
      }
      
      mayBlock() { return this.isAsync() || this.isResolvedState() }
      
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
      
      enterSync() {
        if (this.needsExclusiveLock()) {
          const cstate = getOrCreateAsyncState(this.#componentIdx);
          cstate.exclusiveLock();
        }
        return true;
      }
      
      async enter(opts) {
        _debugLog('[AsyncTask#enter()] args', {
          taskID: this.#id,
          componentIdx: this.#componentIdx,
          subtaskID: this.getParentSubtask()?.id(),
        });
        
        if (this.#entered) {
          throw new Error(`task with ID [${this.#id}] should not be entered twice`);
        }
        
        const cstate = getOrCreateAsyncState(this.#componentIdx);
        
        // If a task is either synchronous or host-provided (e.g. a host import, whether sync or async)
        // then we can avoid component-relevant tracking and immediately enter
        if (this.isSync() || opts?.isHost) {
          this.#entered = true;
          
          // TODO(breaking): remove once manually-spccifying async fns is removed
          // It is currently possible for an actually sync export to be specified
          // as async via JSPI
          if (this.#isManualAsync) {
            if (this.needsExclusiveLock()) { cstate.exclusiveLock(); }
          }
          
          return this.#entered;
        }
        
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
        
        this.#entered = true;
        return this.#entered;
      }
      
      isRunningState() { return this.#state !== AsyncTask.State.RESOLVED; }
      isResolvedState() { return this.#state === AsyncTask.State.RESOLVED; }
      isResolved() { return this.#state === AsyncTask.State.RESOLVED; }
      
      async waitUntil(opts) {
        const { readyFn, waitableSetRep, cancellable } = opts;
        _debugLog('[AsyncTask#waitUntil()] args', { taskID: this.#id, waitableSetRep, cancellable });
        
        const state = getOrCreateAsyncState(this.#componentIdx);
        const wset = state.handles.get(waitableSetRep);
        
        let event;
        
        wset.incrementNumWaiting();
        
        const keepGoing = await this.suspendUntil({
          readyFn: () => {
            const hasPendingEvent = wset.hasPendingEvent();
            const ready = readyFn();
            return ready && hasPendingEvent;
          },
          cancellable,
        });
        
        if (keepGoing) {
          event = wset.getPendingEvent();
        } else {
          event = {
            code: ASYNC_EVENT_CODE.TASK_CANCELLED,
            payload0: 0,
            payload1: 0,
          };
        }
        
        wset.decrementNumWaiting();
        
        return event;
      }
      
      async yieldUntil(opts) {
        const { readyFn, cancellable } = opts;
        _debugLog('[AsyncTask#yieldUntil()] args', { taskID: this.#id, cancellable });
        
        const keepGoing = await this.suspendUntil({ readyFn, cancellable });
        if (keepGoing) {
          return {
            code: ASYNC_EVENT_CODE.NONE,
            payload0: 0,
            payload1: 0,
          };
        }
        
        return {
          code: ASYNC_EVENT_CODE.TASK_CANCELLED,
          payload0: 0,
          payload1: 0,
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
        if (ready && ASYNC_DETERMINISM === 'random') {
          const coinFlip = _coinFlip();
          if (coinFlip) { return true }
        }
        
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
      const keepGoing = await cstate.suspendTask({ task: this, readyFn });
      return keepGoing;
    }
    
    deliverPendingCancel(opts) {
      const { cancellable } = opts;
      _debugLog('[AsyncTask#deliverPendingCancel()] args', { cancellable });
      
      if (cancellable && this.#state === AsyncTask.State.PENDING_CANCEL) {
        this.#state = AsyncTask.State.CANCEL_DELIVERED;
        return true;
      }
      
      return false;
    }
    
    isCancelled() { return this.cancelled }
    
    cancel(args) {
      _debugLog('[AsyncTask#cancel()] args', { });
      if (this.taskState() !== AsyncTask.State.CANCEL_DELIVERED) {
        throw new Error(`(component [${this.#componentIdx}]) task [${this.#id}] invalid task state [${this.taskState()}] for cancellation`);
      }
      if (this.borrowedHandles.length > 0) { throw new Error('task still has borrow handles'); }
      this.cancelled = true;
      this.onResolve(args?.error ?? new Error('task cancelled'));
      this.#state = AsyncTask.State.RESOLVED;
    }
    
    onResolve(taskValue) {
      const handlers = this.#onResolveHandlers;
      this.#onResolveHandlers = [];
      for (const f of handlers) {
        try {
          // TODO(fix): resolve handlers getting called a ton?
          f(taskValue);
        } catch (err) {
          _debugLog("[AsyncTask#onResolve] error during task resolve handler", err);
          throw err;
        }
      }
      
      if (this.#parentSubtask) {
        const meta = this.#parentSubtask.getCallMetadata();
        // Run the rturn fn if it has not already been called -- this *should* have happened in
        // `task.return`, but some paths do not go through task.return (e.g. async lower of sync fn
        // which goes through prepare + async-start-call)
        if (meta.returnFn && !meta.returnFnCalled) {
          _debugLog('[AsyncTask#onResolve()] running returnFn', {
            componentIdx: this.#componentIdx,
            taskID: this.#id,
            subtaskID: this.#parentSubtask.id(),
          });
          const memory = meta.getMemoryFn();
          meta.returnFn.apply(null, [taskValue, meta.resultPtr]);
          meta.returnFnCalled = true;
        }
      }
      
      if (this.#postReturnFn) {
        _debugLog('[AsyncTask#onResolve()] running post return ', {
          componentIdx: this.#componentIdx,
          taskID: this.#id,
        });
        try {
          this.#postReturnFn(taskValue);
        } catch (err) {
          _debugLog("[AsyncTask#onResolve] error during task resolve handler", err);
          throw err;
        }
      }
      
      if (this.#parentSubtask) {
        this.#parentSubtask.onResolve(taskValue);
      }
    }
    
    registerOnResolveHandler(f) {
      this.#onResolveHandlers.push(f);
    }
    
    isRejected() { return this.#rejected; }
    
    setErrored(err) {
      this.#errored = err;
    }
    
    reject(taskErr) {
      _debugLog('[AsyncTask#reject()] args', {
        componentIdx: this.#componentIdx,
        taskID: this.#id,
        parentSubtask: this.#parentSubtask,
        parentSubtaskID: this.#parentSubtask?.id(),
        entryFnName: this.entryFnName(),
        callbackFnName: this.#callbackFnName,
        errMsg: taskErr.message,
      });
      
      if (this.isResolvedState() || this.#rejected) { return; }
      
      for (const subtask of this.#subtasks) {
        subtask.reject(taskErr);
      }
      
      this.#rejected = true;
      this.cancelRequested = true;
      this.#state = AsyncTask.State.PENDING_CANCEL;
      const cancelled = this.deliverPendingCancel({ cancellable: true });
      
      // TODO: do cleanup here to reset the machinery so we can run again?
      
      
      this.cancel({ error: taskErr });
    }
    
    resolve(results) {
      _debugLog('[AsyncTask#resolve()] args', {
        componentIdx: this.#componentIdx,
        taskID: this.#id,
        entryFnName: this.entryFnName(),
        callbackFnName: this.#callbackFnName,
      });
      
      if (this.#state === AsyncTask.State.RESOLVED) {
        throw new Error(`(component [${this.#componentIdx}]) task [${this.#id}]  is already resolved (did you forget to wait for an import?)`);
      }
      
      if (this.borrowedHandles.length > 0) {
        throw new Error('task still has borrow handles');
      }
      
      this.#state = AsyncTask.State.RESOLVED;
      
      switch (results.length) {
        case 0:
        this.onResolve(undefined);
        break;
        case 1:
        this.onResolve(results[0]);
        break;
        default:
        _debugLog('[AsyncTask#resolve()] unexpected number of results', {
          componentIdx: this.#componentIdx,
          results,
          taskID: this.#id,
          subtaskID: this.#parentSubtask?.id(),
          entryFnName: this.#entryFnName,
          callbackFnName: this.#callbackFnName,
        });
        throw new Error('unexpected number of results');
      }
    }
    
    exit() {
      _debugLog('[AsyncTask#exit()]', {
        componentIdx: this.#componentIdx,
        taskID: this.#id,
      });
      
      if (this.#exited)  { throw new Error("task has already exited"); }
      
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
      
      // Exempt the host from exclusive lock check
      if (this.#componentIdx !== -1 && this.needsExclusiveLock() && !state.isExclusivelyLocked()) {
        throw new Error(`task [${this.#id}] exit: component [${this.#componentIdx}] should have been exclusively locked`);
      }
      
      state.exclusiveRelease();
      
      for (const f of this.#onExitHandlers) {
        try {
          f();
        } catch (err) {
          console.error("error during task exit handler", err);
          throw err;
        }
      }
      
      this.#exited = true;
      clearCurrentTask(this.#componentIdx, this.id());
    }
    
    needsExclusiveLock() {
      return !this.#isAsync || this.hasCallback();
    }
    
    createSubtask(args) {
      _debugLog('[AsyncTask#createSubtask()] args', args);
      const { componentIdx, childTask, callMetadata, fnName, isAsync, isManualAsync } = args;
      
      const cstate = getOrCreateAsyncState(this.#componentIdx);
      if (!cstate) {
        throw new Error(`invalid/missing async state for component idx [${componentIdx}]`);
      }
      
      const waitable = new Waitable({
        componentIdx: this.#componentIdx,
        target: `subtask (internal ID [${this.#id}])`,
      });
      
      const newSubtask = new AsyncSubtask({
        componentIdx,
        childTask,
        parentTask: this,
        callMetadata,
        isAsync,
        isManualAsync,
        fnName,
        waitable,
      });
      this.#subtasks.push(newSubtask);
      newSubtask.setTarget(`subtask (internal ID [${newSubtask.id()}], waitable [${waitable.idx()}], component [${componentIdx}])`);
      waitable.setIdx(cstate.handles.insert(newSubtask));
      waitable.setTarget(`waitable for subtask (waitable id [${waitable.idx()}], subtask internal ID [${newSubtask.id()}])`);
      
      return newSubtask;
    }
    
    getLatestSubtask() {
      return this.#subtasks.at(-1);
    }
    
    getSubtaskByWaitableRep(rep) {
      if (rep === undefined) { throw new TypeError('missing rep'); }
      return this.#subtasks.find(s => s.waitableRep() === rep);
    }
    
    currentSubtask() {
      _debugLog('[AsyncTask#currentSubtask()]');
      if (this.#subtasks.length === 0) { return undefined; }
      return this.#subtasks.at(-1);
    }
    
    removeSubtask(subtask) {
      if (this.#subtasks.length === 0) { throw new Error('cannot end current subtask: no current subtask'); }
      this.#subtasks = this.#subtasks.filter(t => t !== subtask);
      return subtask;
    }
  }
  
  function _lowerImportBackwardsCompat(args) {
    const params = [...arguments].slice(1);
    _debugLog('[_lowerImportBackwardsCompat()] args', { args, params });
    const {
      functionIdx,
      componentIdx,
      isAsync,
      isManualAsync,
      paramLiftFns,
      resultLowerFns,
      funcTypeIsAsync,
      metadata,
      memoryIdx,
      getMemoryFn,
      getReallocFn,
      importFn,
    } = args;
    
    const { taskID } = _getGlobalCurrentTaskMeta(componentIdx);
    
    const taskMeta = getCurrentTask(componentIdx, taskID);
    if (!taskMeta) { throw new Error('invalid/missing async task meta'); }
    
    const task = taskMeta.task;
    if (!task) { throw new Error('invalid/missing async task'); }
    
    const cstate = getOrCreateAsyncState(componentIdx);
    
    // TODO: re-enable this check -- postReturn can call imports though,
    // and that breaks things.
    //
    // if (!cstate.mayLeave) {
      //     throw new Error(`cannot leave instance [${componentIdx}]`);
      // }
      
      if (!task.mayBlock() && funcTypeIsAsync && !isAsync) {
        throw new Error("non async exports cannot synchronously call async functions");
      }
      
      // If there is an existing task, this should be part of a subtask
      const memory = getMemoryFn();
      const subtask = task.createSubtask({
        componentIdx,
        parentTask: task,
        fnName: importFn.fnName,
        isAsync,
        isManualAsync,
        callMetadata: {
          memoryIdx,
          memory,
          realloc: getReallocFn(),
          resultPtr: params[0],
          lowers: resultLowerFns,
        }
      });
      task.setReturnMemoryIdx(memoryIdx);
      task.setReturnMemory(getMemoryFn());
      
      subtask.onStart();
      
      // If dealing with a sync lowered sync function, we can directly return results
      //
      // TODO(breaking): remove once we get rid of manual async import specification,
      // as func types cannot be detected in that case only (and we don't need that w/ p3)
      if (!isManualAsync && !isAsync && !funcTypeIsAsync) {
        const res = importFn(...params);
        // TODO(breaking): remove once we get rid of manual async import specification,
        // as func types cannot be detected in that case only (and we don't need that w/ p3)
        if (!funcTypeIsAsync && !subtask.isReturned()) {
          throw new Error('post-execution subtasks must either be async or returned');
        }
        return subtask.getResult();
      }
      
      // Sync-lowered async functions requires async behavior because the callee *can* block,
      // but this call must *act* synchronously and return immediately with the result
      // (i.e. not returning until the work is done)
      //
      // TODO(breaking): remove checking for manual async specification here, once we can go p3-only
      //
      if (!isManualAsync && !isAsync && funcTypeIsAsync) {
        const { promise, resolve } = new Promise();
        queueMicrotask(async () => {
          if (!subtask.isResolvedState()) {
            await task.suspendUntil({ readyFn: () => task.isResolvedState() });
          }
          resolve(subtask.getResult());
        });
        return promise;
      }
      
      // NOTE: at this point we know that we are working with an async lowered import
      
      const subtaskState = subtask.getStateNumber();
      if (subtaskState < 0 || subtaskState > 2**5) {
        throw new Error('invalid subtask state, out of valid range');
      }
      
      subtask.setOnProgressFn(() => {
        subtask.setPendingEvent(() => {
          if (subtask.isResolved()) { subtask.deliverResolve(); }
          const event = {
            code: ASYNC_EVENT_CODE.SUBTASK,
            payload0: subtask.waitableRep(),
            payload1: subtask.getStateNumber(),
          }
          return event;
        });
      });
      
      // This is a hack to maintain backwards compatibility with
      // manually-specified async imports, used in wasm exports that are
      // not actually async (but are specified as so).
      //
      // This is not normal p3 sync behavior but instead anticipating that
      // the caller that is doing manual async will be waiting for a promise that
      // resolves to the *actual* result.
      //
      // TODO(breaking): remove once manually specified async is removed
      //
      // There are a few cases:
      // 1. sync function with async types (e.g. `f: func() -> stream<u32>`)
      // 2. async function with async types (e.g. `f: async func() -> stream<u32>`)
      // 3. async function with sync types (e.g. `f: async func() -> list<u32>`)
      // 4. sync function with non-async types (e.g. `f: func() -> list<u32>`)
      //
      // This hack *only* applies to 4 -- the case where an async JS host function
      // is supplied to a Wasm export which does *not* need to do any async abi
      // lifting/lowering (async ABI did not exist when JSPI integratiton was
      // initially merged to enable asynchronously returning values from the host)
      //
      const requiresManualAsyncResult = !isAsync && !funcTypeIsAsync && isManualAsync;
      let manualAsyncResult;
      if (requiresManualAsyncResult) {
        manualAsyncResult = promiseWithResolvers();
      }
      
      queueMicrotask(async () => {
        try {
          _debugLog('[_lowerImportBackwardsCompat()] calling lowered import', { importFn, params });
          const res = await importFn(...params);
          if (requiresManualAsyncResult) {
            manualAsyncResult.resolve(subtask.getResult());
          }
        } catch (err) {
          _debugLog("[_lowerImportBackwardsCompat()] import fn error:", err);
          if (requiresManualAsyncResult) {
            manualAsyncResult.reject(err);
          }
          throw err;
        }
      });
      
      if (requiresManualAsyncResult) { return manualAsyncResult.promise; }
      
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
      
      const rem = ctx.storagePtr % 2;
      if (rem !== 0) { ctx.storagePtr += (2 - rem); }
      
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
      val = new DataView(ctx.memory.buffer).getBigUint64(ctx.storagePtr, true);
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
      
      const start = new DataView(ctx.memory.buffer).getUint32(ctx.storagePtr, true);
      const codeUnits = new DataView(ctx.memory.buffer).getUint32(ctx.storagePtr + 4, true);
      val = TEXT_DECODER_UTF8.decode(new Uint8Array(ctx.memory.buffer, start, codeUnits));
      
      ctx.storagePtr += 8;
      
      const rem = ctx.storagePtr % 4;
      if (rem !== 0) { ctx.storagePtr += (4 - rem); }
      
      return [val, ctx];
    }
    
    function _liftFlatVariant(casesAndLiftFns) {
      return function _liftFlatVariantInner(ctx) {
        _debugLog('[_liftFlatVariant()] args', { ctx });
        
        const origUseParams = ctx.useDirectParams;
        
        let caseIdx;
        let liftRes;
        const originalPtr = ctx.storagePtr;
        const numCases =  casesAndLiftFns.length;
        if (casesAndLiftFns.length < 256) {
          liftRes = _liftFlatU8(ctx);
        } else if (numCases >= 256 && numCases < 65536) {
          liftRes = _liftFlatU16(ctx);
        } else if (numCases >= 65536 && numCases < 4_294_967_296) {
          liftRes = _liftFlatU32(ctx);
        } else {
          throw new Error(`unsupported number of variant cases [${numCases}]`);
        }
        caseIdx = liftRes[0];
        ctx = liftRes[1];
        
        const [ tag, liftFn, size32, align32, payloadOffset32 ] = casesAndLiftFns[caseIdx];
        if (payloadOffset32 === undefined) { throw new Error('unexpectedly missing payload offset'); }
        
        if (originalPtr !== undefined) {
          ctx.storagePtr = originalPtr + payloadOffset32;
        }
        
        let val;
        if (liftFn === null) {
          val = { tag };
          // NOTE: here we need to move past the entire object in memory
          // despite moving to the payload which we now know is missing/unnecessary
          ctx.storagePtr = originalPtr + size32;
        } else {
          const [newVal, newCtx] = liftFn(ctx);
          val = { tag, val: newVal };
          ctx = newCtx;
          
          // NOTE: Padding can be left over after doing the lift if it was less than
          // space left for the payload normally.
          if (ctx.storagePtr < originalPtr + size32) {
            ctx.storagePtr = originalPtr + size32;
          }
        }
        
        const rem = ctx.storagePtr % align32;
        if (rem !== 0) { ctx.storagePtr += align32 - rem; }
        
        return [val, ctx];
      }
    }
    
    function _liftFlatList(elemLiftFn, align32, knownLen) {
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
          const [res, nextCtx] = elemLiftFn(ctx);
          val.push(res);
          ctx = nextCtx;
          
          const rem = ctx.storagePtr % align32;
          if (rem !== 0) { newCtx.storagePtr += (align32 - rem); }
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
      
      // TODO: ALIGNMENT IS WRONG?
      
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
    
    function _lowerFlatStringUTF8(ctx) {
      _debugLog('[_lowerFlatStringUTF8()] args', ctx);
      
      const { memory, realloc, vals, storagePtr, storageLen } = ctx;
      
      const s = vals[0];
      const { ptr, len, codepoints } = _utf8AllocateAndEncode(vals[0], realloc, memory);
      
      const view = new DataView(memory.buffer);
      view.setUint32(storagePtr, ptr, true);
      view.setUint32(storagePtr + 4, codepoints, true);
      
      return len;
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
    
    function _lowerFlatVariant(lowerMetas) {
      return function _lowerFlatVariantInner(ctx) {
        _debugLog('[_lowerFlatVariant()] args', ctx);
        
        const { memory, realloc, vals, storageLen, componentIdx } = ctx;
        let storagePtr = ctx.storagePtr;
        
        const { tag, val } = vals[0];
        const disc = lowerMetas.findIndex(m => m[0] === tag);
        if (disc === -1) {
          throw new Error(`invalid variant tag/discriminant [${tag}] (valid tags: ${variantMetas.map(m => m[0])})`);
        }
        
        const [ _tag, lowerFn, size32, align32, payloadOffset32 ] = lowerMetas[disc];
        
        const originalPtr = ctx.resultPtr;
        ctx.vals = [disc];
        let discLowerRes;
        if (lowerMetas.length < 256) {
          discLowerRes = _lowerFlatU8(ctx);
        } else if (lowerMetas.length >= 256 && lowerMetas.length < 65536) {
          discLowerRes = _lowerFlatU16(ctx);
        } else if (lowerMetas.length >= 65536 && lowerMetas.length < 4_294_967_296) {
          discLowerRes = _lowerFlatU32(ctx);
        } else {
          throw new Error('unsupported number of cases [' + lowerMetas.legnth + ']');
        }
        
        ctx.resultPtr = originalPtr + payloadOffset32;
        
        const payloadBytesWritten = lowerFn({
          memory,
          realloc,
          vals: [val],
          storagePtr,
          storageLen,
          componentIdx,
        });
        let bytesWritten = payloadOffset + payloadBytesWritten;
        
        const rem = ctx.storagePtr % align32;
        if (rem !== 0) {
          const pad = align32 - rem;
          ctx.storagePtr += pad;
          bytesWritten += pad;
        }
        
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
    
    function _lowerFlatOption(lowerMetas) {
      function _lowerFlatOptionInner(ctx) {
        _debugLog('[_lowerFlatOption()] args', { ctx });
        return _lowerFlatVariant(lowerMetas)(ctx);
      }
    }
    
    function _lowerFlatResult(lowerMetas) {
      return function _lowerFlatResultInner(ctx) {
        _debugLog('[_lowerFlatResult()] args', { lowerMetas });
        return _lowerFlatVariant(lowerMetas)(ctx);
      };
    }
    
    function _lowerFlatOwn(size, memory, vals, storagePtr, storageLen) {
      _debugLog('[_lowerFlatOwn()] args', { size, memory, vals, storagePtr, storageLen });
      throw new Error('flat lower for owned resources not yet implemented!');
    }
    
    const STREAMS = new RepTable({ target: 'global stream map' });
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
      #errored = null;
      
      #backpressure = 0;
      #backpressureWaiters = 0n;
      
      #handlerMap = new Map();
      #nextHandlerID = 0n;
      
      #tickLoop = null;
      #tickLoopInterval = null;
      
      #onExclusiveReleaseHandlers = [];
      
      mayLeave = true;
      
      handles;
      subtasks;
      
      constructor(args) {
        this.#componentIdx = args.componentIdx;
        this.handles = new RepTable({ target: `component [${this.#componentIdx}] handles (waitable objects)` });
        this.subtasks = new RepTable({ target: `component [${this.#componentIdx}] subtasks` });
      };
      
      componentIdx() { return this.#componentIdx; }
      
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
      
      setBackpressure(v) {
        this.#backpressure = v;
        return this.#backpressure
      }
      getBackpressure() { return this.#backpressure; }
      
      incrementBackpressure() {
        const current = this.#backpressure;
        if (current < 0 || current > 2**16) {
          throw new Error(`invalid current backpressure value [${current}]`);
        }
        const newValue = this.getBackpressure() + 1;
        if (newValue >= 2**16) {
          throw new Error(`invalid new backpressure value [${newValue}], overflow`);
        }
        return this.setBackpressure(newValue);
      }
      
      decrementBackpressure() {
        const current = this.#backpressure;
        if (current < 0 || current > 2**16) {
          throw new Error(`invalid current backpressure value [${current}]`);
        }
        const newValue = Math.max(0, current - 1);
        if (newValue < 0) {
          throw new Error(`invalid new backpressure value [${newValue}], underflow`);
        }
        return this.setBackpressure(newValue);
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
      
      isExclusivelyLocked() { return this.#locked === true; }
      setLocked(locked) {
        this.#locked = locked;
      }
      
      // TODO(fix): we might want to check for pre-locked status here, we should be deterministically
      // going from locked -> unlocked and vice versa
      exclusiveLock() {
        _debugLog('[ComponentAsyncState#exclusiveLock()]', {
          locked: this.#locked,
          componentIdx: this.#componentIdx,
        });
        this.setLocked(true);
      }
      
      exclusiveRelease() {
        _debugLog('[ComponentAsyncState#exclusiveRelease()] args', {
          locked: this.#locked,
          componentIdx: this.#componentIdx,
        });
        this.setLocked(false);
        
        this.#onExclusiveReleaseHandlers = this.#onExclusiveReleaseHandlers.filter(v => !!v);
        for (const [idx, f] of this.#onExclusiveReleaseHandlers.entries()) {
          try {
            this.#onExclusiveReleaseHandlers[idx] = null;
            f();
          } catch (err) {
            _debugLog("error while executing handler for next exclusive release", err);
            throw err;
          }
        }
      }
      
      onNextExclusiveRelease(fn) {
        _debugLog('[ComponentAsyncState#()onNextExclusiveRelease] registering');
        this.#onExclusiveReleaseHandlers.push(fn);
      }
      
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
      
      // TODO(threads): readyFn is normally on the thread
      suspendTask(args) {
        const { task, readyFn } = args;
        const taskID = task.id();
        _debugLog('[ComponentAsyncState#suspendTask()]', {
          taskID,
          componentIdx: this.#componentIdx,
          taskEntryFnName: task.entryFnName(),
          subtask: task.getParentSubtask(),
        });
        
        if (this.#getSuspendedTaskMeta(taskID)) {
          throw new Error(`task [${taskID}] already suspended`);
        }
        
        const { promise, resolve, reject } = promiseWithResolvers();
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
        
        this.runTickLoop();
        
        return promise;
      }
      
      resumeTaskByID(taskID) {
        const meta = this.#removeSuspendedTaskMeta(taskID);
        if (!meta) { return; }
        if (meta.taskID !== taskID) { throw new Error('task ID does not match'); }
        meta.resume();
      }
      
      async runTickLoop() {
        if (this.#tickLoop !== null) { return; }
        this.#tickLoop = 1;
        setTimeout(async () => {
          let done = this.tick();
          while (!done) {
            await new Promise((resolve) => setTimeout(resolve, 30));
            done = this.tick();
          }
          this.#tickLoop = null;
        }, 10);
      }
      
      tick() {
        // _debugLog('[ComponentAsyncState#tick()]', { suspendedTaskIDs: this.#suspendedTaskIDs });
        
        const resumableTasks = this.#suspendedTaskIDs.filter(t => t !== null);
        for (const taskID of resumableTasks) {
          const meta = this.#suspendedTasksByTaskID.get(taskID);
          if (!meta || !meta.readyFn) {
            throw new Error(`missing/invalid task despite ID [${taskID}] being present`);
          }
          
          // If the task failed via any means, allow the task to resume because
          // it's been cancelled -- the callback should immediately exit as well
          if (meta.task.isRejected()) {
            _debugLog('[ComponentAsyncState#suspendTask()] detected task rejection, leaving early', { meta });
            this.resumeTaskByID(taskID);
            return;
          }
          
          const isReady = meta.readyFn();
          if (!isReady) { continue; }
          
          this.resumeTaskByID(taskID);
        }
        
        return this.#suspendedTaskIDs.filter(t => t !== null).length === 0;
      }
      
      addStreamEndToTable(args) {
        _debugLog('[ComponentAsyncState#addStreamEnd()] args', args);
        const { tableIdx, streamEnd } = args;
        if (typeof streamEnd === 'number') { throw new Error("INSERTING BAD STREAMEND"); }
        
        let { table, componentIdx } = STREAM_TABLES[tableIdx];
        if (componentIdx === undefined || !table) {
          throw new Error(`invalid global stream table state for table [${tableIdx}]`);
        }
        
        const handle = table.insert(streamEnd);
        streamEnd.setHandle(handle);
        streamEnd.setStreamTableIdx(tableIdx);
        
        const cstate = getOrCreateAsyncState(componentIdx);
        const waitableIdx = cstate.handles.insert(streamEnd);
        streamEnd.setWaitableIdx(waitableIdx);
        
        _debugLog('[ComponentAsyncState#addStreamEnd()] added stream end', {
          tableIdx,
          table,
          handle,
          streamEnd,
          destComponentIdx: componentIdx,
        });
        
        return { handle, waitableIdx };
      }
      
      createWaitable(args) {
        return new Waitable({ target: args?.target, });
      }
      
      createStream(args) {
        _debugLog('[ComponentAsyncState#createStream()] args', args);
        const { tableIdx, elemMeta } = args;
        if (tableIdx === undefined) { throw new Error("missing table idx while adding stream"); }
        if (elemMeta === undefined) { throw new Error("missing element metadata while adding stream"); }
        
        const { table: localStreamTable, componentIdx } = STREAM_TABLES[tableIdx];
        if (!localStreamTable) {
          throw new Error(`missing global stream table lookup for table [${tableIdx}] while creating stream`);
        }
        if (componentIdx !== this.#componentIdx) {
          throw new Error('component idx mismatch while creating stream');
        }
        
        const readWaitable = this.createWaitable();
        const writeWaitable = this.createWaitable();
        
        const stream = new InternalStream({
          tableIdx,
          componentIdx: this.#componentIdx,
          elemMeta,
          readWaitable,
          writeWaitable,
        });
        stream.setGlobalStreamMapRep(STREAMS.insert(stream));
        
        const writeEnd = stream.writeEnd();
        writeEnd.setWaitableIdx(this.handles.insert(writeEnd));
        writeEnd.setHandle(localStreamTable.insert(writeEnd));
        if (writeEnd.streamTableIdx() !== tableIdx) { throw new Error("unexpectedly mismatched stream table"); }
        
        const writeEndWaitableIdx = writeEnd.waitableIdx();
        const writeEndHandle = writeEnd.handle();
        writeWaitable.setTarget(`waitable for stream write end (waitable [${writeEndWaitableIdx}])`);
        writeEnd.setTarget(`stream write end (waitable [${writeEndWaitableIdx}])`);
        
        const readEnd = stream.readEnd();
        readEnd.setWaitableIdx(this.handles.insert(readEnd));
        readEnd.setHandle(localStreamTable.insert(readEnd));
        if (readEnd.streamTableIdx() !== tableIdx) { throw new Error("unexpectedly mismatched stream table"); }
        
        const readEndWaitableIdx = readEnd.waitableIdx();
        const readEndHandle = readEnd.handle();
        readWaitable.setTarget(`waitable for read end (waitable [${readEndWaitableIdx}])`);
        readEnd.setTarget(`stream read end (waitable [${readEndWaitableIdx}])`);
        
        return {
          writeEndWaitableIdx,
          writeEndHandle,
          readEndWaitableIdx,
          readEndHandle,
        };
      }
      
      getStreamEnd(args) {
        _debugLog('[ComponentAsyncState#getStreamEnd()] args', args);
        const { tableIdx, streamEndHandle, streamEndWaitableIdx } = args;
        if (tableIdx === undefined) { throw new Error('missing table idx while getting stream end'); }
        
        const { table, componentIdx } = STREAM_TABLES[tableIdx];
        const cstate = getOrCreateAsyncState(componentIdx);
        
        let streamEnd;
        if (streamEndWaitableIdx !== undefined) {
          streamEnd = cstate.handles.get(streamEndWaitableIdx);
        } else if (streamEndHandle !== undefined) {
          if (!table) { throw new Error(`missing/invalid table [${tableIdx}] while getting stream end`); }
          streamEnd = table.get(streamEndHandle);
        } else {
          throw new TypeError("must specify either waitable idx or handle to retrieve stream");
        }
        
        if (!streamEnd) {
          throw new Error(`missing stream end (tableIdx [${tableIdx}], handle [${streamEndHandle}], waitableIdx [${streamEndWaitableIdx}])`);
        }
        if (tableIdx && streamEnd.streamTableIdx() !== tableIdx) {
          throw new Error(`stream end table idx [${streamEnd.streamTableIdx()}] does not match [${tableIdx}]`);
        }
        
        return streamEnd;
      }
      
      deleteStreamEnd(args) {
        _debugLog('[ComponentAsyncState#deleteStreamEnd()] args', args);
        const { tableIdx, streamEndWaitableIdx } = args;
        if (tableIdx === undefined) { throw new Error("missing table idx while removing stream end"); }
        if (streamEndWaitableIdx === undefined) { throw new Error("missing stream idx while removing stream end"); }
        
        const { table, componentIdx } = STREAM_TABLES[tableIdx];
        const cstate = getOrCreateAsyncState(componentIdx);
        
        const streamEnd = cstate.handles.get(streamEndWaitableIdx);
        if (!streamEnd) {
          throw new Error(`missing stream end [${streamEndWaitableIdx}] in component handles while deleting stream`);
        }
        if (streamEnd.streamTableIdx() !== tableIdx) {
          throw new Error(`stream end table idx [${streamEnd.streamTableIdx()}] does not match [${tableIdx}]`);
        }
        
        let removed = cstate.handles.remove(streamEnd.waitableIdx());
        if (!removed) {
          throw new Error(`failed to remove stream end [${streamEndWaitableIdx}] waitable obj in component [${componentIdx}]`);
        }
        
        removed = table.remove(streamEnd.handle());
        if (!removed) {
          throw new Error(`failed to remove stream end with handle [${streamEnd.handle()}] from stream table [${tableIdx}] in component [${componentIdx}]`);
        }
        
        return streamEnd;
      }
      
      removeStreamEndFromTable(args) {
        _debugLog('[ComponentAsyncState#removeStreamEndFromTable()] args', args);
        
        const { tableIdx, streamWaitableIdx } = args;
        if (tableIdx === undefined) { throw new Error("missing table idx while removing stream end"); }
        if (streamWaitableIdx === undefined) {
          throw new Error("missing stream end waitable idx while removing stream end");
        }
        
        const { table, componentIdx } = STREAM_TABLES[tableIdx];
        if (!table) { throw new Error(`missing/invalid table [${tableIdx}] while removing stream end`); }
        
        const cstate = getOrCreateAsyncState(componentIdx);
        
        const streamEnd = cstate.handles.get(streamWaitableIdx);
        if (!streamEnd) {
          throw new Error(`missing stream end (handle [${streamWaitableIdx}], table [${tableIdx}])`);
        }
        const handle = streamEnd.handle();
        
        let removed = cstate.handles.remove(streamWaitableIdx);
        if (!removed) {
          throw new Error(`failed to remove streamEnd from handles (waitable idx [${streamWaitableIdx}]), component [${componentIdx}])`);
        }
        
        removed = table.remove(handle);
        if (!removed) {
          throw new Error(`failed to remove streamEnd from table (handle [${handle}]), table [${tableIdx}], component [${componentIdx}])`);
        }
        
        return streamEnd;
      }
    }
    
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
    
    
    if (!getCoreModule) getCoreModule = (name) => fetchCompile(new URL(`./${name}`, import.meta.url));
    const module0 = getCoreModule('spar_wasm.core.wasm');
    const module1 = getCoreModule('spar_wasm.core2.wasm');
    const module2 = getCoreModule('spar_wasm.core3.wasm');
    
    const { getEnvironment } = imports['wasi:cli/environment'];
    getEnvironment._isHostProvided = true;
    
    if (getEnvironment=== undefined) {
      const err = new Error("unexpectedly undefined instance import 'getEnvironment', was 'getEnvironment' available at instantiation?");
      console.error("ERROR:", err.toString());
      throw err;
    }
    
    const { exit } = imports['wasi:cli/exit'];
    exit._isHostProvided = true;
    
    if (exit=== undefined) {
      const err = new Error("unexpectedly undefined instance import 'exit', was 'exit' available at instantiation?");
      console.error("ERROR:", err.toString());
      throw err;
    }
    
    const { getStderr } = imports['wasi:cli/stderr'];
    getStderr._isHostProvided = true;
    
    if (getStderr=== undefined) {
      const err = new Error("unexpectedly undefined instance import 'getStderr', was 'getStderr' available at instantiation?");
      console.error("ERROR:", err.toString());
      throw err;
    }
    
    const { getStdin } = imports['wasi:cli/stdin'];
    getStdin._isHostProvided = true;
    
    if (getStdin=== undefined) {
      const err = new Error("unexpectedly undefined instance import 'getStdin', was 'getStdin' available at instantiation?");
      console.error("ERROR:", err.toString());
      throw err;
    }
    
    const { getStdout } = imports['wasi:cli/stdout'];
    getStdout._isHostProvided = true;
    
    if (getStdout=== undefined) {
      const err = new Error("unexpectedly undefined instance import 'getStdout', was 'getStdout' available at instantiation?");
      console.error("ERROR:", err.toString());
      throw err;
    }
    
    const { TerminalInput } = imports['wasi:cli/terminal-input'];
    TerminalInput._isHostProvided = true;
    
    if (TerminalInput=== undefined) {
      const err = new Error("unexpectedly undefined instance import 'TerminalInput', was 'TerminalInput' available at instantiation?");
      console.error("ERROR:", err.toString());
      throw err;
    }
    
    const { TerminalOutput } = imports['wasi:cli/terminal-output'];
    TerminalOutput._isHostProvided = true;
    
    if (TerminalOutput=== undefined) {
      const err = new Error("unexpectedly undefined instance import 'TerminalOutput', was 'TerminalOutput' available at instantiation?");
      console.error("ERROR:", err.toString());
      throw err;
    }
    
    const { getTerminalStderr } = imports['wasi:cli/terminal-stderr'];
    getTerminalStderr._isHostProvided = true;
    
    if (getTerminalStderr=== undefined) {
      const err = new Error("unexpectedly undefined instance import 'getTerminalStderr', was 'getTerminalStderr' available at instantiation?");
      console.error("ERROR:", err.toString());
      throw err;
    }
    
    const { getTerminalStdin } = imports['wasi:cli/terminal-stdin'];
    getTerminalStdin._isHostProvided = true;
    
    if (getTerminalStdin=== undefined) {
      const err = new Error("unexpectedly undefined instance import 'getTerminalStdin', was 'getTerminalStdin' available at instantiation?");
      console.error("ERROR:", err.toString());
      throw err;
    }
    
    const { getTerminalStdout } = imports['wasi:cli/terminal-stdout'];
    getTerminalStdout._isHostProvided = true;
    
    if (getTerminalStdout=== undefined) {
      const err = new Error("unexpectedly undefined instance import 'getTerminalStdout', was 'getTerminalStdout' available at instantiation?");
      console.error("ERROR:", err.toString());
      throw err;
    }
    
    const { now } = imports['wasi:clocks/monotonic-clock'];
    now._isHostProvided = true;
    
    if (now=== undefined) {
      const err = new Error("unexpectedly undefined instance import 'now', was 'now' available at instantiation?");
      console.error("ERROR:", err.toString());
      throw err;
    }
    
    const { now: now$1 } = imports['wasi:clocks/wall-clock'];
    now$1._isHostProvided = true;
    
    if (now$1=== undefined) {
      const err = new Error("unexpectedly undefined instance import 'now$1', was 'now' available at instantiation?");
      console.error("ERROR:", err.toString());
      throw err;
    }
    
    const { getDirectories } = imports['wasi:filesystem/preopens'];
    getDirectories._isHostProvided = true;
    
    if (getDirectories=== undefined) {
      const err = new Error("unexpectedly undefined instance import 'getDirectories', was 'getDirectories' available at instantiation?");
      console.error("ERROR:", err.toString());
      throw err;
    }
    
    const { Descriptor, DirectoryEntryStream } = imports['wasi:filesystem/types'];
    Descriptor._isHostProvided = true;
    
    if (Descriptor=== undefined) {
      const err = new Error("unexpectedly undefined instance import 'Descriptor', was 'Descriptor' available at instantiation?");
      console.error("ERROR:", err.toString());
      throw err;
    }
    
    DirectoryEntryStream._isHostProvided = true;
    
    if (DirectoryEntryStream=== undefined) {
      const err = new Error("unexpectedly undefined instance import 'DirectoryEntryStream', was 'DirectoryEntryStream' available at instantiation?");
      console.error("ERROR:", err.toString());
      throw err;
    }
    
    const { Error: Error$1 } = imports['wasi:io/error'];
    Error$1._isHostProvided = true;
    
    if (Error$1=== undefined) {
      const err = new Error("unexpectedly undefined instance import 'Error$1', was 'Error' available at instantiation?");
      console.error("ERROR:", err.toString());
      throw err;
    }
    
    const { Pollable } = imports['wasi:io/poll'];
    Pollable._isHostProvided = true;
    
    if (Pollable=== undefined) {
      const err = new Error("unexpectedly undefined instance import 'Pollable', was 'Pollable' available at instantiation?");
      console.error("ERROR:", err.toString());
      throw err;
    }
    
    const { InputStream, OutputStream } = imports['wasi:io/streams'];
    InputStream._isHostProvided = true;
    
    if (InputStream=== undefined) {
      const err = new Error("unexpectedly undefined instance import 'InputStream', was 'InputStream' available at instantiation?");
      console.error("ERROR:", err.toString());
      throw err;
    }
    
    OutputStream._isHostProvided = true;
    
    if (OutputStream=== undefined) {
      const err = new Error("unexpectedly undefined instance import 'OutputStream', was 'OutputStream' available at instantiation?");
      console.error("ERROR:", err.toString());
      throw err;
    }
    
    const { insecureSeed } = imports['wasi:random/insecure-seed'];
    insecureSeed._isHostProvided = true;
    
    if (insecureSeed=== undefined) {
      const err = new Error("unexpectedly undefined instance import 'insecureSeed', was 'insecureSeed' available at instantiation?");
      console.error("ERROR:", err.toString());
      throw err;
    }
    
    let gen = (function* _initGenerator () {
      let exports0;
      
      const _trampoline8 = function(arg0) {
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
        let hostProvided = true;
        
        let parentTask;
        let task;
        let subtask;
        
        const createTask = () => {
          const results = createNewCurrentTask({
            componentIdx: -1, // 0,
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
          
          if (hostProvided) {
            subtask = parentTask.getLatestSubtask();
            if (!subtask) {
              throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
            }
            task.setParentSubtask(subtask);
          }
        }
        
        const started = task.enterSync();
        let ret;_withGlobalCurrentTaskMeta({
          componentIdx: task.componentIdx(),
          taskID: task.id(),
          fn: () => exit(variant0)
        })
        ;
        _debugLog('[iface="wasi:cli/exit@0.2.6", function="exit"][Instruction::Return]', {
          funcName: 'exit',
          paramCount: 0,
          async: false,
          postReturn: false
        });
        task.resolve([ret]);
        task.exit();
      }
      _trampoline8.fnName = 'wasi:cli/exit@0.2.6#exit';
      const handleTable0 = [T_FLAG, 0];
      const captureTable0= new Map();
      let captureCnt0 = 0;
      handleTables[0] = handleTable0;
      
      const _trampoline9 = function(arg0) {
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
        let hostProvided = true;
        
        let parentTask;
        let task;
        let subtask;
        
        const createTask = () => {
          const results = createNewCurrentTask({
            componentIdx: -1, // 0,
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
          
          if (hostProvided) {
            subtask = parentTask.getLatestSubtask();
            if (!subtask) {
              throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
            }
            task.setParentSubtask(subtask);
          }
        }
        
        const started = task.enterSync();
        let ret;_withGlobalCurrentTaskMeta({
          componentIdx: task.componentIdx(),
          taskID: task.id(),
          fn: () => rsc0.block()
        })
        ;
        for (const rsc of curResourceBorrows) {
          rsc[symbolRscHandle] = undefined;
        }
        curResourceBorrows = [];
        _debugLog('[iface="wasi:io/poll@0.2.6", function="[method]pollable.block"][Instruction::Return]', {
          funcName: '[method]pollable.block',
          paramCount: 0,
          async: false,
          postReturn: false
        });
        task.resolve([ret]);
        task.exit();
      }
      _trampoline9.fnName = 'wasi:io/poll@0.2.6#block';
      const handleTable2 = [T_FLAG, 0];
      const captureTable2= new Map();
      let captureCnt2 = 0;
      handleTables[2] = handleTable2;
      
      const _trampoline10 = function(arg0) {
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
        let hostProvided = true;
        
        let parentTask;
        let task;
        let subtask;
        
        const createTask = () => {
          const results = createNewCurrentTask({
            componentIdx: -1, // 0,
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
          
          if (hostProvided) {
            subtask = parentTask.getLatestSubtask();
            if (!subtask) {
              throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
            }
            task.setParentSubtask(subtask);
          }
        }
        
        const started = task.enterSync();
        let ret = _withGlobalCurrentTaskMeta({
          componentIdx: task.componentIdx(),
          taskID: task.id(),
          fn: () => rsc0.subscribe()
        })
        ;
        for (const rsc of curResourceBorrows) {
          rsc[symbolRscHandle] = undefined;
        }
        curResourceBorrows = [];
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
        task.resolve([handle3]);
        task.exit();
        return handle3;
      }
      _trampoline10.fnName = 'wasi:io/streams@0.2.6#subscribe';
      const handleTable3 = [T_FLAG, 0];
      const captureTable3= new Map();
      let captureCnt3 = 0;
      handleTables[3] = handleTable3;
      
      const _trampoline11 = function(arg0) {
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
        let hostProvided = true;
        
        let parentTask;
        let task;
        let subtask;
        
        const createTask = () => {
          const results = createNewCurrentTask({
            componentIdx: -1, // 0,
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
          
          if (hostProvided) {
            subtask = parentTask.getLatestSubtask();
            if (!subtask) {
              throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
            }
            task.setParentSubtask(subtask);
          }
        }
        
        const started = task.enterSync();
        let ret = _withGlobalCurrentTaskMeta({
          componentIdx: task.componentIdx(),
          taskID: task.id(),
          fn: () => rsc0.subscribe()
        })
        ;
        for (const rsc of curResourceBorrows) {
          rsc[symbolRscHandle] = undefined;
        }
        curResourceBorrows = [];
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
        task.resolve([handle3]);
        task.exit();
        return handle3;
      }
      _trampoline11.fnName = 'wasi:io/streams@0.2.6#subscribe';
      
      const _trampoline12 = function() {
        _debugLog('[iface="wasi:cli/stdin@0.2.6", function="get-stdin"] [Instruction::CallInterface] (sync, @ enter)');
        let hostProvided = true;
        
        let parentTask;
        let task;
        let subtask;
        
        const createTask = () => {
          const results = createNewCurrentTask({
            componentIdx: -1, // 0,
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
          
          if (hostProvided) {
            subtask = parentTask.getLatestSubtask();
            if (!subtask) {
              throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
            }
            task.setParentSubtask(subtask);
          }
        }
        
        const started = task.enterSync();
        let ret = _withGlobalCurrentTaskMeta({
          componentIdx: task.componentIdx(),
          taskID: task.id(),
          fn: () => getStdin()
        })
        ;
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
        task.resolve([handle0]);
        task.exit();
        return handle0;
      }
      _trampoline12.fnName = 'wasi:cli/stdin@0.2.6#getStdin';
      
      const _trampoline13 = function() {
        _debugLog('[iface="wasi:cli/stdout@0.2.6", function="get-stdout"] [Instruction::CallInterface] (sync, @ enter)');
        let hostProvided = true;
        
        let parentTask;
        let task;
        let subtask;
        
        const createTask = () => {
          const results = createNewCurrentTask({
            componentIdx: -1, // 0,
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
          
          if (hostProvided) {
            subtask = parentTask.getLatestSubtask();
            if (!subtask) {
              throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
            }
            task.setParentSubtask(subtask);
          }
        }
        
        const started = task.enterSync();
        let ret = _withGlobalCurrentTaskMeta({
          componentIdx: task.componentIdx(),
          taskID: task.id(),
          fn: () => getStdout()
        })
        ;
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
        task.resolve([handle0]);
        task.exit();
        return handle0;
      }
      _trampoline13.fnName = 'wasi:cli/stdout@0.2.6#getStdout';
      
      const _trampoline14 = function() {
        _debugLog('[iface="wasi:cli/stderr@0.2.6", function="get-stderr"] [Instruction::CallInterface] (sync, @ enter)');
        let hostProvided = true;
        
        let parentTask;
        let task;
        let subtask;
        
        const createTask = () => {
          const results = createNewCurrentTask({
            componentIdx: -1, // 0,
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
          
          if (hostProvided) {
            subtask = parentTask.getLatestSubtask();
            if (!subtask) {
              throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
            }
            task.setParentSubtask(subtask);
          }
        }
        
        const started = task.enterSync();
        let ret = _withGlobalCurrentTaskMeta({
          componentIdx: task.componentIdx(),
          taskID: task.id(),
          fn: () => getStderr()
        })
        ;
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
        task.resolve([handle0]);
        task.exit();
        return handle0;
      }
      _trampoline14.fnName = 'wasi:cli/stderr@0.2.6#getStderr';
      
      const _trampoline15 = function() {
        _debugLog('[iface="wasi:clocks/monotonic-clock@0.2.6", function="now"] [Instruction::CallInterface] (sync, @ enter)');
        let hostProvided = true;
        
        let parentTask;
        let task;
        let subtask;
        
        const createTask = () => {
          const results = createNewCurrentTask({
            componentIdx: -1, // 0,
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
          
          if (hostProvided) {
            subtask = parentTask.getLatestSubtask();
            if (!subtask) {
              throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
            }
            task.setParentSubtask(subtask);
          }
        }
        
        const started = task.enterSync();
        let ret = _withGlobalCurrentTaskMeta({
          componentIdx: task.componentIdx(),
          taskID: task.id(),
          fn: () => now()
        })
        ;
        _debugLog('[iface="wasi:clocks/monotonic-clock@0.2.6", function="now"][Instruction::Return]', {
          funcName: 'now',
          paramCount: 1,
          async: false,
          postReturn: false
        });
        task.resolve([toUint64(ret)]);
        task.exit();
        return toUint64(ret);
      }
      _trampoline15.fnName = 'wasi:clocks/monotonic-clock@0.2.6#now';
      let exports1;
      let memory0;
      let realloc0;
      let realloc0Async;
      
      const _trampoline16 = function(arg0) {
        _debugLog('[iface="wasi:random/insecure-seed@0.2.6", function="insecure-seed"] [Instruction::CallInterface] (sync, @ enter)');
        let hostProvided = true;
        
        let parentTask;
        let task;
        let subtask;
        
        const createTask = () => {
          const results = createNewCurrentTask({
            componentIdx: -1, // 0,
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
          
          if (hostProvided) {
            subtask = parentTask.getLatestSubtask();
            if (!subtask) {
              throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
            }
            task.setParentSubtask(subtask);
          }
        }
        
        const started = task.enterSync();
        let ret = _withGlobalCurrentTaskMeta({
          componentIdx: task.componentIdx(),
          taskID: task.id(),
          fn: () => insecureSeed()
        })
        ;
        var [tuple0_0, tuple0_1] = ret;
        dataView(memory0).setBigInt64(arg0 + 0, toUint64(tuple0_0), true);
        dataView(memory0).setBigInt64(arg0 + 8, toUint64(tuple0_1), true);
        _debugLog('[iface="wasi:random/insecure-seed@0.2.6", function="insecure-seed"][Instruction::Return]', {
          funcName: 'insecure-seed',
          paramCount: 0,
          async: false,
          postReturn: false
        });
        task.resolve([ret]);
        task.exit();
      }
      _trampoline16.fnName = 'wasi:random/insecure-seed@0.2.6#insecureSeed';
      const handleTable1 = [T_FLAG, 0];
      const captureTable1= new Map();
      let captureCnt1 = 0;
      handleTables[1] = handleTable1;
      
      const _trampoline17 = function(arg0, arg1, arg2) {
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
        let hostProvided = true;
        
        let parentTask;
        let task;
        let subtask;
        
        const createTask = () => {
          const results = createNewCurrentTask({
            componentIdx: -1, // 0,
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
          
          if (hostProvided) {
            subtask = parentTask.getLatestSubtask();
            if (!subtask) {
              throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
            }
            task.setParentSubtask(subtask);
          }
        }
        
        const started = task.enterSync();
        
        let ret;
        try {
          ret = { tag: 'ok', val: _withGlobalCurrentTaskMeta({
            componentIdx: task.componentIdx(),
            taskID: task.id(),
            fn: () => rsc0.blockingRead(BigInt.asUintN(64, BigInt(arg1)))
          })
        };
      } catch (e) {
        ret = { tag: 'err', val: getErrorPayload(e) };
      }
      
      for (const rsc of curResourceBorrows) {
        rsc[symbolRscHandle] = undefined;
      }
      curResourceBorrows = [];
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
      task.resolve([ret]);
      task.exit();
    }
    _trampoline17.fnName = 'wasi:io/streams@0.2.6#blockingRead';
    
    const _trampoline18 = function(arg0, arg1) {
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
      let hostProvided = true;
      
      let parentTask;
      let task;
      let subtask;
      
      const createTask = () => {
        const results = createNewCurrentTask({
          componentIdx: -1, // 0,
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
        
        if (hostProvided) {
          subtask = parentTask.getLatestSubtask();
          if (!subtask) {
            throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
          }
          task.setParentSubtask(subtask);
        }
      }
      
      const started = task.enterSync();
      
      let ret;
      try {
        ret = { tag: 'ok', val: _withGlobalCurrentTaskMeta({
          componentIdx: task.componentIdx(),
          taskID: task.id(),
          fn: () => rsc0.checkWrite()
        })
      };
    } catch (e) {
      ret = { tag: 'err', val: getErrorPayload(e) };
    }
    
    for (const rsc of curResourceBorrows) {
      rsc[symbolRscHandle] = undefined;
    }
    curResourceBorrows = [];
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
    task.resolve([ret]);
    task.exit();
  }
  _trampoline18.fnName = 'wasi:io/streams@0.2.6#checkWrite';
  
  const _trampoline19 = function(arg0, arg1, arg2, arg3) {
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
    let hostProvided = true;
    
    let parentTask;
    let task;
    let subtask;
    
    const createTask = () => {
      const results = createNewCurrentTask({
        componentIdx: -1, // 0,
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
      
      if (hostProvided) {
        subtask = parentTask.getLatestSubtask();
        if (!subtask) {
          throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
        }
        task.setParentSubtask(subtask);
      }
    }
    
    const started = task.enterSync();
    
    let ret;
    try {
      ret = { tag: 'ok', val: _withGlobalCurrentTaskMeta({
        componentIdx: task.componentIdx(),
        taskID: task.id(),
        fn: () => rsc0.write(result3)
      })
    };
  } catch (e) {
    ret = { tag: 'err', val: getErrorPayload(e) };
  }
  
  for (const rsc of curResourceBorrows) {
    rsc[symbolRscHandle] = undefined;
  }
  curResourceBorrows = [];
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
  task.resolve([ret]);
  task.exit();
}
_trampoline19.fnName = 'wasi:io/streams@0.2.6#write';

const _trampoline20 = function(arg0, arg1) {
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
  let hostProvided = true;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: -1, // 0,
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
    
    if (hostProvided) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
      }
      task.setParentSubtask(subtask);
    }
  }
  
  const started = task.enterSync();
  
  let ret;
  try {
    ret = { tag: 'ok', val: _withGlobalCurrentTaskMeta({
      componentIdx: task.componentIdx(),
      taskID: task.id(),
      fn: () => rsc0.blockingFlush()
    })
  };
} catch (e) {
  ret = { tag: 'err', val: getErrorPayload(e) };
}

for (const rsc of curResourceBorrows) {
  rsc[symbolRscHandle] = undefined;
}
curResourceBorrows = [];
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
task.resolve([ret]);
task.exit();
}
_trampoline20.fnName = 'wasi:io/streams@0.2.6#blockingFlush';
const handleTable6 = [T_FLAG, 0];
const captureTable6= new Map();
let captureCnt6 = 0;
handleTables[6] = handleTable6;

const _trampoline21 = function(arg0, arg1, arg2) {
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
  let hostProvided = true;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: -1, // 0,
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
    
    if (hostProvided) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
      }
      task.setParentSubtask(subtask);
    }
  }
  
  const started = task.enterSync();
  
  let ret;
  try {
    ret = { tag: 'ok', val: _withGlobalCurrentTaskMeta({
      componentIdx: task.componentIdx(),
      taskID: task.id(),
      fn: () => rsc0.readViaStream(BigInt.asUintN(64, BigInt(arg1)))
    })
  };
} catch (e) {
  ret = { tag: 'err', val: getErrorPayload(e) };
}

for (const rsc of curResourceBorrows) {
  rsc[symbolRscHandle] = undefined;
}
curResourceBorrows = [];
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
task.resolve([ret]);
task.exit();
}
_trampoline21.fnName = 'wasi:filesystem/types@0.2.6#readViaStream';

const _trampoline22 = function(arg0, arg1, arg2) {
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
  let hostProvided = true;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: -1, // 0,
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
    
    if (hostProvided) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
      }
      task.setParentSubtask(subtask);
    }
  }
  
  const started = task.enterSync();
  
  let ret;
  try {
    ret = { tag: 'ok', val: _withGlobalCurrentTaskMeta({
      componentIdx: task.componentIdx(),
      taskID: task.id(),
      fn: () => rsc0.writeViaStream(BigInt.asUintN(64, BigInt(arg1)))
    })
  };
} catch (e) {
  ret = { tag: 'err', val: getErrorPayload(e) };
}

for (const rsc of curResourceBorrows) {
  rsc[symbolRscHandle] = undefined;
}
curResourceBorrows = [];
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
task.resolve([ret]);
task.exit();
}
_trampoline22.fnName = 'wasi:filesystem/types@0.2.6#writeViaStream';

const _trampoline23 = function(arg0, arg1) {
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
  let hostProvided = true;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: -1, // 0,
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
    
    if (hostProvided) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
      }
      task.setParentSubtask(subtask);
    }
  }
  
  const started = task.enterSync();
  
  let ret;
  try {
    ret = { tag: 'ok', val: _withGlobalCurrentTaskMeta({
      componentIdx: task.componentIdx(),
      taskID: task.id(),
      fn: () => rsc0.appendViaStream()
    })
  };
} catch (e) {
  ret = { tag: 'err', val: getErrorPayload(e) };
}

for (const rsc of curResourceBorrows) {
  rsc[symbolRscHandle] = undefined;
}
curResourceBorrows = [];
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
task.resolve([ret]);
task.exit();
}
_trampoline23.fnName = 'wasi:filesystem/types@0.2.6#appendViaStream';

const _trampoline24 = function(arg0, arg1) {
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
  let hostProvided = true;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: -1, // 0,
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
    
    if (hostProvided) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
      }
      task.setParentSubtask(subtask);
    }
  }
  
  const started = task.enterSync();
  
  let ret;
  try {
    ret = { tag: 'ok', val: _withGlobalCurrentTaskMeta({
      componentIdx: task.componentIdx(),
      taskID: task.id(),
      fn: () => rsc0.getFlags()
    })
  };
} catch (e) {
  ret = { tag: 'err', val: getErrorPayload(e) };
}

for (const rsc of curResourceBorrows) {
  rsc[symbolRscHandle] = undefined;
}
curResourceBorrows = [];
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
task.resolve([ret]);
task.exit();
}
_trampoline24.fnName = 'wasi:filesystem/types@0.2.6#getFlags';
const handleTable7 = [T_FLAG, 0];
const captureTable7= new Map();
let captureCnt7 = 0;
handleTables[7] = handleTable7;

const _trampoline25 = function(arg0, arg1) {
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
  let hostProvided = true;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: -1, // 0,
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
    
    if (hostProvided) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
      }
      task.setParentSubtask(subtask);
    }
  }
  
  const started = task.enterSync();
  
  let ret;
  try {
    ret = { tag: 'ok', val: _withGlobalCurrentTaskMeta({
      componentIdx: task.componentIdx(),
      taskID: task.id(),
      fn: () => rsc0.readDirectory()
    })
  };
} catch (e) {
  ret = { tag: 'err', val: getErrorPayload(e) };
}

for (const rsc of curResourceBorrows) {
  rsc[symbolRscHandle] = undefined;
}
curResourceBorrows = [];
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
task.resolve([ret]);
task.exit();
}
_trampoline25.fnName = 'wasi:filesystem/types@0.2.6#readDirectory';

const _trampoline26 = function(arg0, arg1) {
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
  let hostProvided = true;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: -1, // 0,
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
    
    if (hostProvided) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
      }
      task.setParentSubtask(subtask);
    }
  }
  
  const started = task.enterSync();
  
  let ret;
  try {
    ret = { tag: 'ok', val: _withGlobalCurrentTaskMeta({
      componentIdx: task.componentIdx(),
      taskID: task.id(),
      fn: () => rsc0.stat()
    })
  };
} catch (e) {
  ret = { tag: 'err', val: getErrorPayload(e) };
}

for (const rsc of curResourceBorrows) {
  rsc[symbolRscHandle] = undefined;
}
curResourceBorrows = [];
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
task.resolve([ret]);
task.exit();
}
_trampoline26.fnName = 'wasi:filesystem/types@0.2.6#stat';

const _trampoline27 = function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
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
  let hostProvided = true;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: -1, // 0,
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
    
    if (hostProvided) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
      }
      task.setParentSubtask(subtask);
    }
  }
  
  const started = task.enterSync();
  
  let ret;
  try {
    ret = { tag: 'ok', val: _withGlobalCurrentTaskMeta({
      componentIdx: task.componentIdx(),
      taskID: task.id(),
      fn: () => rsc0.openAt(flags3, result4, flags5, flags6)
    })
  };
} catch (e) {
  ret = { tag: 'err', val: getErrorPayload(e) };
}

for (const rsc of curResourceBorrows) {
  rsc[symbolRscHandle] = undefined;
}
curResourceBorrows = [];
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
task.resolve([ret]);
task.exit();
}
_trampoline27.fnName = 'wasi:filesystem/types@0.2.6#openAt';

const _trampoline28 = function(arg0, arg1) {
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
  let hostProvided = true;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: -1, // 0,
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
    
    if (hostProvided) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
      }
      task.setParentSubtask(subtask);
    }
  }
  
  const started = task.enterSync();
  
  let ret;
  try {
    ret = { tag: 'ok', val: _withGlobalCurrentTaskMeta({
      componentIdx: task.componentIdx(),
      taskID: task.id(),
      fn: () => rsc0.metadataHash()
    })
  };
} catch (e) {
  ret = { tag: 'err', val: getErrorPayload(e) };
}

for (const rsc of curResourceBorrows) {
  rsc[symbolRscHandle] = undefined;
}
curResourceBorrows = [];
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
task.resolve([ret]);
task.exit();
}
_trampoline28.fnName = 'wasi:filesystem/types@0.2.6#metadataHash';

const _trampoline29 = function(arg0, arg1, arg2, arg3, arg4) {
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
  let hostProvided = true;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: -1, // 0,
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
    
    if (hostProvided) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
      }
      task.setParentSubtask(subtask);
    }
  }
  
  const started = task.enterSync();
  
  let ret;
  try {
    ret = { tag: 'ok', val: _withGlobalCurrentTaskMeta({
      componentIdx: task.componentIdx(),
      taskID: task.id(),
      fn: () => rsc0.metadataHashAt(flags3, result4)
    })
  };
} catch (e) {
  ret = { tag: 'err', val: getErrorPayload(e) };
}

for (const rsc of curResourceBorrows) {
  rsc[symbolRscHandle] = undefined;
}
curResourceBorrows = [];
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
task.resolve([ret]);
task.exit();
}
_trampoline29.fnName = 'wasi:filesystem/types@0.2.6#metadataHashAt';

const _trampoline30 = function(arg0, arg1) {
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
  let hostProvided = true;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: -1, // 0,
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
    
    if (hostProvided) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
      }
      task.setParentSubtask(subtask);
    }
  }
  
  const started = task.enterSync();
  
  let ret;
  try {
    ret = { tag: 'ok', val: _withGlobalCurrentTaskMeta({
      componentIdx: task.componentIdx(),
      taskID: task.id(),
      fn: () => rsc0.readDirectoryEntry()
    })
  };
} catch (e) {
  ret = { tag: 'err', val: getErrorPayload(e) };
}

for (const rsc of curResourceBorrows) {
  rsc[symbolRscHandle] = undefined;
}
curResourceBorrows = [];
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
task.resolve([ret]);
task.exit();
}
_trampoline30.fnName = 'wasi:filesystem/types@0.2.6#readDirectoryEntry';

const _trampoline31 = function(arg0) {
  _debugLog('[iface="wasi:cli/environment@0.2.6", function="get-environment"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = true;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: -1, // 0,
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
    
    if (hostProvided) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
      }
      task.setParentSubtask(subtask);
    }
  }
  
  const started = task.enterSync();
  let ret = _withGlobalCurrentTaskMeta({
    componentIdx: task.componentIdx(),
    taskID: task.id(),
    fn: () => getEnvironment()
  })
  ;
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
  task.resolve([ret]);
  task.exit();
}
_trampoline31.fnName = 'wasi:cli/environment@0.2.6#getEnvironment';
const handleTable4 = [T_FLAG, 0];
const captureTable4= new Map();
let captureCnt4 = 0;
handleTables[4] = handleTable4;

const _trampoline32 = function(arg0) {
  _debugLog('[iface="wasi:cli/terminal-stdin@0.2.6", function="get-terminal-stdin"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = true;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: -1, // 0,
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
    
    if (hostProvided) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
      }
      task.setParentSubtask(subtask);
    }
  }
  
  const started = task.enterSync();
  let ret = _withGlobalCurrentTaskMeta({
    componentIdx: task.componentIdx(),
    taskID: task.id(),
    fn: () => getTerminalStdin()
  })
  ;
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
  task.resolve([ret]);
  task.exit();
}
_trampoline32.fnName = 'wasi:cli/terminal-stdin@0.2.6#getTerminalStdin';
const handleTable5 = [T_FLAG, 0];
const captureTable5= new Map();
let captureCnt5 = 0;
handleTables[5] = handleTable5;

const _trampoline33 = function(arg0) {
  _debugLog('[iface="wasi:cli/terminal-stdout@0.2.6", function="get-terminal-stdout"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = true;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: -1, // 0,
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
    
    if (hostProvided) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
      }
      task.setParentSubtask(subtask);
    }
  }
  
  const started = task.enterSync();
  let ret = _withGlobalCurrentTaskMeta({
    componentIdx: task.componentIdx(),
    taskID: task.id(),
    fn: () => getTerminalStdout()
  })
  ;
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
  task.resolve([ret]);
  task.exit();
}
_trampoline33.fnName = 'wasi:cli/terminal-stdout@0.2.6#getTerminalStdout';

const _trampoline34 = function(arg0) {
  _debugLog('[iface="wasi:cli/terminal-stderr@0.2.6", function="get-terminal-stderr"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = true;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: -1, // 0,
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
    
    if (hostProvided) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
      }
      task.setParentSubtask(subtask);
    }
  }
  
  const started = task.enterSync();
  let ret = _withGlobalCurrentTaskMeta({
    componentIdx: task.componentIdx(),
    taskID: task.id(),
    fn: () => getTerminalStderr()
  })
  ;
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
  task.resolve([ret]);
  task.exit();
}
_trampoline34.fnName = 'wasi:cli/terminal-stderr@0.2.6#getTerminalStderr';

const _trampoline35 = function(arg0) {
  _debugLog('[iface="wasi:clocks/wall-clock@0.2.6", function="now"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = true;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: -1, // 0,
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
    
    if (hostProvided) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
      }
      task.setParentSubtask(subtask);
    }
  }
  
  const started = task.enterSync();
  let ret = _withGlobalCurrentTaskMeta({
    componentIdx: task.componentIdx(),
    taskID: task.id(),
    fn: () => now$1()
  })
  ;
  var {seconds: v0_0, nanoseconds: v0_1 } = ret;
  dataView(memory0).setBigInt64(arg0 + 0, toUint64(v0_0), true);
  dataView(memory0).setInt32(arg0 + 8, toUint32(v0_1), true);
  _debugLog('[iface="wasi:clocks/wall-clock@0.2.6", function="now"][Instruction::Return]', {
    funcName: 'now',
    paramCount: 0,
    async: false,
    postReturn: false
  });
  task.resolve([ret]);
  task.exit();
}
_trampoline35.fnName = 'wasi:clocks/wall-clock@0.2.6#now$1';

const _trampoline36 = function(arg0) {
  _debugLog('[iface="wasi:filesystem/preopens@0.2.6", function="get-directories"] [Instruction::CallInterface] (sync, @ enter)');
  let hostProvided = true;
  
  let parentTask;
  let task;
  let subtask;
  
  const createTask = () => {
    const results = createNewCurrentTask({
      componentIdx: -1, // 0,
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
    
    if (hostProvided) {
      subtask = parentTask.getLatestSubtask();
      if (!subtask) {
        throw new Error(`Missing subtask (in parent task [${parentTask.id()}]) for host import, has the import been lowered? (ensure asyncImports are set properly)`);
      }
      task.setParentSubtask(subtask);
    }
  }
  
  const started = task.enterSync();
  let ret = _withGlobalCurrentTaskMeta({
    componentIdx: task.componentIdx(),
    taskID: task.id(),
    fn: () => getDirectories()
  })
  ;
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
  task.resolve([ret]);
  task.exit();
}
_trampoline36.fnName = 'wasi:filesystem/preopens@0.2.6#getDirectories';
let exports2;
let postReturn0;
let postReturn0Async;
let postReturn1;
let postReturn1Async;
let postReturn2;
let postReturn2Async;
let postReturn3;
let postReturn3Async;
let postReturn4;
let postReturn4Async;
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
let trampoline8 = _trampoline8.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 8,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline8.manuallyAsync,
  paramLiftFns: [_liftFlatResult([['ok', null, 0, 0, 0],['err', null, 0, 0, 0],])],
  resultLowerFns: [],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: null,
  getMemoryFn: () => null,
  getReallocFn: () => null,
  importFn: _trampoline8,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 8,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline8.manuallyAsync,
  paramLiftFns: [_liftFlatResult([['ok', null, 0, 0, 0],['err', null, 0, 0, 0],])],
  resultLowerFns: [],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: null,
  getMemoryFn: () => null,
  getReallocFn: () => null,
  importFn: _trampoline8,
},
);
let trampoline9 = _trampoline9.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 9,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline9.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 0)],
  resultLowerFns: [],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: null,
  getMemoryFn: () => null,
  getReallocFn: () => null,
  importFn: _trampoline9,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 9,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline9.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 0)],
  resultLowerFns: [],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: null,
  getMemoryFn: () => null,
  getReallocFn: () => null,
  importFn: _trampoline9,
},
);
let trampoline10 = _trampoline10.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 10,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline10.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 2)],
  resultLowerFns: [_lowerFlatOwn.bind(null, 0)],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: null,
  getMemoryFn: () => null,
  getReallocFn: () => null,
  importFn: _trampoline10,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 10,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline10.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 2)],
  resultLowerFns: [_lowerFlatOwn.bind(null, 0)],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: null,
  getMemoryFn: () => null,
  getReallocFn: () => null,
  importFn: _trampoline10,
},
);
let trampoline11 = _trampoline11.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 11,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline11.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 3)],
  resultLowerFns: [_lowerFlatOwn.bind(null, 0)],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: null,
  getMemoryFn: () => null,
  getReallocFn: () => null,
  importFn: _trampoline11,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 11,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline11.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 3)],
  resultLowerFns: [_lowerFlatOwn.bind(null, 0)],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: null,
  getMemoryFn: () => null,
  getReallocFn: () => null,
  importFn: _trampoline11,
},
);
let trampoline12 = _trampoline12.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 12,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline12.manuallyAsync,
  paramLiftFns: [],
  resultLowerFns: [_lowerFlatOwn.bind(null, 2)],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: null,
  getMemoryFn: () => null,
  getReallocFn: () => null,
  importFn: _trampoline12,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 12,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline12.manuallyAsync,
  paramLiftFns: [],
  resultLowerFns: [_lowerFlatOwn.bind(null, 2)],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: null,
  getMemoryFn: () => null,
  getReallocFn: () => null,
  importFn: _trampoline12,
},
);
let trampoline13 = _trampoline13.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 13,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline13.manuallyAsync,
  paramLiftFns: [],
  resultLowerFns: [_lowerFlatOwn.bind(null, 3)],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: null,
  getMemoryFn: () => null,
  getReallocFn: () => null,
  importFn: _trampoline13,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 13,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline13.manuallyAsync,
  paramLiftFns: [],
  resultLowerFns: [_lowerFlatOwn.bind(null, 3)],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: null,
  getMemoryFn: () => null,
  getReallocFn: () => null,
  importFn: _trampoline13,
},
);
let trampoline14 = _trampoline14.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 14,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline14.manuallyAsync,
  paramLiftFns: [],
  resultLowerFns: [_lowerFlatOwn.bind(null, 3)],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: null,
  getMemoryFn: () => null,
  getReallocFn: () => null,
  importFn: _trampoline14,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 14,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline14.manuallyAsync,
  paramLiftFns: [],
  resultLowerFns: [_lowerFlatOwn.bind(null, 3)],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: null,
  getMemoryFn: () => null,
  getReallocFn: () => null,
  importFn: _trampoline14,
},
);
let trampoline15 = _trampoline15.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 15,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline15.manuallyAsync,
  paramLiftFns: [],
  resultLowerFns: [_lowerFlatU64],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: null,
  getMemoryFn: () => null,
  getReallocFn: () => null,
  importFn: _trampoline15,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 15,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline15.manuallyAsync,
  paramLiftFns: [],
  resultLowerFns: [_lowerFlatU64],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: null,
  getMemoryFn: () => null,
  getReallocFn: () => null,
  importFn: _trampoline15,
},
);
let trampoline16 = _trampoline16.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 16,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline16.manuallyAsync,
  paramLiftFns: [],
  resultLowerFns: [_lowerFlatTuple.bind(null, 34)],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline16,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 16,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline16.manuallyAsync,
  paramLiftFns: [],
  resultLowerFns: [_lowerFlatTuple.bind(null, 34)],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline16,
},
);
let trampoline17 = _trampoline17.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 17,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline17.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 2),_liftFlatU64],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatList({ elemLowerFn: _lowerFlatU8, typeIdx: 4 }), 12, 4, 4 ],[ 'err', _lowerFlatVariant([[ 'last-operation-failed', _lowerFlatOwn.bind(null, 1), 8, 4, 4 ],[ 'closed', null, 8, 4, 4 ],]), 12, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => realloc0,
  importFn: _trampoline17,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 17,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline17.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 2),_liftFlatU64],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatList({ elemLowerFn: _lowerFlatU8, typeIdx: 4 }), 12, 4, 4 ],[ 'err', _lowerFlatVariant([[ 'last-operation-failed', _lowerFlatOwn.bind(null, 1), 8, 4, 4 ],[ 'closed', null, 8, 4, 4 ],]), 12, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => realloc0,
  importFn: _trampoline17,
},
);
let trampoline18 = _trampoline18.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 18,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline18.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 3)],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatU64, 16, 8, 8 ],[ 'err', _lowerFlatVariant([[ 'last-operation-failed', _lowerFlatOwn.bind(null, 1), 8, 4, 4 ],[ 'closed', null, 8, 4, 4 ],]), 16, 8, 8 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline18,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 18,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline18.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 3)],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatU64, 16, 8, 8 ],[ 'err', _lowerFlatVariant([[ 'last-operation-failed', _lowerFlatOwn.bind(null, 1), 8, 4, 4 ],[ 'closed', null, 8, 4, 4 ],]), 16, 8, 8 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline18,
},
);
let trampoline19 = _trampoline19.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 19,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline19.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 3),_liftFlatList.bind(null, 4)],
  resultLowerFns: [_lowerFlatResult([[ 'ok', null, 12, 4, 4 ],[ 'err', _lowerFlatVariant([[ 'last-operation-failed', _lowerFlatOwn.bind(null, 1), 8, 4, 4 ],[ 'closed', null, 8, 4, 4 ],]), 12, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline19,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 19,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline19.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 3),_liftFlatList.bind(null, 4)],
  resultLowerFns: [_lowerFlatResult([[ 'ok', null, 12, 4, 4 ],[ 'err', _lowerFlatVariant([[ 'last-operation-failed', _lowerFlatOwn.bind(null, 1), 8, 4, 4 ],[ 'closed', null, 8, 4, 4 ],]), 12, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline19,
},
);
let trampoline20 = _trampoline20.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 20,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline20.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 3)],
  resultLowerFns: [_lowerFlatResult([[ 'ok', null, 12, 4, 4 ],[ 'err', _lowerFlatVariant([[ 'last-operation-failed', _lowerFlatOwn.bind(null, 1), 8, 4, 4 ],[ 'closed', null, 8, 4, 4 ],]), 12, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline20,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 20,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline20.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 3)],
  resultLowerFns: [_lowerFlatResult([[ 'ok', null, 12, 4, 4 ],[ 'err', _lowerFlatVariant([[ 'last-operation-failed', _lowerFlatOwn.bind(null, 1), 8, 4, 4 ],[ 'closed', null, 8, 4, 4 ],]), 12, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline20,
},
);
let trampoline21 = _trampoline21.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 21,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline21.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 6),_liftFlatU64],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatOwn.bind(null, 2), 8, 4, 4 ],[ 'err', _lowerFlatEnum.bind(null, 0), 8, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline21,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 21,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline21.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 6),_liftFlatU64],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatOwn.bind(null, 2), 8, 4, 4 ],[ 'err', _lowerFlatEnum.bind(null, 0), 8, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline21,
},
);
let trampoline22 = _trampoline22.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 22,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline22.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 6),_liftFlatU64],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatOwn.bind(null, 3), 8, 4, 4 ],[ 'err', _lowerFlatEnum.bind(null, 0), 8, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline22,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 22,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline22.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 6),_liftFlatU64],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatOwn.bind(null, 3), 8, 4, 4 ],[ 'err', _lowerFlatEnum.bind(null, 0), 8, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline22,
},
);
let trampoline23 = _trampoline23.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 23,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline23.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 6)],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatOwn.bind(null, 3), 8, 4, 4 ],[ 'err', _lowerFlatEnum.bind(null, 0), 8, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline23,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 23,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline23.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 6)],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatOwn.bind(null, 3), 8, 4, 4 ],[ 'err', _lowerFlatEnum.bind(null, 0), 8, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline23,
},
);
let trampoline24 = _trampoline24.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 24,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline24.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 6)],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatFlags.bind(null, 0), 2, 1, 1 ],[ 'err', _lowerFlatEnum.bind(null, 0), 2, 1, 1 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline24,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 24,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline24.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 6)],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatFlags.bind(null, 0), 2, 1, 1 ],[ 'err', _lowerFlatEnum.bind(null, 0), 2, 1, 1 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline24,
},
);
let trampoline25 = _trampoline25.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 25,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline25.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 6)],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatOwn.bind(null, 7), 8, 4, 4 ],[ 'err', _lowerFlatEnum.bind(null, 0), 8, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline25,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 25,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline25.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 6)],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatOwn.bind(null, 7), 8, 4, 4 ],[ 'err', _lowerFlatEnum.bind(null, 0), 8, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline25,
},
);
let trampoline26 = _trampoline26.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 26,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline26.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 6)],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatRecord.bind(null, [['type', _lowerFlatEnum.bind(null, 1), 96, 8 ],['linkCount', _lowerFlatU64, 96, 8 ],['size', _lowerFlatU64, 96, 8 ],['dataAccessTimestamp', _lowerFlatOption([[ 'some', _lowerFlatRecord.bind(null, [['seconds', _lowerFlatU64, 16, 8 ],['nanoseconds', _lowerFlatU32, 16, 8 ],]), 24, 8, 8 ],[ 'none', null, 24, 8, 8 ],]), 96, 8 ],['dataModificationTimestamp', _lowerFlatOption([[ 'some', _lowerFlatRecord.bind(null, [['seconds', _lowerFlatU64, 16, 8 ],['nanoseconds', _lowerFlatU32, 16, 8 ],]), 24, 8, 8 ],[ 'none', null, 24, 8, 8 ],]), 96, 8 ],['statusChangeTimestamp', _lowerFlatOption([[ 'some', _lowerFlatRecord.bind(null, [['seconds', _lowerFlatU64, 16, 8 ],['nanoseconds', _lowerFlatU32, 16, 8 ],]), 24, 8, 8 ],[ 'none', null, 24, 8, 8 ],]), 96, 8 ],]), 104, 8, 8 ],[ 'err', _lowerFlatEnum.bind(null, 0), 104, 8, 8 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline26,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 26,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline26.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 6)],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatRecord.bind(null, [['type', _lowerFlatEnum.bind(null, 1), 96, 8 ],['linkCount', _lowerFlatU64, 96, 8 ],['size', _lowerFlatU64, 96, 8 ],['dataAccessTimestamp', _lowerFlatOption([[ 'some', _lowerFlatRecord.bind(null, [['seconds', _lowerFlatU64, 16, 8 ],['nanoseconds', _lowerFlatU32, 16, 8 ],]), 24, 8, 8 ],[ 'none', null, 24, 8, 8 ],]), 96, 8 ],['dataModificationTimestamp', _lowerFlatOption([[ 'some', _lowerFlatRecord.bind(null, [['seconds', _lowerFlatU64, 16, 8 ],['nanoseconds', _lowerFlatU32, 16, 8 ],]), 24, 8, 8 ],[ 'none', null, 24, 8, 8 ],]), 96, 8 ],['statusChangeTimestamp', _lowerFlatOption([[ 'some', _lowerFlatRecord.bind(null, [['seconds', _lowerFlatU64, 16, 8 ],['nanoseconds', _lowerFlatU32, 16, 8 ],]), 24, 8, 8 ],[ 'none', null, 24, 8, 8 ],]), 96, 8 ],]), 104, 8, 8 ],[ 'err', _lowerFlatEnum.bind(null, 0), 104, 8, 8 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline26,
},
);
let trampoline27 = _trampoline27.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 27,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline27.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 6),_liftFlatFlags.bind(null, 1),_liftFlatStringUTF8,_liftFlatFlags.bind(null, 2),_liftFlatFlags.bind(null, 0)],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatOwn.bind(null, 6), 8, 4, 4 ],[ 'err', _lowerFlatEnum.bind(null, 0), 8, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline27,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 27,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline27.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 6),_liftFlatFlags.bind(null, 1),_liftFlatStringUTF8,_liftFlatFlags.bind(null, 2),_liftFlatFlags.bind(null, 0)],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatOwn.bind(null, 6), 8, 4, 4 ],[ 'err', _lowerFlatEnum.bind(null, 0), 8, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline27,
},
);
let trampoline28 = _trampoline28.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 28,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline28.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 6)],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatRecord.bind(null, [['lower', _lowerFlatU64, 16, 8 ],['upper', _lowerFlatU64, 16, 8 ],]), 24, 8, 8 ],[ 'err', _lowerFlatEnum.bind(null, 0), 24, 8, 8 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline28,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 28,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline28.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 6)],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatRecord.bind(null, [['lower', _lowerFlatU64, 16, 8 ],['upper', _lowerFlatU64, 16, 8 ],]), 24, 8, 8 ],[ 'err', _lowerFlatEnum.bind(null, 0), 24, 8, 8 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline28,
},
);
let trampoline29 = _trampoline29.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 29,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline29.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 6),_liftFlatFlags.bind(null, 1),_liftFlatStringUTF8],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatRecord.bind(null, [['lower', _lowerFlatU64, 16, 8 ],['upper', _lowerFlatU64, 16, 8 ],]), 24, 8, 8 ],[ 'err', _lowerFlatEnum.bind(null, 0), 24, 8, 8 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline29,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 29,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline29.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 6),_liftFlatFlags.bind(null, 1),_liftFlatStringUTF8],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatRecord.bind(null, [['lower', _lowerFlatU64, 16, 8 ],['upper', _lowerFlatU64, 16, 8 ],]), 24, 8, 8 ],[ 'err', _lowerFlatEnum.bind(null, 0), 24, 8, 8 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline29,
},
);
let trampoline30 = _trampoline30.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 30,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline30.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 7)],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatOption([[ 'some', _lowerFlatRecord.bind(null, [['type', _lowerFlatEnum.bind(null, 1), 12, 4 ],['name', _lowerFlatStringUTF8, 12, 4 ],]), 16, 4, 4 ],[ 'none', null, 16, 4, 4 ],]), 20, 4, 4 ],[ 'err', _lowerFlatEnum.bind(null, 0), 20, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => realloc0,
  importFn: _trampoline30,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 30,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline30.manuallyAsync,
  paramLiftFns: [_liftFlatBorrow.bind(null, 7)],
  resultLowerFns: [_lowerFlatResult([[ 'ok', _lowerFlatOption([[ 'some', _lowerFlatRecord.bind(null, [['type', _lowerFlatEnum.bind(null, 1), 12, 4 ],['name', _lowerFlatStringUTF8, 12, 4 ],]), 16, 4, 4 ],[ 'none', null, 16, 4, 4 ],]), 20, 4, 4 ],[ 'err', _lowerFlatEnum.bind(null, 0), 20, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => realloc0,
  importFn: _trampoline30,
},
);
let trampoline31 = _trampoline31.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 31,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline31.manuallyAsync,
  paramLiftFns: [],
  resultLowerFns: [_lowerFlatList({ elemLowerFn: _lowerFlatTuple.bind(null, 11), typeIdx: 5 })],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => realloc0,
  importFn: _trampoline31,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 31,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline31.manuallyAsync,
  paramLiftFns: [],
  resultLowerFns: [_lowerFlatList({ elemLowerFn: _lowerFlatTuple.bind(null, 11), typeIdx: 5 })],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => realloc0,
  importFn: _trampoline31,
},
);
let trampoline32 = _trampoline32.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 32,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline32.manuallyAsync,
  paramLiftFns: [],
  resultLowerFns: [_lowerFlatOption([[ 'some', _lowerFlatOwn.bind(null, 4), 8, 4, 4 ],[ 'none', null, 8, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline32,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 32,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline32.manuallyAsync,
  paramLiftFns: [],
  resultLowerFns: [_lowerFlatOption([[ 'some', _lowerFlatOwn.bind(null, 4), 8, 4, 4 ],[ 'none', null, 8, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline32,
},
);
let trampoline33 = _trampoline33.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 33,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline33.manuallyAsync,
  paramLiftFns: [],
  resultLowerFns: [_lowerFlatOption([[ 'some', _lowerFlatOwn.bind(null, 5), 8, 4, 4 ],[ 'none', null, 8, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline33,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 33,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline33.manuallyAsync,
  paramLiftFns: [],
  resultLowerFns: [_lowerFlatOption([[ 'some', _lowerFlatOwn.bind(null, 5), 8, 4, 4 ],[ 'none', null, 8, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline33,
},
);
let trampoline34 = _trampoline34.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 34,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline34.manuallyAsync,
  paramLiftFns: [],
  resultLowerFns: [_lowerFlatOption([[ 'some', _lowerFlatOwn.bind(null, 5), 8, 4, 4 ],[ 'none', null, 8, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline34,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 34,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline34.manuallyAsync,
  paramLiftFns: [],
  resultLowerFns: [_lowerFlatOption([[ 'some', _lowerFlatOwn.bind(null, 5), 8, 4, 4 ],[ 'none', null, 8, 4, 4 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline34,
},
);
let trampoline35 = _trampoline35.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 35,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline35.manuallyAsync,
  paramLiftFns: [],
  resultLowerFns: [_lowerFlatRecord.bind(null, [['seconds', _lowerFlatU64, 16, 8 ],['nanoseconds', _lowerFlatU32, 16, 8 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline35,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 35,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline35.manuallyAsync,
  paramLiftFns: [],
  resultLowerFns: [_lowerFlatRecord.bind(null, [['seconds', _lowerFlatU64, 16, 8 ],['nanoseconds', _lowerFlatU32, 16, 8 ],])],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => null,
  importFn: _trampoline35,
},
);
let trampoline36 = _trampoline36.manuallyAsync ? new WebAssembly.Suspending(_lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 36,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline36.manuallyAsync,
  paramLiftFns: [],
  resultLowerFns: [_lowerFlatList({ elemLowerFn: _lowerFlatTuple.bind(null, 32), typeIdx: 6 })],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => realloc0,
  importFn: _trampoline36,
},
)) : _lowerImportBackwardsCompat.bind(
null,
{
  trampolineIdx: 36,
  componentIdx: 0,
  isAsync: false,
  isManualAsync: _trampoline36.manuallyAsync,
  paramLiftFns: [],
  resultLowerFns: [_lowerFlatList({ elemLowerFn: _lowerFlatTuple.bind(null, 32), typeIdx: 6 })],
  funcTypeIsAsync: false,
  getCallbackFn: () => null,
  getPostReturnFn: () => null,
  isCancellable: false,
  memoryIdx: 0,
  getMemoryFn: () => memory0,
  getReallocFn: () => realloc0,
  importFn: _trampoline36,
},
);
Promise.all([module0, module1, module2]).catch(() => {});
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
realloc0 = exports1.cabi_realloc;

try {
  realloc0Async = WebAssembly.promising(exports1.cabi_realloc);
} catch(err) {
  realloc0Async = exports1.cabi_realloc;
}

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

try {
  postReturn0Async = WebAssembly.promising(exports1['cabi_post_pulseengine:rivet/adapter@0.1.0#id']);
} catch(err) {
  postReturn0Async = exports1['cabi_post_pulseengine:rivet/adapter@0.1.0#id'];
}

postReturn1 = exports1['cabi_post_pulseengine:rivet/adapter@0.1.0#supported-types'];

try {
  postReturn1Async = WebAssembly.promising(exports1['cabi_post_pulseengine:rivet/adapter@0.1.0#supported-types']);
} catch(err) {
  postReturn1Async = exports1['cabi_post_pulseengine:rivet/adapter@0.1.0#supported-types'];
}

postReturn2 = exports1['cabi_post_pulseengine:rivet/adapter@0.1.0#import'];

try {
  postReturn2Async = WebAssembly.promising(exports1['cabi_post_pulseengine:rivet/adapter@0.1.0#import']);
} catch(err) {
  postReturn2Async = exports1['cabi_post_pulseengine:rivet/adapter@0.1.0#import'];
}

postReturn3 = exports1['cabi_post_pulseengine:rivet/adapter@0.1.0#export'];

try {
  postReturn3Async = WebAssembly.promising(exports1['cabi_post_pulseengine:rivet/adapter@0.1.0#export']);
} catch(err) {
  postReturn3Async = exports1['cabi_post_pulseengine:rivet/adapter@0.1.0#export'];
}

postReturn4 = exports1['cabi_post_pulseengine:rivet/renderer@0.1.0#analyze'];

try {
  postReturn4Async = WebAssembly.promising(exports1['cabi_post_pulseengine:rivet/renderer@0.1.0#analyze']);
} catch(err) {
  postReturn4Async = exports1['cabi_post_pulseengine:rivet/renderer@0.1.0#analyze'];
}

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
    isManualAsync: false,
    entryFnName: 'adapter010Id',
    getCallbackFn: () => null,
    callbackFnName: 'null',
    errHandling: 'none',
    callingWasmExport: true,
  });
  
  const started = task.enterSync();
  task.setReturnMemoryIdx(0);
  task.setReturnMemory(memory0);
  let ret =   _withGlobalCurrentTaskMeta({
    taskID: task.id(),
    componentIdx: task.componentIdx(),
    fn: () => adapter010Id(),
  });
  
  var ptr0 = dataView(memory0).getUint32(ret + 0, true);
  var len0 = dataView(memory0).getUint32(ret + 4, true);
  var result0 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr0, len0));
  _debugLog('[iface="pulseengine:rivet/adapter@0.1.0", function="id"][Instruction::Return]', {
    funcName: 'id',
    paramCount: 1,
    async: false,
    postReturn: true
  });
  task.resolve([result0]);
  const retCopy = result0;
  
  let cstate = getOrCreateAsyncState(0);
  cstate.mayLeave = false;
  postReturn0(ret);
  cstate.mayLeave = true;
  task.exit();
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
    isManualAsync: false,
    entryFnName: 'adapter010Name',
    getCallbackFn: () => null,
    callbackFnName: 'null',
    errHandling: 'none',
    callingWasmExport: true,
  });
  
  const started = task.enterSync();
  task.setReturnMemoryIdx(0);
  task.setReturnMemory(memory0);
  let ret =   _withGlobalCurrentTaskMeta({
    taskID: task.id(),
    componentIdx: task.componentIdx(),
    fn: () => adapter010Name(),
  });
  
  var ptr0 = dataView(memory0).getUint32(ret + 0, true);
  var len0 = dataView(memory0).getUint32(ret + 4, true);
  var result0 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr0, len0));
  _debugLog('[iface="pulseengine:rivet/adapter@0.1.0", function="name"][Instruction::Return]', {
    funcName: 'name',
    paramCount: 1,
    async: false,
    postReturn: true
  });
  task.resolve([result0]);
  const retCopy = result0;
  
  let cstate = getOrCreateAsyncState(0);
  cstate.mayLeave = false;
  postReturn0(ret);
  cstate.mayLeave = true;
  task.exit();
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
    isManualAsync: false,
    entryFnName: 'adapter010SupportedTypes',
    getCallbackFn: () => null,
    callbackFnName: 'null',
    errHandling: 'none',
    callingWasmExport: true,
  });
  
  const started = task.enterSync();
  task.setReturnMemoryIdx(0);
  task.setReturnMemory(memory0);
  let ret =   _withGlobalCurrentTaskMeta({
    taskID: task.id(),
    componentIdx: task.componentIdx(),
    fn: () => adapter010SupportedTypes(),
  });
  
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
  task.resolve([result1]);
  const retCopy = result1;
  
  let cstate = getOrCreateAsyncState(0);
  cstate.mayLeave = false;
  postReturn1(ret);
  cstate.mayLeave = true;
  task.exit();
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
    isManualAsync: false,
    entryFnName: 'adapter010Import',
    getCallbackFn: () => null,
    callbackFnName: 'null',
    errHandling: 'throw-result-err',
    callingWasmExport: true,
  });
  
  const started = task.enterSync();
  task.setReturnMemoryIdx(0);
  task.setReturnMemory(memory0);
  let ret =   _withGlobalCurrentTaskMeta({
    taskID: task.id(),
    componentIdx: task.componentIdx(),
    fn: () => adapter010Import(ptr0, len0, result5, len5),
  });
  
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
  task.resolve([retCopy.val]);
  
  let cstate = getOrCreateAsyncState(0);
  cstate.mayLeave = false;
  postReturn2(ret);
  cstate.mayLeave = true;
  task.exit();
  
  
  
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
    isManualAsync: false,
    entryFnName: 'adapter010Export',
    getCallbackFn: () => null,
    callbackFnName: 'null',
    errHandling: 'throw-result-err',
    callingWasmExport: true,
  });
  
  const started = task.enterSync();
  task.setReturnMemoryIdx(0);
  task.setReturnMemory(memory0);
  let ret =   _withGlobalCurrentTaskMeta({
    taskID: task.id(),
    componentIdx: task.componentIdx(),
    fn: () => adapter010Export(result21, len21, result26, len26),
  });
  
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
  task.resolve([retCopy.val]);
  
  let cstate = getOrCreateAsyncState(0);
  cstate.mayLeave = false;
  postReturn3(ret);
  cstate.mayLeave = true;
  task.exit();
  
  
  
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
    isManualAsync: false,
    entryFnName: 'renderer010Render',
    getCallbackFn: () => null,
    callbackFnName: 'null',
    errHandling: 'throw-result-err',
    callingWasmExport: true,
  });
  
  const started = task.enterSync();
  task.setReturnMemoryIdx(0);
  task.setReturnMemory(memory0);
  let ret =   _withGlobalCurrentTaskMeta({
    taskID: task.id(),
    componentIdx: task.componentIdx(),
    fn: () => renderer010Render(ptr0, len0, result2, len2),
  });
  
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
  task.resolve([retCopy.val]);
  
  let cstate = getOrCreateAsyncState(0);
  cstate.mayLeave = false;
  postReturn3(ret);
  cstate.mayLeave = true;
  task.exit();
  
  
  
  if (typeof retCopy === 'object' && retCopy.tag === 'err') {
    throw new ComponentError(retCopy.val);
  }
  return retCopy.val;
  
}
let renderer010Analyze;

function analyze(arg0) {
  
  var encodeRes = _utf8AllocateAndEncode(arg0, realloc0, memory0);
  var ptr0= encodeRes.ptr;
  var len0 = encodeRes.len;
  
  _debugLog('[iface="pulseengine:rivet/renderer@0.1.0", function="analyze"][Instruction::CallWasm] enter', {
    funcName: 'analyze',
    paramCount: 2,
    async: false,
    postReturn: true,
  });
  const hostProvided = false;
  
  const [task, _wasm_call_currentTaskID] = createNewCurrentTask({
    componentIdx: 0,
    isAsync: false,
    isManualAsync: false,
    entryFnName: 'renderer010Analyze',
    getCallbackFn: () => null,
    callbackFnName: 'null',
    errHandling: 'throw-result-err',
    callingWasmExport: true,
  });
  
  const started = task.enterSync();
  task.setReturnMemoryIdx(0);
  task.setReturnMemory(memory0);
  let ret =   _withGlobalCurrentTaskMeta({
    taskID: task.id(),
    componentIdx: task.componentIdx(),
    fn: () => renderer010Analyze(ptr0, len0),
  });
  
  let variant10;
  switch (dataView(memory0).getUint8(ret + 0, true)) {
    case 0: {
      var len5 = dataView(memory0).getUint32(ret + 8, true);
      var base5 = dataView(memory0).getUint32(ret + 4, true);
      var result5 = [];
      for (let i = 0; i < len5; i++) {
        const base = base5 + i * 32;
        var ptr1 = dataView(memory0).getUint32(base + 0, true);
        var len1 = dataView(memory0).getUint32(base + 4, true);
        var result1 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr1, len1));
        var ptr2 = dataView(memory0).getUint32(base + 8, true);
        var len2 = dataView(memory0).getUint32(base + 12, true);
        var result2 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr2, len2));
        var ptr3 = dataView(memory0).getUint32(base + 16, true);
        var len3 = dataView(memory0).getUint32(base + 20, true);
        var result3 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr3, len3));
        var ptr4 = dataView(memory0).getUint32(base + 24, true);
        var len4 = dataView(memory0).getUint32(base + 28, true);
        var result4 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr4, len4));
        result5.push({
          severity: result1,
          message: result2,
          componentPath: result3,
          analysisName: result4,
        });
      }
      variant10= {
        tag: 'ok',
        val: result5
      };
      break;
    }
    case 1: {
      let variant9;
      switch (dataView(memory0).getUint8(ret + 4, true)) {
        case 0: {
          var ptr6 = dataView(memory0).getUint32(ret + 8, true);
          var len6 = dataView(memory0).getUint32(ret + 12, true);
          var result6 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr6, len6));
          variant9= {
            tag: 'parse-error',
            val: result6
          };
          break;
        }
        case 1: {
          var ptr7 = dataView(memory0).getUint32(ret + 8, true);
          var len7 = dataView(memory0).getUint32(ret + 12, true);
          var result7 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr7, len7));
          variant9= {
            tag: 'no-root',
            val: result7
          };
          break;
        }
        case 2: {
          var ptr8 = dataView(memory0).getUint32(ret + 8, true);
          var len8 = dataView(memory0).getUint32(ret + 12, true);
          var result8 = TEXT_DECODER_UTF8.decode(new Uint8Array(memory0.buffer, ptr8, len8));
          variant9= {
            tag: 'layout-error',
            val: result8
          };
          break;
        }
        default: {
          throw new TypeError('invalid variant discriminant for RenderError');
        }
      }
      variant10= {
        tag: 'err',
        val: variant9
      };
      break;
    }
    default: {
      throw new TypeError('invalid variant discriminant for expected');
    }
  }
  _debugLog('[iface="pulseengine:rivet/renderer@0.1.0", function="analyze"][Instruction::Return]', {
    funcName: 'analyze',
    paramCount: 1,
    async: false,
    postReturn: true
  });
  const retCopy = variant10;
  task.resolve([retCopy.val]);
  
  let cstate = getOrCreateAsyncState(0);
  cstate.mayLeave = false;
  postReturn4(ret);
  cstate.mayLeave = true;
  task.exit();
  
  
  
  if (typeof retCopy === 'object' && retCopy.tag === 'err') {
    throw new ComponentError(retCopy.val);
  }
  return retCopy.val;
  
}
adapter010Id = exports1['pulseengine:rivet/adapter@0.1.0#id'];
adapter010Name = exports1['pulseengine:rivet/adapter@0.1.0#name'];
adapter010SupportedTypes = exports1['pulseengine:rivet/adapter@0.1.0#supported-types'];
adapter010Import = exports1['pulseengine:rivet/adapter@0.1.0#import'];
adapter010Export = exports1['pulseengine:rivet/adapter@0.1.0#export'];
renderer010Render = exports1['pulseengine:rivet/renderer@0.1.0#render'];
renderer010Analyze = exports1['pulseengine:rivet/renderer@0.1.0#analyze'];
const adapter010 = {
  'export': _export,
  id: id,
  'import': _import,
  name: name,
  supportedTypes: supportedTypes,
  
};
const renderer010 = {
  analyze: analyze,
  render: render,
  
};

return { adapter: adapter010, renderer: renderer010, 'pulseengine:rivet/adapter@0.1.0': adapter010, 'pulseengine:rivet/renderer@0.1.0': renderer010,  };
})();
let promise, resolve, reject;
function runNext (value) {
  try {
    let done;
    do {
      ({ value, done } = gen.next(value));
    } while (!(value instanceof Promise) && !done);
    if (done) {
      if (resolve) return resolve(value);
      else return value;
    }
    if (!promise) promise = new Promise((_resolve, _reject) => (resolve = _resolve, reject = _reject));
    value.then(nextVal => done ? resolve() : runNext(nextVal), reject);
  }
  catch (e) {
    if (reject) reject(e);
    else throw e;
  }
}
const maybeSyncReturn = runNext(null);
return promise || maybeSyncReturn;
};
