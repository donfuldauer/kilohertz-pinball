const http=require('http'),fs=require('fs'),path=require('path');
const root=path.resolve(process.argv[2]),port=+process.argv[3];
const csp="default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
const mime={'.html':'text/html; charset=utf-8','.js':'text/javascript','.wasm':'application/wasm'};
http.createServer((q,r)=>{let p=path.normalize(path.join(root,decodeURIComponent(q.url.split('?')[0])));
 if(!p.startsWith(root)){r.writeHead(403).end();return}
 if(p===root||p===root+'/')p=path.join(root,'index.html');
 fs.readFile(p,(e,d)=>{const h={'Content-Security-Policy':csp,'Cache-Control':'no-store'};
  if(e){r.writeHead(404,h).end();return} h['Content-Type']=mime[path.extname(p)]||'application/octet-stream'; r.writeHead(200,h).end(d)})}).listen(port,'127.0.0.1');
