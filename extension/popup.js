const serverEl = document.getElementById("server");
const tokenEl = document.getElementById("token");
const codeEl = document.getElementById("code");
const statusEl = document.getElementById("status");

chrome.storage.local.get(["server", "token"], (stored) => {
  serverEl.value = stored.server || "http://127.0.0.1:8787";
  tokenEl.value = stored.token || "";
});

document.getElementById("pair").addEventListener("click", async () => {
  const server = serverEl.value.replace(/\/+$/, "");
  const code = codeEl.value.trim();
  if (!code) {
    statusEl.textContent = "Paste a pairing code from `jobseeker pair`.";
    return;
  }
  statusEl.textContent = "Pairing…";
  try {
    const res = await fetch(`${server}/api/v1/auth/pair`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ code }),
    });
    const body = await res.json();
    if (!res.ok) {
      statusEl.textContent = body.error?.message || res.statusText;
      return;
    }
    tokenEl.value = body.token;
    await chrome.storage.local.set({ server, token: body.token });
    codeEl.value = "";
    statusEl.textContent = `Paired as ${body.name}. Token stored on this device.`;
  } catch (err) {
    statusEl.textContent = err.message || String(err);
  }
});

document.getElementById("capture").addEventListener("click", async () => {
  const server = serverEl.value.replace(/\/+$/, "");
  const token = tokenEl.value.trim();
  await chrome.storage.local.set({ server, token });
  statusEl.textContent = "Capturing…";
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  if (!tab?.id) {
    statusEl.textContent = "No active tab.";
    return;
  }
  const [{ result }] = await chrome.scripting.executeScript({
    target: { tabId: tab.id },
    func: collectPage,
  });
  try {
    const headers = { "content-type": "application/json" };
    if (token) headers.authorization = `Bearer ${token}`;
    const res = await fetch(`${server}/api/v1/ingest/capture`, {
      method: "POST",
      headers,
      body: JSON.stringify(result),
    });
    const body = await res.json();
    if (!res.ok) {
      statusEl.textContent = body.error?.message || res.statusText;
      return;
    }
    statusEl.textContent = `Queued ${body.task_id}`;
  } catch (err) {
    statusEl.textContent = err.message || String(err);
  }
});

function collectPage() {
  const scripts = [...document.querySelectorAll('script[type="application/ld+json"]')]
    .map((s) => {
      try {
        return JSON.parse(s.textContent || "null");
      } catch {
        return null;
      }
    })
    .filter(Boolean);
  const root =
    document.querySelector("main") ||
    document.querySelector("[class*='job']") ||
    document.body;
  return {
    url: location.href,
    html: document.documentElement.outerHTML,
    text: root.innerText,
    selected_html: window.getSelection()?.toString()
      ? (() => {
          const range = window.getSelection().getRangeAt(0);
          const div = document.createElement("div");
          div.appendChild(range.cloneContents());
          return div.innerHTML;
        })()
      : null,
    page_meta: scripts.length === 1 ? scripts[0] : scripts,
    client: { name: "jobseeker-extension", version: "0.1.0", browser: navigator.userAgent },
  };
}
