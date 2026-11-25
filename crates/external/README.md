# pdfcraft-external

Layer L4, native only (the engine leaves it out of the web build). Optional programs the user
installed separately, used by PlutarchPages for the features that need them:

- `office_to_pdf`: Office files (doc, docx, odt, rtf, xls, xlsx, ods, ppt, pptx, odp; no
  macro-enabled formats) through LibreOffice's `soffice --headless --convert-to pdf`. LibreOffice
  older than 26.2.5 is refused (`MIN_LIBREOFFICE`): earlier releases have document-parser
  memory-safety bugs fixed in 26.2.5 (for example CVE-2026-4430 and CVE-2026-8357).
- `html_to_pdf`: a local HTML file through Chrome, Edge or Chromium (`--headless=new
  --print-to-pdf`), with scripts off and the browser told to refuse every network request (unknown host names and a
  dead proxy, loopback included).
- `url_to_pdf`: a web page (`checked_web_url`: http or https, no user name or password, no
  address naming this computer or a private network in any numeric spelling), scripts and network
  on. The check covers the address typed, not where it leads: a public name that resolves to a
  private address, a redirect, or a page that loads from the local network still reaches it.
- `Speech`: Read Out Loud through the system's speech program, text on standard input only:
  Windows PowerShell with a fixed encoded System.Speech script, macOS `say -f -` (with `[[` broken
  up), or `espeak-ng --stdin` on Linux. Speech Dispatcher's `spd-say` is not used (its pipe mode
  reads lines starting `!-!` as commands).

Each conversion runs in a private `tempfile` folder: a fixed input name (`in/input.<ext>`), a fresh
browser or LibreOffice profile, home, cache and temporary folders inside it, absolute paths only,
and the target after `--` so it can't be read as a switch. The program runs in its own process
group with a deadline (120 s) and a cancel flag. When a run ends, its process group is ended
(`killpg` through rustix on Unix, after the program exits but before it is reaped, so the group id
can't have been reused) or its process tree (`taskkill /T /F` on Windows, while the program runs).
This is best effort: a descendant that leaves the group, or outlives its Windows parent, can keep
running. The program gets an allowlisted environment (system, locale and font variables, whose
values may still point outside the stage; no credentials, proxies or session addresses). A failed
exit is an error. The result's size is checked every 100 ms while the program runs (a fast
program can briefly write more before it is stopped), and the result is read once (not through a
link on Unix), at most 512 MB, and must start like a PDF and end with `%%EOF`; the engine then
parses it before showing it.

LibreOffice's fresh profile turns macros off, blocks untrusted links and sets Writer and Calc never
to update linked content (sections, files, external data) on load. Impress has no setting for
linked content. Upstream converted test documents whose linked sections, pictures and external
cell references point at a local listener, and saw no connection with or without the Writer and
Calc settings (LibreOffice 26.8.1, Windows, 26 October 2026), so the settings are a second line of
defence rather than a measured fix.

This is not an operating-system sandbox. A converter that a hostile file compromises runs with the
user's rights, and a local HTML file can still include other local files it names (as a browser
would). The engine therefore keeps conversion off until the user turns it on (Preferences ▸ External
converters, or `PDFCRAFT_EXTERNAL_CONVERTERS=1` for the CLI and MCP server); the programs are
located from `PDFCRAFT_CHROME` / `PDFCRAFT_SOFFICE`, the usual install folders, then `PATH`.

The programs are never linked, copied, bundled or downloaded (AGENTS.md §2).
