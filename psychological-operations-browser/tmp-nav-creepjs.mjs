import net from "node:net";
const code = `(() => { location.assign("https://abrahamjuliot.github.io/creepjs/"); return { navigating: true }; })()`;
const msg = JSON.stringify({ type: "eval", code }) + "\n";
const client = net.connect("\\\\.\\pipe\\psyops_browser_stdin");
client.on("connect", () => client.end(msg));
client.on("error", (e) => { console.error(e.message); process.exit(1); });
