/** Exercise the real interpreter and shared relay adapter, not a Python stub. */
export async function testPythonNetwork(evaluate: (expression: string) => Promise<unknown>): Promise<void> {
  await evaluate(`(async()=>{
    const {createPythonTool}=await import('./src/sdk.js');
    const signal=()=>new AbortController().signal;
    const denied=createPythonTool({siteAccess:true});
    const run=code=>denied.execute({code},signal());
    try {
      const output=await run('import numpy as np\\nprint(np.arange(3).sum())');
      if(output.trim().split(String.fromCharCode(10)).at(-1)!=='3')throw Error('Bundled numpy must work with networking disabled: '+output);
      const request='from pyodide.http import pyfetch\\nawait pyfetch('+JSON.stringify(location.origin+'/v1/models')+')';
      if(!(await run(request)).includes('Network access is disabled'))throw Error('pyfetch bypassed disabled networking');
      const jsfetch='from js import fetch\\nawait fetch('+JSON.stringify(location.origin+'/v1/models')+')';
      if(!(await run(jsfetch)).includes('Network access is disabled'))throw Error('JS fetch bypassed disabled networking');
      for(const code of ['from js import XMLHttpRequest\\nXMLHttpRequest.new()','from js import WebSocket\\nWebSocket.new("wss://example.org")'])
        if(!(await run(code)).includes('Network access is disabled'))throw Error('Browser API bypass: '+code);
      const install=await run('import micropip\\nawait micropip.install("flask==3.1.2")');
      if(!install.includes('Network access is disabled'))throw Error('Additional packages bypassed permission: '+install);
      await run('from lethetic_wire import listen, Response\\nasync def handler(request):\\n    if request.path == "/local": return Response.json({"ok":True})\\n    response=await pyfetch('+JSON.stringify(location.origin+'/v1/models')+')\\n    return Response.json(await response.json())\\nlisten(8090, handler=handler)');
      const local=await denied.requestSite({port:8090,method:'GET',path:'/local'},signal());
      if(local.status!==200)throw Error('Network denied local Python route');
      const external=await denied.requestSite({port:8090,method:'GET',path:'/external'},signal());
      if(external.status!==500 || !atob(external.body).includes('Network access is disabled'))throw Error('Site handler bypassed permission: '+JSON.stringify(external));
    } finally {denied.reset()}
    const relay={endpoint:location.origin+'/python-relay',token:'python-test-relay-token-123456789'};
    const tool=createPythonTool({siteAccess:true,networkAccess:true,relay});
    try {
      const code='from pyodide.http import pyfetch\\nfrom lethetic_wire import listen, Response\\nasync def handler(request):\\n    response=await pyfetch("https://python-relay-target.invalid/api", method="PATCH", headers={"X-Target":"ok"}, body="payload")\\n    return Response.json(await response.json())\\nlisten(8090, handler=handler)';
      const started=await tool.execute({code},signal());
      if(!tool.sites().length)throw Error(started);
      const response=await tool.requestSite({port:8090,method:'GET',path:'/'},signal());
      const data=JSON.parse(atob(response.body));
      if(response.status!==200 || data.method!=='PATCH' || data.url!=='https://python-relay-target.invalid/api' || atob(data.bodyBase64)!=='payload' || data.headers['x-target']!=='ok' || data.headers.authorization)throw Error('Relay request failed: '+JSON.stringify(response));
      const xhr=await tool.execute({code:'from js import XMLHttpRequest\\nXMLHttpRequest.new()'},signal());
      if(!xhr.includes('async HTTP fetch only'))throw Error('Relay XHR bypass');
      const failure=await tool.execute({code:'await pyfetch("https://python-relay-target.invalid/failure")'},signal());
      if(!failure.includes('HTTP relay failed (502)') || !failure.includes('DNS lookup failed') || failure.includes(relay.token))throw Error('Python lost or leaked relay details: '+failure);

    } finally {tool.reset()}
    for(const token of ['', 'incorrect-relay-token-123456789']) {
      const bad=createPythonTool({networkAccess:true,relay:{...relay,token}});
      try {
        const output=await bad.execute({code:'from pyodide.http import pyfetch\\nr=await pyfetch('+JSON.stringify(location.origin+'/v1/models')+')\\nprint(await r.string())'},signal());
        if(!output.includes(token?'HTTP relay failed (401)':'Relay token is missing'))throw Error('Relay silently fell back: '+output);
      } finally {bad.reset()}
    }
  })()`);
}
