//! What kind of bytes these are — decided by looking at them.
//!
//! Never by the file extension and never by the content type the browser
//! announced: both are chosen by whoever is uploading. This is an **allow**
//! list of four formats, and the reason it is one is SVG, which is executable
//! XML that happens to be called an image.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaType {
    Jpeg,
    Png,
    Gif,
    Webp,
}

impl MediaType {
    pub fn mime(&self) -> &'static str {
        match self {
            MediaType::Jpeg => "image/jpeg",
            MediaType::Png => "image/png",
            MediaType::Gif => "image/gif",
            MediaType::Webp => "image/webp",
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            MediaType::Jpeg => "jpg",
            MediaType::Png => "png",
            MediaType::Gif => "gif",
            MediaType::Webp => "webp",
        }
    }

    pub fn from_mime(mime: &str) -> Option<Self> {
        match mime {
            "image/jpeg" => Some(MediaType::Jpeg),
            "image/png" => Some(MediaType::Png),
            "image/gif" => Some(MediaType::Gif),
            "image/webp" => Some(MediaType::Webp),
            _ => None,
        }
    }
}

/// The allow list, read from the first bytes.
pub fn detect(data: &[u8]) -> Option<MediaType> {
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(MediaType::Jpeg);
    }
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(MediaType::Png);
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return Some(MediaType::Gif);
    }
    if data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        return Some(MediaType::Webp);
    }
    None
}

/// What a picture knows besides the picture: where it was taken, when, with
/// which camera, under which name. A phone writes all of that into every
/// photograph, and until 2026-09-20 treff stored and served it untouched.
///
/// This **removes segments, it does not re-encode**. Decoding an upload to
/// encode it again would strip everything by construction — and would also
/// mean running a decoder over bytes a stranger chose, lose quality on every
/// JPEG, and flatten animations. The whole design keeps foreign bytes out of
/// anything that interprets them (that is why the format list is an allow
/// list); a walk along the segment lengths interprets nothing. The trade is
/// written down in ADR 0004.
///
/// It never rejects and never guesses: bytes it cannot walk come back exactly
/// as they arrived. Refusing is the allow list's job, and `detect` has
/// already run by the time anything gets here.
pub fn strip_metadata(data: &[u8], kind: MediaType) -> Vec<u8> {
    let stripped = match kind {
        MediaType::Jpeg => strip_jpeg(data),
        MediaType::Png => strip_png(data),
        MediaType::Webp => strip_webp(data),
        // GIF carries no EXIF, and no camera writes one. Walking its
        // sub-block chains for comment extensions would put animations at
        // risk to remove metadata nothing produces. ADR 0004.
        MediaType::Gif => None,
    };
    stripped.unwrap_or_else(|| data.to_vec())
}

/// Does this JPEG segment carry something about the photographer rather than
/// about the picture?
///
/// APP1 is Exif and XMP, APP13 is IPTC — those are the ones with the
/// location in them. The rest of the APPn range is vendor territory nobody
/// here needs, and `COM` is a free-text comment. Three are kept on purpose:
/// APP0 is JFIF (density), APP2 is the ICC colour profile, APP14 is Adobe's
/// colour transform. Drop those three and the picture itself changes.
fn is_jpeg_metadata(marker: u8) -> bool {
    matches!(marker, 0xE1 | 0xE3..=0xED | 0xEF | 0xFE)
}

fn strip_jpeg(data: &[u8]) -> Option<Vec<u8>> {
    if !data.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut out = vec![0xFF, 0xD8];
    let mut i = 2;
    loop {
        if i + 1 >= data.len() || data[i] != 0xFF {
            return None;
        }
        let marker = data[i + 1];
        match marker {
            // A run of 0xFF is legal padding in front of a marker.
            0xFF => {
                out.push(0xFF);
                i += 1;
            }
            // The start of scan: from here on it is the picture, entropy
            // coded, and a byte that looks like a marker is not one. Copy
            // the rest and stop walking.
            0xDA | 0xD9 => {
                out.extend_from_slice(&data[i..]);
                return Some(out);
            }
            // Markers that stand alone, without a length after them.
            0x01 | 0xD0..=0xD8 => {
                out.extend_from_slice(&[0xFF, marker]);
                i += 2;
            }
            _ => {
                if i + 3 >= data.len() {
                    return None;
                }
                let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
                // The length counts itself. Anything shorter is not a
                // segment, and trusting it would walk backwards forever.
                if len < 2 {
                    return None;
                }
                let end = i.checked_add(2)?.checked_add(len)?;
                if end > data.len() {
                    return None;
                }
                if !is_jpeg_metadata(marker) {
                    out.extend_from_slice(&data[i..end]);
                }
                i = end;
            }
        }
    }
}

