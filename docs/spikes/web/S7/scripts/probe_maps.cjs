// Runs one probe of the S1 wasm-bindgen module and then prints the guest's
// memory map (to find V8's JIT code regions in a qemu -d in_asm log).
// usage: node probe_maps.cjs <pkg_dir> <probe> <maps_out>
const fs = require('node:fs');
const [pkg, probe, out] = process.argv.slice(2);
const m = require(`${pkg}/s1_harness.js`);
console.log(`${probe}: ${m.probe(probe)}`);
console.log(`arch=${process.arch} host_arch=${process.config.variables.host_arch} target_arch=${process.config.variables.target_arch} v8=${process.versions.v8}`);
fs.writeFileSync(out, fs.readFileSync('/proc/self/maps'));
