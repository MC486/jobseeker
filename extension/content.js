// Present so the popup can also ask the content script later. Capture is
// click-initiated from the popup; this file does not scrape in the background.
chrome.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
  if (msg?.type === "ping") sendResponse({ ok: true, url: location.href });
});
