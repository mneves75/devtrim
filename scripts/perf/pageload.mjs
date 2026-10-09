#!/usr/bin/env node
// Measure cold page loads of the shipped HTML pages in headless Chrome over the
// DevTools protocol, with no dependency beyond Node's built-in WebSocket.
//
// usage: node scripts/perf/pageload.mjs <chrome> <base-url> <runs> <budget-ms> <page>...
//
// Every sample opens a fresh tab with the HTTP cache disabled, so it is a cold
// load of the page and every subresource it requests; one unmeasured load per
// page and viewport warms the browser process first. For each page at a
// desktop and a phone-sized viewport it reports the median, p95 and maximum
// of loadEventEnd and of largest contentful paint (milliseconds from
// navigation start) and the bytes transferred, with every raw sample.
// After load, image decode and font readiness, LCP must remain unchanged for
// 250 ms within a 5-second observation deadline. Later dynamic content is
// outside this bounded measurement; this is not a page-lifetime LCP metric.
//
// It fails closed: a sample counts only when the document and every
// subresource loaded, every image decoded, no Content-Security-Policy
// violation fired, and both timings are finite and positive. The run exits 1
// when any page's median or p95 of either timing reaches the budget, and 2 on
// a broken sample or measurement error. These are local numbers — loopback
// server, no network or CPU throttling — not what a visitor on a real network
// sees.
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const [chrome, base, runsText, budgetText, ...pages] = process.argv.slice(2);
const runs = Number(runsText);
const budget = Number(budgetText);
if (!chrome || !base || !Number.isInteger(runs) || runs < 1 || !(budget > 0) || pages.length === 0) {
  console.error("usage: pageload.mjs <chrome> <base-url> <runs> <budget-ms> <page>...");
  process.exit(2);
}
const VIEWPORTS = [
  { name: "desktop", width: 1280, height: 800, mobile: false, deviceScaleFactor: 1 },
  { name: "mobile", width: 390, height: 844, mobile: true, deviceScaleFactor: 3 },
];
const STEP_DEADLINE_MS = 15_000;
const PAINT_DEADLINE_MS = 5_000;
const PAINT_QUIET_MS = 250;

const profile = mkdtempSync(join(tmpdir(), "devtrim-pageload-"));
const browser = spawn(chrome, [
  "--headless=new",
  "--remote-debugging-port=0",
  `--user-data-dir=${profile}`,
  "--no-first-run",
  "--no-default-browser-check",
  "--disable-extensions",
  "--disable-background-networking",
  "--disable-component-update",
  "about:blank",
], { stdio: "ignore" });
let browserError;
browser.on("error", (error) => { browserError = error; });

// Chrome keeps writing its profile until it exits, so remove it only after exit.
async function cleanup() {
  if (browser.pid !== undefined && browser.exitCode === null && browser.signalCode === null) {
    const exited = new Promise((resolve) => browser.once("exit", resolve));
    browser.kill();
    await exited;
  }
  rmSync(profile, { recursive: true, force: true });
}

function deadline(promise, what) {
  let timer;
  const expired = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(`${what} did not finish within ${STEP_DEADLINE_MS} ms`)), STEP_DEADLINE_MS);
  });
  return Promise.race([promise, expired]).finally(() => clearTimeout(timer));
}

