// The official A2A JavaScript SDK (@a2a-js/sdk) against Liaison, on both bindings: coqui-a2a's
// loopback endpoint over a proven link, and the gateway's own /v1/a2a with a bearer. Run by the
// ignored test `the_official_a2a_javascript_sdk_completes_exchanges_on_both_bindings`, which starts
// the gateway, a stand-in Agent and the loopback endpoint and passes their addresses here.
//
//   A2A_JS_SDK_DIR  a directory with node_modules/@a2a-js/sdk
//   LIAISON_LOOPBACK, LIAISON_LOOPBACK_TOKEN   the loopback endpoint and its token
//   LIAISON_HTTP, LIAISON_BEARER, LIAISON_ORIGIN   /v1/a2a/, a peer's bearer, the origin its card names
import { existsSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const need = (name) => {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is not set`);
  return value;
};
const sdkDir = resolve(need("A2A_JS_SDK_DIR"));
const sdkEntry = join(sdkDir, "node_modules", "@a2a-js", "sdk", "dist", "client", "index.js");
if (!existsSync(sdkEntry)) throw new Error(`@a2a-js/sdk is not installed under ${sdkDir}`);
const { ClientFactory, ClientFactoryOptions, DefaultAgentCardResolver, JsonRpcTransportFactory } = await import(
  pathToFileURL(sdkEntry)
);
const sdkVersion = JSON.parse(readFileSync(join(sdkDir, "node_modules", "@a2a-js", "sdk", "package.json"), "utf8")).version;
console.log(`@a2a-js/sdk ${sdkVersion}`);

let failed = 0;
const check = (name, ok, detail = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail ? `: ${detail}` : ""}`);
  if (!ok) failed += 1;
};

// A fetch that adds the credential and, for the gateway, does what its HTTPS reverse proxy would:
// the card names the public origin, and the request goes to the listener behind it.
const fetching = (authorization, origin, listener) => async (input, init) => {
  let url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
  if (origin && url.startsWith(origin)) url = listener + url.slice(origin.length);
  const headers = { ...Object.fromEntries(new Headers(init?.headers ?? {})), authorization };
  return fetch(url, { ...init, headers });
};

const request = (id, text) => ({
  message: {
    messageId: id,
    role: 1,
    parts: [{ content: { $case: "text", value: text } }],
    metadata: { addressee: "agent:G/007" },
  },
});

async function exchange(label, base, fetchImpl) {
  const factory = new ClientFactory(
    ClientFactoryOptions.createFrom(ClientFactoryOptions.default, {
      transports: [new JsonRpcTransportFactory({ fetchImpl })],
      cardResolver: new DefaultAgentCardResolver({ fetchImpl }),
    }),
  );
  const client = await factory.createFromUrl(base);
  const card = await client.getAgentCard();
  check(`${label}: the card was read`, card.name === "AI6 Liaison", JSON.stringify(card.skills?.map((skill) => skill.id)));

  const result = await client.sendMessage(request(`${label}-1`, `hello from the sdk on ${label}`));
  const answered = JSON.stringify(result);
  check(`${label}: sendMessage got the Agent's reply`, answered.includes(`re: hello from the sdk on ${label}`), answered.slice(0, 300));
  const taskId = result.taskId ?? result.task?.id ?? result.id;

  const events = [];
  for await (const event of client.sendMessageStream(request(`${label}-2`, `streamed on ${label}`))) {
    events.push(event);
  }
  const streamed = JSON.stringify(events);
  check(
    `${label}: sendMessageStream carried the reply and ended completed`,
    streamed.includes(`re: streamed on ${label}`) && /"state":(3|"TASK_STATE_COMPLETED")/.test(streamed),
    `${events.length} events`,
  );

  const again = await client.getTask({ id: taskId });
  const state = String(again.status?.state);
  check(`${label}: getTask follows the thread to completed`, state.includes("COMPLETED") || state === "3", state);
}

try {
  const loopback = need("LIAISON_LOOPBACK");
  await exchange("loopback", loopback, fetching(`Bearer ${need("LIAISON_LOOPBACK_TOKEN")}`));

  const http = need("LIAISON_HTTP");
  const origin = need("LIAISON_ORIGIN");
  const listener = new URL(http).origin;
  await exchange("http", http, fetching(`Bearer ${need("LIAISON_BEARER")}`, origin, listener));
  const unsigned = await fetch(new URL(".well-known/agent-card.json", http));
  check("http: no bearer is refused", unsigned.status === 401, String(unsigned.status));
} catch (error) {
  failed += 1;
  console.log(`FAIL ${error?.stack ?? error}`);
}
process.exit(failed ? 1 : 0);
