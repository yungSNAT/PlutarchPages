# Test data for the PDF/A-1b checks

`verapdf-1.30.4-pdfa-1b.tsv` and `pdfa-1b-agreement.tsv` are test fixtures for `../verapdf_agreement.rs`, an ignored test that needs a local checkout of the corpus below.

- Source: the file names and SHA-256 hashes of the `PDF_A-1b` folder of the veraPDF test corpus, <https://github.com/veraPDF/veraPDF-corpus>, commit `377728596647e40edf16003d68d620773ce96ec5`, by the veraPDF Consortium.
- Licence: Creative Commons Attribution 4.0 International (CC BY 4.0), <https://creativecommons.org/licenses/by/4.0/>; the legal code is in `LICENSE-CC-BY-4.0.txt` here.
- Changes: no corpus file is included. The fixtures list the corpus's file names with each file's hash, the expected verdict, the rule ids veraPDF 1.30.4 reported for it (27 Nov 2026) and, in `pdfa-1b-agreement.tsv`, how far this repository's checks agreed when last measured.
