# Media and codec policy for Takt 1.0

This matrix describes the shipped Windows package. Codec patents and required
royalties vary by country and use case; an open-source license does not grant a
universal patent clearance.

## Playback

Takt ships one upstream LGPL libmpv/FFmpeg runtime. Actual detection is based on
the file contents, not the filename extension.

| Family | Typical containers | 1.0 playback | Source-code license route | Patent note |
| --- | --- | --- | --- | --- |
| H.264/AVC + AAC | MP4, MOV, MKV, M4A | Yes | LGPL FFmpeg decoder path | Patent pools exist in some jurisdictions. |
| HEVC/H.265 | MP4, MOV, MKV | Yes | LGPL FFmpeg decoder path | Patent pools exist; terms are jurisdiction-specific. |
| VP8/VP9 + Opus/Vorbis | WebM, MKV, Ogg | Yes | Permissive/BSD-style codec libraries through LGPL FFmpeg | Published royalty-free grants reduce, but do not eliminate, legal risk. |
| AV1 + Opus | WebM, MKV, MP4 | Yes | AOM/BSD-style decoder through LGPL FFmpeg | AOMedia publishes a patent license; third-party claims remain possible. |
| MPEG-1/2, MPEG-4 Part 2, MP3 | MPG, TS, AVI, MP3 | Yes | LGPL FFmpeg decoder path | Some historical patents have expired in many countries; no universal claim is made. |
| FLAC, ALAC, PCM, WavPack, APE, TTA | FLAC, M4A, WAV, AIFF, WV, APE, TTA | Yes | Permissive/LGPL-compatible decoder paths | Generally lower codec-patent concern; container and bundled metadata still need robust parsing. |
| Images used as media or artwork | JPEG, PNG, GIF, WebP, BMP | Yes | Rust `image` graph and/or LGPL FFmpeg | No patent-clearance claim. Resource limits are enforced before decode. |
| DVD/Blu-ray menus and network streaming | Disc/network formats | Not a supported 1.0 promise | Some libraries exist in upstream builds | Deliberately outside the local-file product contract. |

Container support means demuxing a container whose streams use supported codecs;
it does not mean every damaged, encrypted, DRM-protected or unusual file will
play. DRM circumvention is not included.

## Conversion

Takt 1.0 does **not** redistribute `ffmpeg.exe`. The converter uses, in order,
the explicit `TAKT_FFMPEG` path, `ffmpeg.exe` beside Takt, `tools/ffmpeg.exe`,
or the user's `PATH`. Its probe reports when that executable is missing or
incompatible.

The UI can request MP4/H.264, MP4/H.265, WebM, MP3, AAC/M4A, FLAC, WAV, Opus and
stream-copy profiles, but availability and license obligations are properties of
the exact user-supplied FFmpeg build. Takt never silently downloads a codec or
claims that a click transfers the distribution or patent responsibility.

## Full GPL versus selected LGPL runtime

Both variants were downloaded from upstream run `38326895870` and hash-checked:

| Variant | Archive | DLL | Relevant difference |
| --- | ---: | ---: | --- |
| GPL full | 64,901,987 bytes | 157,854,720 bytes | Adds GPL-only dependencies/features such as DVD navigation and Rubber Band. |
| LGPL selected for 1.0 | 56,765,949 bytes | 127,147,520 bytes | Retains the local-file decoding needed by Takt with `-Dgpl=false`. |

The LGPL variant saves 8,136,038 compressed bytes and 30,707,200 installed
bytes. A future royalty-free-only modular media pack remains a post-1.0 research
item: the upstream LGPL build is smaller, but it is not a patent-cleared subset.