/// PNG's own text chunks, plus the EXIF block a phone writes and the
/// timestamp. Everything else — header, palette, transparency, pixels — is
/// the picture.
const PNG_METADATA: [&[u8; 4]; 5] = [b"eXIf", b"tEXt", b"zTXt", b"iTXt", b"tIME"];

fn strip_png(data: &[u8]) -> Option<Vec<u8>> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if !data.starts_with(SIGNATURE) {
        return None;
    }
    let mut out = SIGNATURE.to_vec();
    let mut i = SIGNATURE.len();
    while i < data.len() {
        if i + 8 > data.len() {
            return None;
        }
        let len = u32::from_be_bytes(data[i..i + 4].try_into().ok()?) as usize;
        let kind = &data[i + 4..i + 8];
        // Four bytes of length, four of type, four of CRC.
        let end = i.checked_add(12)?.checked_add(len)?;
        if end > data.len() {
            return None;
        }
        if !PNG_METADATA.iter().any(|m| m.as_slice() == kind) {
            out.extend_from_slice(&data[i..end]);
        }
        i = end;
    }
    Some(out)
}

fn strip_webp(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 12 || !data.starts_with(b"RIFF") || &data[8..12] != b"WEBP" {
        return None;
    }
    let declared = u32::from_le_bytes(data[4..8].try_into().ok()?) as usize;
    // The declared length covers "WEBP" and everything after it. A file that
    // does not say at least that much, or claims more than it brought, is
    // not one this walk can trust.
    if declared < 4 || 8usize.checked_add(declared)? > data.len() {
        return None;
    }
    let payload_end = 8 + declared;

    let mut body = b"WEBP".to_vec();
    let mut flags_at = None;
    let mut i = 12;
    while i < payload_end {
        if i + 8 > payload_end {
            return None;
        }
        let kind = &data[i..i + 4];
        let len = u32::from_le_bytes(data[i + 4..i + 8].try_into().ok()?) as usize;
        // Every chunk is padded to an even length.
        let end = i.checked_add(8)?.checked_add(len)?.checked_add(len & 1)?;
        if end > payload_end {
            return None;
        }
        if kind != b"EXIF" && kind != b"XMP " {
            if kind == b"VP8X" {
                flags_at = Some(body.len() + 8);
            }
            body.extend_from_slice(&data[i..end]);
        }
        i = end;
    }

    // VP8X announces in its first byte which optional chunks are present.
    // Removing the chunks and leaving the bits set points a decoder at
    // something that is no longer there: bit 3 is EXIF, bit 2 is XMP.
    if let Some(at) = flags_at
        && at < body.len()
    {
        body[at] &= !0x0c;
    }

    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_the_four_allowed_formats() {
        assert_eq!(detect(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(MediaType::Jpeg));
        assert_eq!(
            detect(b"\x89PNG\r\n\x1a\n....").map(|m| m.mime()),
            Some("image/png")
        );
        assert_eq!(
            detect(b"GIF89a.......").map(|m| m.mime()),
            Some("image/gif")
        );
        assert_eq!(
            detect(b"RIFF\x00\x00\x00\x00WEBPVP8 ").map(|m| m.mime()),
            Some("image/webp")
        );
    }

    #[test]
    fn refuses_svg_even_though_it_is_an_image() {
        // SVG is executable XML. It is the reason this list is an allow list
        // and not a deny list.
        assert_eq!(
            detect(b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"),
            None
        );
        // Including with a byte-order mark or leading whitespace in front.
        assert_eq!(detect(b"\xef\xbb\xbf<svg>"), None);
        assert_eq!(detect(b"\n  <svg>"), None);
    }

    #[test]
    fn refuses_html_and_scripts() {
        assert_eq!(detect(b"<!DOCTYPE html><script>alert(1)</script>"), None);
        assert_eq!(detect(b"#!/bin/sh\nrm -rf /"), None);
        assert_eq!(detect(b"%PDF-1.7"), None);
    }

    #[test]
    fn a_png_header_on_html_content_is_still_only_a_png_header() {
        // Nothing is claimed about the rest of the file. It is served later
        // with a FIXED content type and nosniff, so a browser does not get to
        // reconsider.
        assert_eq!(detect(b"\x89PNG\r\n\x1a\n<script>"), Some(MediaType::Png));
    }

    #[test]
    fn an_empty_or_short_file_is_refused_without_panicking() {
        assert_eq!(detect(b""), None);
        assert_eq!(detect(b"\x89PN"), None);
        assert_eq!(detect(b"RIFF"), None, "a truncated header must not index");
        assert_eq!(detect(b"RIFF\x00\x00\x00\x00WEB"), None);
    }

    #[test]
    fn riff_without_webp_is_refused() {
        assert_eq!(detect(b"RIFF\x00\x00\x00\x00AVI LIST"), None);
        assert_eq!(detect(b"RIFF\x00\x00\x00\x00WAVEfmt "), None);
    }

    #[test]
    fn the_extension_never_comes_from_the_upload() {
        // The stored name is built from what we detected, so ".php" or
        // "../../etc/passwd" in a file name cannot reach the disk.
        for (bytes, ext) in [(&b"\x89PNG\r\n\x1a\n"[..], "png"), (&b"GIF89a"[..], "gif")] {
            let detected = detect(bytes).expect("detected");
            assert!(detected.extension().chars().all(|c| c.is_ascii_lowercase()));
            assert_eq!(detected.extension(), ext);
        }
    }

    /// A JPEG: start of image, the segments handed in, then the start of
    /// scan and a byte of "pixels". Enough structure for a walker, not a
    /// picture — nothing here decodes it.
    fn jpeg_with(segments: &[(u8, &[u8])]) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8];
        for (marker, payload) in segments {
            v.push(0xFF);
            v.push(*marker);
            let len = (payload.len() + 2) as u16;
            v.extend_from_slice(&len.to_be_bytes());
            v.extend_from_slice(payload);
        }
        v.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02, 0x42, 0x42, 0xFF, 0xD9]);
        v
    }

    fn png_with(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
        for (kind, payload) in chunks {
            v.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            v.extend_from_slice(*kind);
            v.extend_from_slice(payload);
            v.extend_from_slice(&[0, 0, 0, 0]); // CRC: never read, never rewritten
        }
        v
    }

    fn webp_with(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut payload = b"WEBP".to_vec();
        for (kind, data) in chunks {
            payload.extend_from_slice(*kind);
            payload.extend_from_slice(&(data.len() as u32).to_le_bytes());
            payload.extend_from_slice(data);
            if data.len() % 2 == 1 {
                payload.push(0); // RIFF pads every odd chunk
            }
        }
        let mut v = b"RIFF".to_vec();
        v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        v.extend_from_slice(&payload);
        v
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    /// The reason this function exists: a photograph from a phone carries
    /// where it was taken, and until 2026-09-20 treff stored and served
    /// those bytes untouched. Measured by the audit on 2026-09-15 — the
    /// GPS IFD came back out byte for byte.
    #[test]
    fn a_jpegs_exif_segment_does_not_survive() {
        let exif = b"Exif\x00\x00II*\x00GPSLatitude 51.9";
        let original = jpeg_with(&[(0xE1, exif)]);
        assert!(contains(&original, b"GPSLatitude"), "the fixture is wrong");

        let stripped = strip_metadata(&original, MediaType::Jpeg);

        assert!(!contains(&stripped, b"GPSLatitude"));
        assert!(!contains(&stripped, b"Exif"));
        assert_eq!(
            detect(&stripped),
            Some(MediaType::Jpeg),
            "what comes out is still a JPEG"
        );
    }

    #[test]
    fn a_jpegs_xmp_and_iptc_do_not_survive_either() {
        // XMP rides in APP1 under a different header, IPTC in APP13. Both
        // carry names, locations and camera serial numbers.
        let original = jpeg_with(&[
            (
                0xE1,
                b"http://ns.adobe.com/xap/1.0/\x00<x:xmpmeta>Ada</x:xmpmeta>",
            ),
            (0xED, b"Photoshop 3.0\x008BIM\x04\x04somewhere"),
        ]);
        let stripped = strip_metadata(&original, MediaType::Jpeg);
        assert!(!contains(&stripped, b"xmpmeta"));
        assert!(!contains(&stripped, b"8BIM"));
    }

    #[test]
    fn a_jpegs_own_machinery_is_left_alone() {
        // APP0 is JFIF (density, thumbnails), APP14 is Adobe's colour
        // transform: drop those and the picture changes. Everything after
        // the start of scan is the picture itself and is copied verbatim,
        // even where it happens to spell a marker.
        let original = jpeg_with(&[
            (0xE0, b"JFIF\x00\x01\x02\x00\x00\x01\x00\x01\x00\x00"),
            (0xEE, b"Adobe\x00\x64\x00\x00\x00\x00\x02"),
            (0xDB, b"\x00quantisation"),
        ]);
        let stripped = strip_metadata(&original, MediaType::Jpeg);
        assert!(contains(&stripped, b"JFIF"));
        assert!(contains(&stripped, b"Adobe"));
        assert!(contains(&stripped, b"quantisation"));
        assert!(
            stripped.ends_with(&[0xFF, 0xDA, 0x00, 0x02, 0x42, 0x42, 0xFF, 0xD9]),
            "the scan and what follows it are untouched"
        );
    }

    #[test]
    fn a_jpeg_with_nothing_to_remove_comes_back_unchanged() {
        let original = jpeg_with(&[(0xDB, b"\x00quantisation")]);
        assert_eq!(strip_metadata(&original, MediaType::Jpeg), original);
    }

    #[test]
    fn a_pngs_exif_and_text_chunks_do_not_survive() {
        let original = png_with(&[
            (
                b"IHDR",
                b"\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00",
            ),
            (b"eXIf", b"II*\x00GPSLatitude 51.9"),
            (b"tEXt", b"Author\x00Ada"),
            (
                b"iTXt",
                b"XML:com.adobe.xmp\x00\x00\x00\x00\x00<x:xmpmeta/>",
            ),
            (b"IDAT", b"pixels"),
            (b"IEND", b""),
        ]);
        let stripped = strip_metadata(&original, MediaType::Png);

        assert!(!contains(&stripped, b"eXIf"));
        assert!(!contains(&stripped, b"GPSLatitude"));
        assert!(!contains(&stripped, b"tEXt"));
        assert!(!contains(&stripped, b"iTXt"));
        assert!(contains(&stripped, b"IHDR"), "the header is not metadata");
        assert!(contains(&stripped, b"IDAT"), "nor are the pixels");
        assert!(contains(&stripped, b"IEND"));
        assert_eq!(detect(&stripped), Some(MediaType::Png));
    }

    #[test]
    fn a_webps_exif_and_xmp_chunks_do_not_survive() {
        let original = webp_with(&[
            (b"VP8X", b"\x0c\x00\x00\x00\x00\x00\x00\x00\x00\x00"),
            (b"VP8 ", b"pixels"),
            (b"EXIF", b"II*\x00GPSLatitude 51.9"),
            (b"XMP ", b"<x:xmpmeta>Ada</x:xmpmeta>"),
        ]);
        let stripped = strip_metadata(&original, MediaType::Webp);

        assert!(!contains(&stripped, b"GPSLatitude"));
        assert!(!contains(&stripped, b"xmpmeta"));
        assert!(contains(&stripped, b"pixels"));
        assert_eq!(detect(&stripped), Some(MediaType::Webp));

        // The container says how long it is. A stripper that forgets to say
        // so leaves a file every decoder reads past the end of.
        let declared = u32::from_le_bytes(stripped[4..8].try_into().expect("four bytes"));
        assert_eq!(
            declared as usize,
            stripped.len() - 8,
            "the RIFF length was not corrected"
        );

        // VP8X announces in a flag byte that those chunks are there. Leaving
        // the flags set points a decoder at chunks that are gone. The byte:
        // past the RIFF header (8), past "WEBP" (4), past the chunk's own
        // header (8).
        let flags = stripped[8 + 4 + 8];
        assert_eq!(flags & 0x0c, 0, "the EXIF and XMP flags are still set");
    }

    #[test]
    fn bytes_that_cannot_be_walked_come_back_exactly_as_they_arrived() {
        // Truncated, nonsensical or simply not what the sniffer thought:
        // this function never rejects and never guesses. Refusing is the
        // allow list's job, and it has already run.
        for (bytes, kind) in [
            (&b""[..], MediaType::Jpeg),
            (&b"\xFF\xD8\xFF"[..], MediaType::Jpeg),
            (&b"\xFF\xD8\xFF\xE1\x00"[..], MediaType::Jpeg),
            (
                &b"\xFF\xD8\xFF\xE1\xFF\xFFshorter than it claims"[..],
                MediaType::Jpeg,
            ),
            (&b"\x89PNG\r\n\x1a\n\x00\x00"[..], MediaType::Png),
            (
                &b"\x89PNG\r\n\x1a\n\xFF\xFF\xFF\xFFeXIf"[..],
                MediaType::Png,
            ),
            (&b"RIFF\x00\x00\x00\x00WEBP"[..], MediaType::Webp),
            (&b"RIFF"[..], MediaType::Webp),
        ] {
            assert_eq!(
                strip_metadata(bytes, kind),
                bytes.to_vec(),
                "changed bytes it could not parse: {bytes:?}"
            );
        }
    }

    #[test]
    fn a_gif_is_handed_back_untouched() {
        // Decided in ADR 0004: GIF carries no EXIF, and no camera writes
        // one. Walking its sub-block chains to find comment extensions
        // would risk animations for metadata nothing produces.
        let original = b"GIF89a\x21\xFE\x05hello\x00;".to_vec();
        assert_eq!(strip_metadata(&original, MediaType::Gif), original);
    }
}
