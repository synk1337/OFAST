// Developer tool: drives the running Studio's page from a script, for checking the UI without clicking by hand.
//
// Start the Studio with WebView2's remote debugging port open, then evaluate JavaScript in its page or save a
// screenshot of it:
//
//   cd apps/studio
//   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222" npm run app
//   node tools/studio/cdp.mjs "document.querySelector('.content').innerText"
//   node tools/studio/cdp.mjs "document.querySelector('.demos tbody tr').click()"
//   node tools/studio/cdp.mjs --shot studio.png
//
// It speaks the Chrome DevTools Protocol (Runtime.evaluate, Page.captureScreenshot) over Node's own WebSocket
// (Node 22 or newer). The expression may return a promise: it is awaited, and its value printed as JSON.
const port = 9222;
const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const page = targets.find((t) => t.type === "page");
if (!page) throw new Error("no page target");
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((ok, fail) => {
  ws.onopen = ok;
  ws.onerror = fail;
});
let id = 0;
const pending = new Map();
ws.onmessage = (m) => {
  const msg = JSON.parse(m.data);
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg);
    pending.delete(msg.id);
  }
};
const send = (method, params = {}) =>
  new Promise((ok) => {
    const n = ++id;
    pending.set(n, ok);
    ws.send(JSON.stringify({ id: n, method, params }));
  });

const args = process.argv.slice(2);
if (args[0] === "--shot") {
  const fs = await import("node:fs");
  const r = await send("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(args[1], Buffer.from(r.result.data, "base64"));
  console.log("saved", args[1]);
} else {
  const r = await send("Runtime.evaluate", { expression: args[0], awaitPromise: true, returnByValue: true });
  console.log(JSON.stringify(r.result?.result?.value ?? r.result, null, 1));
}
ws.close();
