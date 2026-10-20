"""Enable Interactive sites, run this file, then open Sites → :8080."""
from pathlib import Path
from lethetic_wire import Response, listen

Path("site").mkdir(exist_ok=True)
Path("site/index.html").write_text("""<!doctype html>
<html><head><meta charset="utf-8"><title>Python notebook</title>
<style>
body{font:18px system-ui;background:#161a26;color:#eef1ff;padding:24px}
h1{color:#9cd5ff}button,input{font:inherit;padding:8px;margin:8px;border-radius:8px}
output{display:block;margin:16px}form{margin-top:24px}
</style></head><body><h1>My Python notebook</h1>
<button id="hello">Call Python</button><output id="answer">Ready</output>
<form id="note"><label>Your note <input name="text" value="Hello from the iframe"></label>
<button>Save note</button></form><p id="saved"></p>
<script>
document.querySelector('#hello').onclick=async()=>{
  const response=await fetch('/api/hello?name=browser');
  document.querySelector('#answer').textContent=(await response.json()).message;
};
document.querySelector('#note').onsubmit=async event=>{
  event.preventDefault();
  const response=await fetch('/api/note',{method:'POST',body:document.querySelector('input').value});
  document.querySelector('#saved').textContent=await response.text();
};
</script></body></html>""", encoding="utf-8")


async def handle(request):
    if request.method == "GET" and request.path == "/api/hello":
        name = request.query.get("name", ["world"])[0]
        return Response.json({"message": f"Hello {name}, from Python!"})
    if request.method == "POST" and request.path == "/api/note":
        Path("note.txt").write_bytes(request.body)
        return Response("Saved note.txt: " + request.text())
    return Response("Not found", status=404)


listen(8080, handler=handle, web_root="site")
print("Site ready on virtual port 8080. Open Sites to interact with Python.")
