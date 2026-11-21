// resolution of clocks and writeFile semantics
if (typeof preciseTime === 'function') { let a=preciseTime(), b; do { b=preciseTime(); } while (b===a); print('preciseTime step us', (b-a)*1e6); }
{ let a=performance.now(), b; do { b=performance.now(); } while (b===a); print('performance.now step ms', b-a); }
if (typeof writeFile === 'function') { const u = new Uint8Array([104,105,10,0xe2,0x82,0xac,10]); print('writeFile ret', writeFile('${WEB_SPIKES}/S10/logs/wf_jsc.bin', u)); }
if (typeof os === 'object') { const u = new Uint8Array([104,105,10,0xe2,0x82,0xac,10]); os.file.writeTypedArrayToFile('${WEB_SPIKES}/S10/logs/wf_sm.bin', u); print('sm write ok'); }
print('euro: €');
