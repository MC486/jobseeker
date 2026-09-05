# jobseeker browser extension

Load this directory as an unpacked Manifest V3 extension.

1. Start `jobseeker serve` on loopback.
2. Chrome → Extensions → Developer mode → Load unpacked → `extension/`.
3. Open a LinkedIn or Indeed posting you are already viewing.
4. Click the extension and **Capture this page**.

The extension ships the rendered DOM and any JSON-LD it finds. It does not parse
the posting. Improving extraction never requires shipping a new extension.