async function endpoint() {
  for (let attempt = 0; attempt < 150; attempt++) {
    if (browserError) throw new Error(`Chrome could not start: ${browserError.message}`);
    if (browser.exitCode !== null || browser.signalCode !== null) {
      throw new Error(`Chrome exited before connecting (${browser.signalCode ?? browser.exitCode})`);
    }
    try {
      const [port, path] = readFileSync(join(profile, "DevToolsActivePort"), "utf8").trim().split("\n");
      return `ws://127.0.0.1:${port}${path}`;
    } catch {
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  }
  throw new Error("Chrome did not publish a DevTools endpoint");
}

function connect(url) {
  const socket = new WebSocket(url);
  let next = 0;
  const pending = new Map();
  const listeners = new Set();
  socket.onmessage = ({ data }) => {
    const message = JSON.parse(data);
    if (message.id !== undefined && pending.has(message.id)) {
      const { resolve, reject } = pending.get(message.id);
      pending.delete(message.id);
      if (message.error) reject(new Error(message.error.message));
      else resolve(message.result);
    } else {
      for (const listener of listeners) listener(message);
    }
  };
  const send = (method, params = {}, sessionId) =>
    deadline(new Promise((resolve, reject) => {
      const id = ++next;
      pending.set(id, { resolve, reject });
      socket.send(JSON.stringify({ id, method, params, sessionId }));
    }), method);
  const once = (method, sessionId) =>
    deadline(new Promise((resolve) => {
      const listener = (message) => {
        if (message.method === method && message.sessionId === sessionId) {
          listeners.delete(listener);
          resolve(message.params);
        }
      };
      listeners.add(listener);
    }), method);
  return new Promise((resolve, reject) => {
    socket.onopen = () => resolve({ send, once, close: () => socket.close() });
    socket.onerror = () => reject(new Error("DevTools connection failed"));
  });
}

// Runs before any page script; CDP-injected scripts are not subject to the
// page's CSP, so they can observe the violations it reports.
const OBSERVE = `
  window.__lcp = 0;
  window.__lcpElementId = null;
  window.__csp = [];
  window.__paintWaiter = null;
  new PerformanceObserver((list) => {
    for (const entry of list.getEntries()) {
      window.__lcp = entry.startTime;
      window.__lcpElementId = entry.id || null;
    }
    if (window.__lcp > 0) window.__paintWaiter?.();
  }).observe({ type: "largest-contentful-paint", buffered: true });
  document.addEventListener("securitypolicyviolation", (event) => {
    window.__csp.push(event.violatedDirective + " " + event.blockedURI);
  });
`;

const READ = `(async () => {
  const images = [...document.images].filter((image) => image.loading !== "lazy" || image.complete);
  const undecoded = [];
  for (const image of images) {
    try { await image.decode(); } catch { undecoded.push(image.currentSrc || image.src); }
    if (image.naturalWidth === 0) undecoded.push(image.currentSrc || image.src);
  }
  await document.fonts.ready;
  // A first paint can precede a larger candidate. Require a quiet observation
  // window after readiness; a blank or continuously changing page times out.
  const paintSettled = await new Promise((resolve) => {
    let quietTimer;
    const finish = (settled) => {
      clearTimeout(quietTimer);
      clearTimeout(timer);
      window.__paintWaiter = null;
      resolve(settled);
    };
    const timer = setTimeout(() => finish(false), ${PAINT_DEADLINE_MS});
    window.__paintWaiter = () => {
      clearTimeout(quietTimer);
      quietTimer = setTimeout(() => finish(true), ${PAINT_QUIET_MS});
    };
    if (window.__lcp > 0) window.__paintWaiter();
  });
  const [navigation] = performance.getEntriesByType("navigation");
  const resources = performance.getEntriesByType("resource");
  return JSON.stringify({
    status: navigation.responseStatus,
    load: navigation.loadEventEnd,
    lcp: window.__lcp,
    lcp_element_id: window.__lcpElementId,
    paintSettled,
    visibility: document.visibilityState,
    bytes: navigation.transferSize + resources.reduce((sum, r) => sum + r.transferSize, 0),
    failed: resources.filter((r) => r.responseStatus >= 400 || r.responseStatus === 0).map((r) => r.name),
    undecoded,
    csp: window.__csp,
  });
})()`;

async function sample(cdp, url, viewport) {
  const { targetId } = await cdp.send("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await cdp.send("Target.attachToTarget", { targetId, flatten: true });
  try {
    await cdp.send("Network.enable", {}, sessionId);
    await cdp.send("Network.setCacheDisabled", { cacheDisabled: true }, sessionId);
    await cdp.send("Emulation.setDeviceMetricsOverride", viewport, sessionId);
    await cdp.send("Page.enable", {}, sessionId);
    await cdp.send("Page.bringToFront", {}, sessionId);
    await cdp.send("Page.addScriptToEvaluateOnNewDocument", { source: OBSERVE }, sessionId);
    const loaded = cdp.once("Page.loadEventFired", sessionId);
    await cdp.send("Page.navigate", { url }, sessionId);
    await loaded;
    const { result } = await cdp.send(
      "Runtime.evaluate",
      { expression: READ, returnByValue: true, awaitPromise: true },
      sessionId,
    );
    return JSON.parse(result.value);
  } finally {
    await cdp.send("Target.closeTarget", { targetId });
  }
}

function broken(s) {
  const reasons = [];
  if (s.status !== 200) reasons.push(`document status ${s.status}`);
  if (s.failed.length) reasons.push(`failed requests ${s.failed.join(", ")}`);
  if (s.undecoded.length) reasons.push(`undecoded images ${s.undecoded.join(", ")}`);
  if (s.csp.length) reasons.push(`CSP violations ${s.csp.join("; ")}`);
  if (!(Number.isFinite(s.load) && s.load > 0)) reasons.push(`load ${s.load}`);
  if (!(Number.isFinite(s.lcp) && s.lcp > 0)) reasons.push(`no LCP observed (${s.lcp})`);
  if (s.visibility !== "visible") reasons.push(`document visibility ${s.visibility}`);
  if (!s.paintSettled) reasons.push(`LCP did not settle within ${PAINT_DEADLINE_MS} ms`);
  return reasons;
}

function percentile(values, fraction) {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.ceil(fraction * sorted.length) - 1)];
}

const round = (value) => Math.round(value * 10) / 10;
const summary = (values) => ({
  median: round(percentile(values, 0.5)),
  p95: round(percentile(values, 0.95)),
  max: round(Math.max(...values)),
});

let exitCode = 0;
try {
  const cdp = await connect(await deadline(endpoint(), "Chrome start"));
  const report = [];
  for (const page of pages) {
    for (const viewport of VIEWPORTS) {
      const url = new URL(page, base).href;
      await sample(cdp, url, viewport); // warm the browser process, not the cache
      const samples = [];
      for (let run = 0; run < runs; run++) {
        const measured = await sample(cdp, url, viewport);
        const reasons = broken(measured);
        if (reasons.length) {
          throw new Error(`${url} (${viewport.name}) did not load correctly: ${reasons.join("; ")}`);
        }
        samples.push(measured);
      }
      const load = summary(samples.map((s) => s.load));
      const lcp = summary(samples.map((s) => s.lcp));
      const over = [load.median, load.p95, lcp.median, lcp.p95].some((value) => value >= budget);
      if (over) exitCode = Math.max(exitCode, 1);
      report.push({
        page,
        viewport: viewport.name,
        runs,
        budget_ms: budget,
        within_budget: !over,
        load_ms: load,
        lcp_ms: lcp,
        bytes: samples[0].bytes,
        samples: samples.map((s) => ({
          load: round(s.load),
          lcp: round(s.lcp),
          lcp_element_id: s.lcp_element_id,
        })),
      });
    }
  }
  cdp.close();
  console.log(JSON.stringify(report, null, 2));
} catch (error) {
  console.error(`pageload: ${error.message}`);
  exitCode = 2;
} finally {
  await cleanup();
}
process.exit(exitCode);
