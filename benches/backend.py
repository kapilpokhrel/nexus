# pip install aiohttp
from aiohttp import web
import sys

async def handler(request):
    return web.Response(
        text="Hello from backend server on port 8000\n",
        content_type="text/plain",
    )

app = web.Application()
app.router.add_route("*", "/", handler)
app.router.add_route("*", "/{tail:.*}", handler)

port = int(sys.argv[1]) if len(sys.argv) > 1 else 8000
web.run_app(app, host="0.0.0.0", port=port)
