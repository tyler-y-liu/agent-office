# Third-party notices

agent-office is distributed both as source and as a prebuilt macOS binary
(`agent-office` in the release tarball, installed by Homebrew). The prebuilt
binary statically links the components below, and it contains the Fusion Pixel
font data embedded with `include_bytes!` (`src/floating/font.rs`). Shipping the
binary therefore distributes those components and that font, so this file
reproduces the copyright notices and license texts that their licenses require
to travel with every copy.

The font license texts are also shipped verbatim as separate files, both in
the release tarball (`licenses/…`) and, when installed with Homebrew, under
`$(brew --prefix)/share/agent-office/licenses/…`. In addition, `LICENSE` in the
package carries the MIT text of agent-office and of pixtuoid, and softbuffer's
MIT text is reproduced in section 3. The exact paths are named in each
section.

Components covered here, with the version this distribution builds:

| Component | Version / form | License |
| --- | --- | --- |
| agent-office | this project | MIT |
| pixtuoid-core, pixtuoid-scene | 0.21.0, vendored and modified | MIT |
| softbuffer | 0.4.8, vendored and modified | MIT OR Apache-2.0 (this distribution uses MIT) |
| Fusion Pixel Font | 8px bitmap, embedded | SIL Open Font License 1.1 |

---

## 1. agent-office

Source: <https://github.com/tyler-y-liu/agent-office>

MIT License

Copyright (c) 2026 tyler-y-liu

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

---

## 2. pixtuoid (pixtuoid-core 0.21.0, pixtuoid-scene 0.21.0)

Source: <https://github.com/pixtuoid/pixtuoid>

The crates in `vendor/pixtuoid-core` and `vendor/pixtuoid-scene`, and the parts
of `src/` noted as adapted from pixtuoid, are derived from pixtuoid and are
distributed under its MIT license. They have been modified for agent-office;
the modifications are marked `agent-office` in the sources.

MIT License

Copyright (c) 2026 Ivan Wang

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

---

## 3. softbuffer

Source: <https://github.com/rust-windowing/softbuffer>

The vendored crate in `vendor/softbuffer` is a modified copy of softbuffer. It
is dual-licensed upstream as `MIT OR Apache-2.0`; this project distributes it
under the MIT option, reproduced in full below. The Apache License 2.0
alternative, which this distribution does not rely on, is available at
<https://github.com/rust-windowing/softbuffer/blob/master/LICENSE-APACHE> and
in the vendored source at `vendor/softbuffer/LICENSE-APACHE`.

Copyright 2022 Kirill Chibisov

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to do
so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL
THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

---

## 4. Fusion Pixel Font

Source: <https://github.com/TakWolf/fusion-pixel-font>

agent-office embeds the 8px bitmap cut of Fusion Pixel (`assets/fusion-pixel-8px.bin`,
included into the binary at `src/floating/font.rs`) and uses it for all text in
the office window. The font data is Font Software under the SIL Open Font
License 1.1, which requires that each copy that bundles the font carry the
copyright notice and the license text (OFL 1.1, condition 2).

### 4.1 Copyright and license of the embedded font

Fusion Pixel Font
<https://github.com/TakWolf/fusion-pixel-font>

Copyright (c) 2022, TakWolf (<https://takwolf.com>).

This Font Software is licensed under the SIL Open Font License, Version 1.1.
The full license text is reproduced in section 4.3 and is also shipped
verbatim as `licenses/OFL-Fusion-Pixel.txt` (repository path
`assets/OFL-Fusion-Pixel.txt`, identical to
`vendor/pixtuoid-scene/fonts/OFL-Fusion-Pixel.txt`).

### 4.2 Upstream component fonts

Fusion Pixel is built from the bitmap fonts below. Their notices are
reproduced here, and each license file is also shipped verbatim under
`licenses/LICENSES-Fusion-Pixel/` (repository path
`assets/LICENSES-Fusion-Pixel/`, identical to
`vendor/pixtuoid-scene/fonts/LICENSES-Fusion-Pixel/`).

**BoutiqueBitmap7x7** — `licenses/LICENSES-Fusion-Pixel/boutique-bitmap-7x7/OFL.txt`

```
[BoutiqueBitmap7x7]
2020-2026《字言字型》Cen-cyun, Liu. Luke Liu.
https://fontspeech.blogspot.com/
These fonts are free software.
Unlimited permission is granted to use, copy, and distribute them, with or without modification, either commercially or noncommercially.
THESE FONTS ARE PROVIDED "AS IS" WITHOUT WARRANTY.
此字型是免費的。
無論您是否進行對本字型進行商業或非商業性修改，均可無限制地使用，複製和分發它們。
本字型的衍生品之授權必須與此字型相同，且不作任何擔保。
```

**MisakiGothic / Misaki (美咲フォント)** — `licenses/LICENSES-Fusion-Pixel/misaki/misaki.txt`

```
Copyright (C) 2002-2021 Num Kadoma

These fonts are free softwares.
Unlimited permission is granted to use, copy, and distribute it, with or without modification, either commercially and noncommercially.
THESE FONTS ARE PROVIDED "AS IS" WITHOUT WARRANTY.

これらのフォントはフリー（自由な）ソフトウエアです。
あらゆる改変の有無に関わらず、また商業的な利用であっても、自由にご利用、複製、再配布することができますが、全て無保証とさせていただきます。
```

**观致 (Guanzhi)** — part of the BoutiqueBitmap7x7 notice file above; it is
built on Misaki and is licensed under the SIL Open Font License 1.1, whose
text is section 4.3.

**Galmuri** — `licenses/LICENSES-Fusion-Pixel/galmuri/LICENSE.txt`

```
Copyright (c) 2019–2025 Lee Minseo (quiple@quiple.dev)

This font software is licensed under the SIL Open Font License, Version 1.1.
```

**Miseki Bitmap** — `licenses/LICENSES-Fusion-Pixel/miseki-bitmap/LICENSE.txt`

```
Copyright (c) 2023-2024 Mark Li (itmarkibfb@gmail.com)

This Font Software is licensed under the SIL Open Font License, Version 1.1.
```

### 4.3 SIL Open Font License 1.1 (full text)

```
SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font creation
efforts of academic and linguistic communities, and to provide a free and
open framework in which fonts may be shared and improved in partnership
with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded,
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply
to any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software components as
distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to, deleting,
or substituting -- in part or in whole -- any of the components of the
Original Version, by changing formats or by porting the Font Software to a
new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed, modify,
redistribute, and sell modified and unmodified copies of the Font
Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components,
in Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the corresponding
Copyright Holder. This restriction only applies to the primary font name as
presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created
using the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.
```

---

## 5. Source locations

| Component | Source |
| --- | --- |
| agent-office | <https://github.com/tyler-y-liu/agent-office> |
| pixtuoid (pixtuoid-core, pixtuoid-scene) | <https://github.com/pixtuoid/pixtuoid> |
| softbuffer | <https://github.com/rust-windowing/softbuffer> |
| Fusion Pixel Font | <https://github.com/TakWolf/fusion-pixel-font> |
| BoutiqueBitmap7x7 | <https://fontspeech.blogspot.com/> |
| Misaki font | <http://littlelimit.net/> |
| Galmuri | <https://github.com/quiple/galmuri> |
| Miseki Bitmap | <https://github.com/ItMarki/MisekiBitmap> |
| SIL Open Font License 1.1 | <https://openfontlicense.org> |
