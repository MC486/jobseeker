# ADR-0008 — Typst for resume rendering

**Status:** accepted · **Date:** 2026-09-05

## Context

Generated resumes and cover letters need to become PDFs that look professional, extract
cleanly as text (ATS parsing), fit precisely to a page budget, and be restyleable by the user
without touching Rust.

## Decision

Typst is the renderer. Templates live in `templates/resume/*.typ` and receive a JSON data
model. Rendering is optional: with `resume.renderer = "none"` the system emits Typst/Markdown
source and skips the PDF (FR-G-04).

## Rationale

- **Precise, scriptable typography.** Page-budget fitting is a hard constraint in resume
  generation ("one page, maximum coverage"). Typst can measure and conditionally adjust
  because templates are a real programming language, not a stylesheet.
- **Single ~30 MB binary, no ecosystem install.** Compare with a TeX distribution
  (multi-gigabyte) or headless Chrome (~150 MB plus a sandbox story).
- **Fast.** Sub-second compiles, so regenerating to compare variants is interactive.
- **Clean text extraction.** Typst PDFs contain real text in a sane reading order, which is
  what ATS parsers need. An `ats.typ` template that avoids tables, multi-column layout, and
  icons is straightforward to write.
- **Templates are user-editable** without recompiling the app, and the data-model boundary
  (JSON in, PDF out) keeps `jobseeker-resume` free of layout concerns.
- **Optional dependency.** A missing `typst` binary degrades to source output instead of
  breaking generation, which keeps the core product working on a minimal install.

## Alternatives considered

| Option | Why not |
|---|---|
| HTML + headless Chrome | Familiar and flexible. Rejected: a browser dependency on the server, slow startup, fragile print CSS for page fitting, and text extraction quality varies with the CSS used. |
| LaTeX | Best-in-class typography and a wealth of resume classes. Rejected on install weight, compile speed, and the difficulty of programmatic length fitting. |
| DOCX generation | Some employers demand `.docx`, and it is editable by the user. Rejected as primary: layout control is poor and the format is painful to generate well. Worth adding as an export later. |
| Markdown → Pandoc | Another heavyweight external dependency, and no precise page control. |
| Pure-Rust PDF (`printpdf`, `genpdf`) | No external dependency at all, which is attractive. Rejected: we would be reimplementing layout, and resume typography is exactly where hand-rolled layout looks amateur. |

## Consequences

- **An external binary is needed for PDFs.** Documented in deployment; detected at startup
  and reported in `/api/v1/meta` so the UI can explain why PDF export is unavailable.
- **Template authors must learn Typst.** Smaller cost than LaTeX, and the shipped templates
  cover the common cases.
- **`.docx` is not supported initially**, which will matter for some applications. Recorded
  in the backlog.
- **Page-budget fitting still needs iteration** (render, measure, adjust) rather than being
  solved analytically. Bounded to a couple of compile passes, which is fast enough to be
  invisible.
