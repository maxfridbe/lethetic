"""Protocol tests; the browser acceptance suite also runs these APIs in real Pyodide."""
import asyncio
import base64
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "static" / "python"))
import lethetic_wire as wire


class PythonSitesTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.previous = os.getcwd()
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        os.chdir(self.root)
        wire._configure(False, str(self.root))

    def tearDown(self):
        wire._configure(False, str(self.root))
        os.chdir(self.previous)
        self.directory.cleanup()

    async def request(self, path="/", method="GET", body=b"", **kwargs):
        return json.loads(await wire._dispatch_json(json.dumps({
            "port": 8080, "path": path, "method": method,
            "body": base64.b64encode(body).decode(), **kwargs,
        })))

    def text(self, response):
        return base64.b64decode(response["body"]).decode()

    async def test_permission_registration_and_revocation(self):
        with self.assertRaisesRegex(PermissionError, "Interactive sites"):
            wire.listen(8080, web_root=".")
        wire._configure(True, str(self.root))
        site = wire.listen(8080, web_root=".")
        with self.assertRaisesRegex(ValueError, "already"):
            wire.listen(8080, web_root=".")
        self.assertEqual([{"port": 8080, "webRoot": "."}], json.loads(wire._sites_json()))
        site.stop()
        self.assertEqual(503, (await self.request())["status"])
        wire.listen(8080, web_root=".")
        wire._configure(False, str(self.root))
        self.assertEqual("[]", wire._sites_json())
        self.assertEqual(403, (await self.request())["status"])

    async def test_static_files_head_and_traversal(self):
        wire._configure(True, str(self.root))
        Path("site").mkdir()
        Path("site/index.html").write_text("<h1>Hello</h1>")
        Path("private.txt").write_text("private")
        Path("site/link.txt").symlink_to(self.root / "private.txt")
        wire.listen(8080, web_root="site")
        page = await self.request()
        self.assertEqual("<h1>Hello</h1>", self.text(page))
        self.assertIn("text/html", page["headers"]["Content-Type"][0])
        self.assertEqual("", (await self.request(method="HEAD"))["body"])
        for path in ["/../private.txt", "/%2e%2e/private.txt", "/a%5cb", "//elsewhere/", "/%00"]:
            self.assertEqual(400, (await self.request(path))["status"], path)
        self.assertEqual(404, (await self.request("/link.txt"))["status"])
        self.assertEqual(404, (await self.request("/missing"))["status"])
        self.assertEqual(405, (await self.request(method="TRACE"))["status"])

    async def test_async_handler_query_binary_and_registration_directory(self):
        wire._configure(True, str(self.root))
        Path("app").mkdir()
        os.chdir("app")

        async def handle(request):
            await asyncio.sleep(0)
            Path("saved.bin").write_bytes(request.body)
            return wire.Response.json({"zip": request.query["zip"], "cwd": os.getcwd()})

        wire.listen(8080, handler=handle)
        os.chdir(self.root)
        response = await self.request("/api?zip=63141&zip=90210", "POST", b"\x00\xff")
        self.assertEqual(200, response["status"])
        self.assertEqual(["63141", "90210"], json.loads(self.text(response))["zip"])
        self.assertEqual(b"\x00\xff", Path("app/saved.bin").read_bytes())
        self.assertEqual(str(self.root), os.getcwd())

    async def test_wsgi_environ_write_iterable_and_close(self):
        wire._configure(True, str(self.root))
        closed = []

        def app(env, start_response):
            self.assertEqual("/cafÃ©", env["PATH_INFO"])
            self.assertEqual("a=1", env["QUERY_STRING"])
            self.assertEqual("POST", env["REQUEST_METHOD"])
            self.assertEqual("application/json", env["CONTENT_TYPE"])
            self.assertEqual("5", env["CONTENT_LENGTH"])
            self.assertNotIn("HTTP_AUTHORIZATION", env)
            self.assertNotIn("HTTP_COOKIE", env)
            write = start_response("201 Created", [("Content-Type", "text/plain"), ("Set-Cookie", "secret=x")])
            write(b"prefix:")
            try:
                yield env["wsgi.input"].read()
            finally:
                closed.append(True)

        wire.listen(8080, app)
        response = await self.request("/caf%C3%A9?a=1", "POST", b"hello", headers={
            "Content-Type": ["application/json"], "Authorization": ["secret"], "Cookie": ["secret=x"],
        })
        self.assertEqual(201, response["status"])
        self.assertEqual("prefix:hello", self.text(response))
        self.assertNotIn("Set-Cookie", response["headers"])
        self.assertEqual([True], closed)

    async def test_fallback_errors_limits_and_timeout(self):
        wire._configure(True, str(self.root))
        Path("index.html").write_text("fallback")
        wire.listen(8080, handler=lambda request: wire.Response("missing", status=404), web_root=".")
        self.assertEqual("fallback", self.text(await self.request()))
        wire.stop(8080)
        wire.listen(8080, handler=lambda request: wire.Response(b"x" * (wire.MAX_BODY + 1)))
        self.assertEqual(500, (await self.request())["status"])
        self.assertEqual(413, (await self.request(method="POST", body=b"x" * (wire.MAX_BODY + 1)))["status"])
        self.assertEqual(400, (await self.request(headers={"Bad\nName": ["x"]}))["status"])
        self.assertEqual(431, (await self.request(headers={"X-Large": ["x" * 16385]}))["status"])
        wire.stop(8080)

        async def slow(request):
            await asyncio.sleep(1)

        wire.listen(8080, handler=slow)
        old = wire.REQUEST_TIMEOUT
        try:
            wire.REQUEST_TIMEOUT = 0.01
            self.assertEqual(504, (await self.request())["status"])
        finally:
            wire.REQUEST_TIMEOUT = old

    async def test_wsgi_validator_and_response_contract(self):
        from wsgiref.validate import validator
        wire._configure(True, str(self.root))

        def app(env, start_response):
            start_response("200 OK", [("Content-Type", "text/plain")])
            return [b"validated"]

        wire.listen(8080, validator(app))
        self.assertEqual("validated", self.text(await self.request()))
        wire.stop(8080)
        wire.listen(8080, handler=lambda request: wire.Response("bad", headers={"X-Test": "a\r\nb"}))
        self.assertEqual(500, (await self.request())["status"])

    async def test_saved_program_top_level_await_and_site_directory(self):
        wire._configure(True, str(self.root))
        Path("app").mkdir()
        Path("app/server.py").write_text(
            "import asyncio\nfrom pathlib import Path\n"
            "from lethetic_wire import listen, Response\n"
            "await asyncio.sleep(0)\n"
            "Path('output.txt').write_text('created')\n"
            "def handle(request):\n    return Response(Path('output.txt').read_text())\n"
            "listen(8080, handler=handle)\n"
        )
        await wire.run_file("app/server.py")
        self.assertEqual(str(self.root), os.getcwd())
        self.assertEqual("created", self.text(await self.request()))


if __name__ == "__main__":
    unittest.main()
