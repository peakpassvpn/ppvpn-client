# Third-party notices

## sing-box v1.13.12

`ppvpn-core` incorporates and links to [sing-box](https://github.com/SagerNet/sing-box), licensed under the GNU General Public License version 3 or later with the following additional term:

> Copyright (C) 2022 by nekohasekai <contact-sagernet@sekai.icu>
>
> This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.
>
> This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
>
> You should have received a copy of the GNU General Public License along with this program. If not, see <http://www.gnu.org/licenses/>.
>
> In addition, no derivative work may use the name or imply association with this application without prior consent.

PPVPN and `ppvpn-core` are not affiliated with or endorsed by the sing-box project or its maintainers.

## The Rust `ppvpn-core` (`crates/`)

The Rust `ppvpn-core`, `ppvpn-account` and `ppvpn-cli` crates are built from source together with their dependencies, which are linked into the binaries. Every dependency's license is compatible with the GNU GPL v3 or later; the complete list, with each crate's declared license, is what `cargo metadata --format-version 1` reports for this workspace's `Cargo.lock`, and each crate's own license text ships with its source.

### Sail

`ppvpn-core` links [Sail](https://github.com/peakpassvpn/sail), licensed under the [Apache License 2.0](https://www.apache.org/licenses/LICENSE-2.0); code Sail took from leaf keeps its original copyright. Sail's own third-party notices are in its `THIRD_PARTY_LICENSES.md`; the parts that reach `ppvpn-core`'s binaries are repeated below.

### BoringSSL (through `btls` and `btls-sys`)

TLS, REALITY included, uses [BoringSSL](https://boringssl.googlesource.com/boringssl), built from source by `btls-sys` and linked statically. BoringSSL is licensed under the [Apache License 2.0](https://www.apache.org/licenses/LICENSE-2.0) (its `LICENSE` file, which also covers the third-party code it includes). The bindings are `btls` (Apache-2.0) and `btls-sys` (MIT).

### Patched dependencies

The workspace's `[patch.crates-io]` builds these from forks, under their upstream licenses: `btls` and `btls-sys` (above), `quinn-proto` (MIT OR Apache-2.0), `route_manager` (Apache-2.0) and `netconfig-rs` (MIT).

### webpki-root-certs 1.0.9

Sail (and so the Rust `ppvpn-core`) includes Mozilla trusted root certificate data distributed by
`webpki-root-certs`. The data is derived from the Common CA Database (CCADB)
and is provided under the following agreement.

Source: <https://github.com/rustls/webpki-roots>

#### Community Data License Agreement – Permissive – Version 2.0

This is the Community Data License Agreement – Permissive, Version 2.0 (the
“agreement”). Data Provider(s) and Data Recipient(s) agree as follows:

##### 1. Provision of the Data

1.1. A Data Recipient may use, modify, and share the Data made available by
Data Provider(s) under this agreement if that Data Recipient follows the terms
of this agreement.

1.2. This agreement does not impose any restriction on a Data Recipient’s use,
modification, or sharing of any portions of the Data that are in the public
domain or that may be used, modified, or shared under any other legal exception
or limitation.

##### 2. Conditions for Sharing Data

2.1. A Data Recipient may share Data, with or without modifications, so long as
the Data Recipient makes available the text of this agreement with the shared
Data.

##### 3. No Restrictions on Results

3.1. This agreement does not impose any restriction or obligations with
respect to the use, modification, or sharing of Results.

##### 4. No Warranty; Limitation of Liability

4.1. All Data Recipients receive the Data subject to the following terms:

THE DATA IS PROVIDED ON AN “AS IS” BASIS, WITHOUT REPRESENTATIONS, WARRANTIES
OR CONDITIONS OF ANY KIND, EITHER EXPRESS OR IMPLIED INCLUDING, WITHOUT
LIMITATION, ANY WARRANTIES OR CONDITIONS OF TITLE, NON-INFRINGEMENT,
MERCHANTABILITY OR FITNESS FOR A PARTICULAR PURPOSE.

NO DATA PROVIDER SHALL HAVE ANY LIABILITY FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING WITHOUT LIMITATION
LOST PROFITS), HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING
IN ANY WAY OUT OF THE DATA OR RESULTS, EVEN IF ADVISED OF THE POSSIBILITY OF
SUCH DAMAGES.

##### 5. Definitions

5.1. “Data” means the material received by a Data Recipient under this
agreement.

5.2. “Data Provider” means any person who is the source of Data provided under
this agreement and in reliance on a Data Recipient’s agreement to its terms.

5.3. “Data Recipient” means any person who receives Data directly or indirectly
from a Data Provider and agrees to the terms of this agreement.

5.4. “Results” means any outcome obtained by computational analysis of Data,
including for example machine learning models and models’ insights.

### tun 0.7.22

Sail uses `tun` to create and operate cross-platform TUN interfaces. The crate
declares the following license.

Source: <https://github.com/meh/rust-tun>

#### DO WHAT THE FUCK YOU WANT TO PUBLIC LICENSE

Version 2, December 2004

Copyright (C) 2004 Sam Hocevar <sam@hocevar.net>

Everyone is permitted to copy and distribute verbatim or modified copies of
this license document, and changing it is allowed as long as the name is
changed.

DO WHAT THE FUCK YOU WANT TO PUBLIC LICENSE TERMS AND CONDITIONS FOR COPYING,
DISTRIBUTION AND MODIFICATION

0. You just DO WHAT THE FUCK YOU WANT TO.

### Unicode data (ICU4X crates)

IDNA processing uses ICU4X crates (`icu_collections`, `icu_locale_core`, `icu_normalizer`, `icu_normalizer_data`, `icu_properties`, `icu_properties_data`, `icu_provider`, `litemap`, `potential_utf`, `tinystr`, `writeable`, `yoke`, `yoke-derive`, `zerofrom`, `zerofrom-derive`, `zerotrie`, `zerovec`, `zerovec-derive`) and `unicode-ident`, licensed under the Unicode License v3:

```text
UNICODE LICENSE V3

COPYRIGHT AND PERMISSION NOTICE

Copyright © 2020-2024 Unicode, Inc.

NOTICE TO USER: Carefully read the following legal agreement. BY
DOWNLOADING, INSTALLING, COPYING OR OTHERWISE USING DATA FILES, AND/OR
SOFTWARE, YOU UNEQUIVOCALLY ACCEPT, AND AGREE TO BE BOUND BY, ALL OF THE
TERMS AND CONDITIONS OF THIS AGREEMENT. IF YOU DO NOT AGREE, DO NOT
DOWNLOAD, INSTALL, COPY, DISTRIBUTE OR USE THE DATA FILES OR SOFTWARE.

Permission is hereby granted, free of charge, to any person obtaining a
copy of data files and any associated documentation (the "Data Files") or
software and any associated documentation (the "Software") to deal in the
Data Files or Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, and/or sell
copies of the Data Files or Software, and to permit persons to whom the
Data Files or Software are furnished to do so, provided that either (a)
this copyright and permission notice appear with all copies of the Data
Files or Software, or (b) this copyright and permission notice appear in
associated Documentation.

THE DATA FILES AND SOFTWARE ARE PROVIDED "AS IS", WITHOUT WARRANTY OF ANY
KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT OF
THIRD PARTY RIGHTS.

IN NO EVENT SHALL THE COPYRIGHT HOLDER OR HOLDERS INCLUDED IN THIS NOTICE
BE LIABLE FOR ANY CLAIM, OR ANY SPECIAL INDIRECT OR CONSEQUENTIAL DAMAGES,
OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS,
WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION,
ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THE DATA
FILES OR SOFTWARE.

Except as contained in this notice, the name of a copyright holder shall
not be used in advertising or otherwise to promote the sale, use or other
dealings in these Data Files or Software without prior written
authorization of the copyright holder.

SPDX-License-Identifier: Unicode-3.0

—

Portions of ICU4X may have been adapted from ICU4C and/or ICU4J.
ICU 1.8.1 to ICU 57.1 © 1995-2016 International Business Machines Corporation and others.
```

### Other dependencies

The remaining crates are under permissive licenses, most under MIT OR Apache-2.0, others under MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, 0BSD, BSL-1.0, Unlicense or CC0-1.0 (some offering several of these). Their copyright and license notices are kept with each crate's source.

PPVPN and `ppvpn-core` are not affiliated with or endorsed by the authors of these components.

