# OCR test data

`arabic-scan.png` is a 200 dpi greyscale 'scan' of six lines of public-domain Arabic: two verses
of al-Mutanabbi (10th century) and two traditional proverbs. The text is `SCAN_TEXT` in
`src/arabic.rs`, one line per line. It was made from that text, saved as `arabic-scan.txt`, with:

```sh
pango-view --no-display --font="Noto Naskh Arabic 14" --dpi=200 --margin=60 --rtl --align=right \
  --background=white --foreground=black -o raw.png arabic-scan.txt
magick raw.png -colorspace Gray -background white -rotate 1 -blur 0x0.6 \
  -attenuate 0.25 +noise Gaussian -depth 8 -strip -define png:compression-level=9 arabic-scan.png
```

Noto Naskh Arabic is OFL-1.1; no font file is part of the image or this repository.
