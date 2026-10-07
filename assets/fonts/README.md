# Embedded fonts

- `DejaVuSansMono.ttf`: unmodified DejaVu Sans Mono, from Ubuntu's
  `fonts-dejavu-core` package. Copyright (c) 2003 Bitstream, Inc.; DejaVu
  changes are public domain. See `DejaVuSansMono-LICENSE.txt`.
- `DroidSansFallbackFull.ttf`: unmodified Droid Sans Fallback, from Ubuntu's
  `fonts-droid-fallback` package. Copyright 2006–2010 Google Corp.
  Droid is a trademark of Google Corp. Licensed under Apache 2.0; see
  `DroidSansFallback-LICENSE.txt`.

DejaVu supplies the terminal's monospace metrics and combining glyphs. Droid
supplies CJK glyphs; its proportional advances never determine terminal column
positions. egui's embedded defaults provide further symbol fallbacks.
