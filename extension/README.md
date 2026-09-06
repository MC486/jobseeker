# jobseeker browser extension

Load this directory as an unpacked Manifest V3 extension.

1. Start `jobseeker serve` on loopback.
2. Chrome → Extensions → Developer mode → Load unpacked → `extension/`.
3. Pair the browser (needed when `auth.mode = token`; optional on loopback):

   ```bash
   jobseeker pair --name "Firefox on laptop"
   ```

   Paste the printed code into the popup and click **Pair this browser**.
   Or run `jobseeker pair --name "…" --emit-token` and paste the `jst_…` token.
4. Open a LinkedIn or Indeed posting you are already viewing.
5. Click the extension and **Capture this page**.

The extension ships the rendered DOM and any JSON-LD it finds. It does not parse
the posting. Improving extraction never requires shipping a new extension.

Device tokens are stored hashed on the server, scoped to ingest, and revocable
with `jobseeker tokens` / `jobseeker revoke <id>`.
