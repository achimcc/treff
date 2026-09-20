# ADR 0004 — An upload's metadata is stripped, not re-encoded

**Date:** 2026-09-20
**Status:** accepted
**Decision:** `media::strip_metadata` walks an upload's own segment and chunk
lengths and drops the ones that carry metadata. It never decodes the picture
and never encodes one. GIF is handed back untouched.

## Context

A security audit on 2026-09-15 uploaded a JPEG carrying a GPS IFD, read the
file back out of `/a/<id>`, and found it byte for byte identical: treff was
handing everyone who could read a topic the place a photograph was taken,
the moment it was taken, and the camera that took it. A phone writes all of
that into every picture it makes, and nobody who attaches a holiday photo to
a thread is thinking about it.

So something has to remove it. The question is what.

## The two ways to do it

**Re-encoding.** Decode the upload to pixels with an image library, encode a
new file from them. Everything that is not a pixel is gone by construction —
there is no list of metadata containers to keep up to date, and no format
whose quirks were overlooked.

**Stripping.** Walk the container's own length fields and copy every part
except the ones that hold metadata. Nothing is decoded; the pixel data is
copied through as the bytes that arrived.

## Why stripping

1. **Re-encoding runs a decoder over bytes a stranger chose.** That is the
   one thing this design has consistently refused. The format list in
   `media.rs` is an *allow* list, and its own comment says why: SVG is
   executable XML that happens to be called an image. An image decoder is a
   far larger and more interesting target than an XML parser, and image
   decoders are where memory-safety bugs live. Adding one to the request path
   to remove a GPS tag trades a privacy leak for a code-execution surface.
2. **A walk along length fields interprets nothing.** It reads numbers, copies
   ranges and stops when a number does not fit. It cannot be made to allocate
   or index out of bounds by what the file *says* — that is a property of the
   code, and `bytes_that_cannot_be_walked_come_back_exactly_as_they_arrived`
   is the test that holds it.
3. **Re-encoding a JPEG loses quality, every time, for nothing.** The picture
   is re-quantised even though not one pixel needed to change.
4. **Re-encoding flattens what it does not understand** — an animation becomes
   its first frame, transparency and colour profiles survive only as well as
   the round trip happens to manage. Stripping keeps the file otherwise
   exactly as it was.
5. **It costs no dependency.** `strip_metadata` is a hundred lines in a file
   that already existed.

## What is dropped, and what is kept on purpose

* **JPEG:** APP1 (Exif, XMP), APP13 (IPTC) and the whole vendor range, plus
  `COM`. **Kept:** APP0 (JFIF), APP2 (the ICC colour profile) and APP14
  (Adobe's colour transform) — drop those three and the picture itself
  changes. Everything from the start of scan on is copied verbatim, because
  after it a byte that looks like a marker is not one.
* **PNG:** `eXIf`, `tEXt`, `zTXt`, `iTXt`, `tIME`.
* **WebP:** the `EXIF` and `XMP ` chunks — and then the RIFF length is
  corrected and the two flag bits in `VP8X` are cleared. Leaving either alone
  would produce a file that announces chunks it no longer has.
* **GIF:** nothing. GIF has no EXIF and no camera writes one; what it can
  carry is a comment extension, which is reached by walking sub-block chains
  through the image descriptors. That walk is the one place in this function
  where a mistake would corrupt an animation, and it would be removing
  metadata that nothing here produces. If a case ever turns up, it is a
  separate change with its own tests.

## The cost we accept

**The list is a list, and lists go stale.** A format that grows a new metadata
container will keep it until somebody adds the four bytes here — where
re-encoding would have dropped it without being told. That is the real price
of this decision, and it is the reason the doc comment on `strip_metadata`
names the trade rather than merely describing the code.

**It is a best effort, not a guarantee.** The function never rejects: bytes it
cannot walk come back untouched, because refusing is the allow list's job and
`detect` has already run. A file that is malformed enough to stop the walk
therefore keeps whatever it carried. That is the right way round — the
alternative is refusing somebody's holiday photo because a camera wrote a
slightly odd file — but it means "stripped" means "stripped where the
container could be read".

## Where it happens

In the handler, **before** anything is written. Doing it on the way out would
leave the location on the disk, and from there in every backup.
