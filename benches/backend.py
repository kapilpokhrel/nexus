# pip install aiohttp
from aiohttp import web

async def handler(request):
    return web.Response(
        text="Hello from backend server on port 8000\n",
        content_type="text/plain",
    )

app = web.Application()
app.router.add_route("*", "/", handler)
app.router.add_route("*", "/{tail:.*}", handler)

web.run_app(app, host="0.0.0.0", port=8000)
