import asyncio
exec(open('cdp.py').read().split('async def main')[0])
async def main():
    async with websockets.connect(WS, max_size=None) as ws:
        c = C(ws)
        ctx = (await c("Target.createBrowserContext", disposeOnDetach=False))["browserContextId"]
        await c("Target.createTarget", url="https://accounts.google.com/ServiceLogin?Email=1cryptoicos@gmail.com&continue=https://www.google.com/maps", browserContextId=ctx, newWindow=True)
        open('ctx.txt','w').write(ctx); print(ctx)
asyncio.run(main())
