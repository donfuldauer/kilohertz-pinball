const g = globalThis;
const names = ['read','readFile','readbuffer','writeFile','print','printErr','putstr','performance','preciseTime','dateNow','scriptArgs','arguments','os','load','quit','gc','console','setTimeout','drainJobQueue','runJobs','$vm','Atomics','SharedArrayBuffer','WebAssembly'];
let out=[];
for (const n of names) { let t; try { t = typeof g[n]; } catch(e) { t='err'; } out.push(n+':'+t); }
print(out.join(' '));
try { print('performance.now:', typeof performance.now, performance.now()); } catch(e) { print('perf err', e); }
try { const b = read(g.scriptArgs ? scriptArgs[0] : arguments[0], 'binary'); print('read binary ->', Object.prototype.toString.call(b), b.length, b[0], b[1]); } catch(e) { print('read err', e); }
try { print('args', g.scriptArgs ? JSON.stringify(scriptArgs) : JSON.stringify(Array.from(arguments))); } catch(e) { print('args err', e); }
print('WA keys', Object.getOwnPropertyNames(WebAssembly).sort().join(','));
print('JSTag', typeof WebAssembly.JSTag);
