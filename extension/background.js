chrome.runtime.onInstalled.addListener(() => {
  chrome.storage.local.get(["server"], (stored) => {
    if (!stored.server) {
      chrome.storage.local.set({ server: "http://127.0.0.1:8787" });
    }
  });
});
